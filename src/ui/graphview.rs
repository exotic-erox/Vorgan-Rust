//! The graph screen session: graph + particle field + hover/focus/drag,
//! its title/info lanes and the tick cadence. Key dispatch lives in the
//! app's event loop; this struct owns graph-mode behavior.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::graph::Graph;
use crate::particles::ParticleField;
use crate::theme::Palette;

pub const TIP: &str = "click a bubble to dive in · drag one near the centre to pin it · right-click unpins · wheel zooms";

pub struct GraphView {
    pub g: Graph,
    pub size: (u16, u16),
    pub parts: ParticleField,
    pub hover: Option<usize>,
    pub focus: Option<usize>,
    pub drag: Option<usize>,
    down: Option<(usize, f32, f32)>,
    moved: bool,
    pub awake: bool,
    pub idle: u32,
    pub frame: Vec<Vec<crate::graph::GSpan>>,
    /// node the info lane describes (hover or focus)
    pub info: Option<usize>,
}

impl GraphView {
    pub fn new(g: Graph, w: u16, h: u16) -> GraphView {
        let parts = ParticleField::new(w, h);
        let mut v = GraphView {
            g,
            parts,
            hover: None,
            focus: None,
            drag: None,
            down: None,
            moved: false,
            awake: true,
            idle: 0,
            frame: vec![],
            info: None,
            size: (0, 0),
        };
        v.resize(w, h);
        v
    }

    pub fn resize(&mut self, w: u16, h: u16) {
        if self.size == (w, h) {
            return;
        } // python resizes only on layout change
        self.size = (w, h);
        self.g.set_size(w as usize, h as usize);
        self.parts.resize(w, h);
        self.parts.retarget(&self.g);
        self.wake();
    }

    pub fn wake(&mut self) {
        self.awake = true;
        self.idle = 0;
    }

    /// one 30 Hz cadence tick: graph physics while awake, particles always
    pub fn tick(&mut self) {
        if self.awake {
            let e = self.g.step(self.drag);
            self.idle = if e < 0.12 * self.g.nodes.len() as f32 && self.drag.is_none() {
                self.idle + 1
            } else {
                0
            };
            if self.idle > 10 {
                self.awake = false;
            }
        }
        self.parts.step(&self.g, 0.4);
        self.render_frame();
    }

    fn render_frame(&mut self) {
        let mut cv = self.g.render(self.hover.or(self.focus));
        self.parts
            .render(&mut cv, &self.g, self.hover.or(self.focus), 0.55);
        self.frame = cv.to_text();
    }

    pub fn set_hover_from_dots(&mut self, dx: f32, dy: f32) {
        let n = self.g.node_at(dx, dy);
        if n != self.hover {
            self.hover = n;
            self.info = n.or(self.focus);
            self.wake();
        }
    }

    /// mouse press at dot coords; returns Some(idx) when a bubble was grabbed
    pub fn mouse_down(&mut self, dx: f32, dy: f32, right: bool) -> Option<usize> {
        let n = self.g.node_at(dx, dy);
        if right {
            if let Some(i) = n {
                if self.g.unpin(i) {
                    self.wake();
                }
            }
            return None;
        }
        if let Some(i) = n {
            self.down = Some((i, dx, dy));
            self.moved = false;
        }
        n
    }

    /// mouse motion; returns true when a drag carries a node
    pub fn mouse_drag(&mut self, dx: f32, dy: f32) -> bool {
        if self.drag.is_none() {
            if let Some((i, sx, sy)) = self.down {
                if i != 0 && (dx - sx).hypot(dy - sy) > 6.0 {
                    self.moved = true;
                    self.drag = Some(i);
                }
            }
        }
        if let Some(i) = self.drag {
            let (x, y) = self.g.to_world(dx, dy);
            if let Some(n) = self.g.nodes.get_mut(i) {
                n.x = x;
                n.y = y;
                n.vx = 0.0;
                n.vy = 0.0;
            }
            self.wake();
            return true;
        }
        false
    }

    /// mouse release: a drag pins the grabbed kid, a plain click activates —
    /// same as python's on_mouse_up (returns (click_idx, pinned_name))
    pub fn mouse_up(&mut self, _dx: f32, _dy: f32) -> (Option<usize>, Option<String>) {
        let Some((i, _, _)) = self.down.take() else {
            return (None, None);
        };
        if let Some(d) = self.drag.take() {
            if self.g.nodes[d].role == crate::graph::Role::Kid {
                self.g.pin(d);
                let (x, y) = crate::particles::home_of(&self.g, &self.g.nodes[d]);
                self.parts.shock(x, y, 1, 18.0); // a small burst where it snaps in
                self.wake();
                return (None, Some(self.g.nodes[d].name.clone()));
            }
            self.wake();
            return (None, None);
        }
        (Some(i), None)
    }

    pub fn zoom_by(&mut self, f: f32) {
        self.g.zoom = 2.4f32.min(0.5f32.max(self.g.zoom * f));
        self.wake();
    }

    pub fn cycle(&mut self, d: i32) -> Option<usize> {
        let mut ks: Vec<usize> = (0..self.g.nodes.len())
            .filter(|&i| {
                matches!(
                    self.g.nodes[i].role,
                    crate::graph::Role::Kid | crate::graph::Role::More
                )
            })
            .collect();
        if ks.is_empty() {
            return None;
        }
        ks.sort_by(|&a, &b| {
            let (na, nb) = (&self.g.nodes[a], &self.g.nodes[b]);
            (na.y)
                .atan2(na.x)
                .partial_cmp(&(nb.y).atan2(nb.x))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let pos = self
            .focus
            .and_then(|f| ks.iter().position(|&k| k == f))
            .unwrap_or(if d > 0 { ks.len() - 1 } else { 0 });
        let next = ks[(pos as i32 + d).rem_euclid(ks.len() as i32) as usize];
        self.focus = Some(next);
        self.info = Some(next);
        self.wake();
        Some(next)
    }

    /// dive into a node (or page the More node); particles burst outward
    pub fn activate(&mut self, i: usize) -> bool {
        if i == 0 {
            return false;
        } // centre: caller closes the screen
        self.g.dive(i);
        self.parts.retarget(&self.g);
        self.parts.shock(0.0, 0.0, 1, 55.0);
        self.hover = None;
        self.focus = None;
        self.info = None;
        self.wake();
        true
    }

    pub fn toggle_hidden(&mut self) {
        self.g.toggle_hidden();
        self.parts.retarget(&self.g);
        self.wake();
    }

    pub fn reset_layout(&mut self) {
        self.g.reset_layout();
        self.parts.retarget(&self.g);
        self.wake();
    }

    // ---------- chrome text ----------

    pub fn title_line(&self, pal: &Palette) -> Line<'static> {
        let mut s = vec![
            Span::styled(
                " GRAPH ".to_string(),
                Style::new()
                    .fg(Color::Rgb(pal.bg.0, pal.bg.1, pal.bg.2))
                    .bg(Color::Rgb(pal.accent.0, pal.accent.1, pal.accent.2))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
        ];
        let crumbs = self.g.loc.crumbs();
        let n = crumbs.len().min(6);
        for (i, (name, _)) in crumbs[crumbs.len() - n..].iter().enumerate() {
            if i > 0 {
                s.push(Span::styled(" › ".to_string(), col(pal.faint)));
            }
            let last = i == n - 1;
            s.push(Span::styled(
                name.clone(),
                if last {
                    Style::new()
                        .fg(Color::Rgb(pal.accent.0, pal.accent.1, pal.accent.2))
                        .add_modifier(Modifier::BOLD)
                } else {
                    col(pal.dim)
                },
            ));
        }
        if self.g.show_hidden {
            s.push(Span::styled("   hidden shown".to_string(), col(pal.pin)));
        }
        Line::from(s)
    }

    pub fn info_line(&self, pal: &Palette) -> Line<'static> {
        let node = self.info.and_then(|i| self.g.nodes.get(i));
        let mut s: Vec<Span> = match node {
            None => {
                vec![
                    Span::styled(TIP.to_string(), col(pal.dim)),
                    Span::styled("   esc go here · q cancel".to_string(), col(pal.faint)),
                ]
            }
            Some(n) => {
                let what = match n.role {
                    crate::graph::Role::Centre => "click to close and go here",
                    crate::graph::Role::More => "click for next page",
                    crate::graph::Role::Parent => "click to go up",
                    _ => "click to dive in",
                };
                let cnt = if matches!(n.role, crate::graph::Role::Centre | crate::graph::Role::Kid)
                    && !n.arc
                {
                    format!(" · {} items", n.count)
                } else {
                    String::new()
                };
                let mut s = vec![
                    Span::styled(
                        format!(" {}", n.name),
                        Style::new()
                            .fg(Color::Rgb(pal.accent.0, pal.accent.1, pal.accent.2))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("{cnt} · {what}"), col(pal.dim)),
                ];
                if self.g.is_pinned(&n.key()) {
                    s.push(Span::styled(
                        " · pinned (right-click to unpin)".to_string(),
                        col(pal.pin),
                    ));
                }
                s
            }
        };
        s.insert(0, Span::raw(" "));
        Line::from(s)
    }
}

fn col(c: crate::theme::Col) -> Style {
    Style::new().fg(Color::Rgb(c.0, c.1, c.2))
}
