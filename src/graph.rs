//! Graph navigator core: folder bubbles, physics, hit testing, Braille
//! canvas. Pure logic, no terminal code — a direct port of the python app's
//! graphcore (near/far rings, damp 0.8, pins, rim-trimmed edges, labels).

use std::collections::{BTreeMap, HashMap};

use crate::model::{is_archive_name, Loc};
use crate::ops;
use crate::state::State as AppState;
use crate::theme::{mix, Col, Palette};

pub const NEAR: f32 = 64.0;
pub const FAR: f32 = 150.0;
pub const MAX_GRAND: usize = 5;

/// braille bit for (row, col) inside one terminal cell (2 dots wide, 4 tall)
pub const BIT: [u8; 8] = [0x01, 0x08, 0x02, 0x10, 0x04, 0x20, 0x40, 0x80];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Centre,
    Parent,
    Kid,
    Grand,
    More,
}

pub struct GNode {
    pub loc: Loc,
    pub name: String,
    pub role: Role,
    pub par: Option<usize>,
    pub count: u32,
    pub arc: bool,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    /// true when its position was remembered, so rebuilds keep it in place
    pub old: bool,
}

impl GNode {
    pub fn key(&self) -> String {
        if self.role == Role::More {
            return format!("{}\u{0}more", self.loc.display());
        }
        self.loc.display()
    }
    pub fn radius(&self) -> f32 {
        match self.role {
            Role::Centre => 36.0,
            Role::Parent => 16.0,
            Role::More => 13.0,
            Role::Grand => 6.0,
            Role::Kid if self.arc => 18.0,
            Role::Kid => 14.0 + (self.count.min(12) as f32) * 1.6,
        }
    }
}

/// one styled run of a terminal row, straight off the braille canvas
pub struct GSpan {
    pub text: String,
    pub fg: Col,
    pub bold: bool,
    pub bg: Option<Col>,
}

/// a text cell drawn over the dot grid
#[derive(Clone, Copy)]
pub struct OverCell {
    pub fg: Col,
    pub bold: bool,
    pub bg: Option<Col>,
}

/// Braille dot canvas: every terminal cell is 2 dots wide, 4 dots tall.
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    bits: Vec<u8>,
    col: Vec<Option<Col>>,
    pr: Vec<i8>,
    over: Vec<Option<(char, OverCell)>>,
}

impl Canvas {
    pub fn new(w: usize, h: usize) -> Canvas {
        Canvas {
            w,
            h,
            bits: vec![0; w * h],
            col: vec![None; w * h],
            pr: vec![-1; w * h],
            over: vec![None; w * h],
        }
    }

    pub fn plot(&mut self, x: i32, y: i32, c: Col, p: i8) {
        if x < 0 || y < 0 {
            return;
        }
        let (cx, cy) = ((x >> 1) as usize, (y >> 2) as usize);
        if cx >= self.w || cy >= self.h {
            return;
        }
        let i = cy * self.w + cx;
        self.bits[i] |= BIT[((y & 3) << 1 | (x & 1)) as usize];
        if p >= self.pr[i] {
            self.pr[i] = p;
            self.col[i] = Some(c);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, c: Col, p: i8, dash: u32) {
        let (mut x0, mut y0, x1, y1) = (
            x0.round() as i32,
            y0.round() as i32,
            x1.round() as i32,
            y1.round() as i32,
        );
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let (mut err, mut k) = (dx + dy, 0);
        for _ in 0..4000 {
            if dash == 0 || (k / dash).is_multiple_of(2) {
                self.plot(x0, y0, c, p);
            }
            if x0 == x1 && y0 == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x0 += sx;
            }
            if e2 <= dx {
                err += dx;
                y0 += sy;
            }
            k += 1;
        }
    }

    pub fn circle(&mut self, cx: f32, cy: f32, r: i32, c: Col, p: i8, dotted: bool) {
        let (cx, cy) = (cx.round() as i32, cy.round() as i32);
        let (mut x, mut y, mut err, mut k) = (r, 0, 1 - r, 0);
        while x >= y {
            if !dotted || k % 2 == 0 {
                for (ax, ay) in [
                    (x, y),
                    (y, x),
                    (-y, x),
                    (-x, y),
                    (-x, -y),
                    (-y, -x),
                    (y, -x),
                    (x, -y),
                ] {
                    self.plot(cx + ax, cy + ay, c, p);
                }
            }
            y += 1;
            k += 1;
            if err < 0 {
                err += 2 * y + 1;
            } else {
                x -= 1;
                err += 2 * (y - x) + 1;
            }
        }
    }

    pub fn disc(&mut self, cx: f32, cy: f32, r: i32, c: Col, p: i8, every: i32) {
        let (cx, cy) = (cx.round() as i32, cy.round() as i32);
        for yy in -r..=r {
            let half = ((r * r - yy * yy).max(0) as f32).sqrt() as i32;
            for xx in -half..=half {
                if (xx + yy) % every == 0 {
                    self.plot(cx + xx, cy + yy, c, p);
                }
            }
        }
    }

    pub fn put(&mut self, col: i32, row: i32, s: &str, cap: OverCell) {
        for (k, ch) in s.chars().enumerate() {
            let cc = col + k as i32;
            if cc >= 0
                && cc < self.w as i32
                && row >= 0
                && row < self.h as i32
                && self.w * (row as usize) + (cc as usize) < self.over.len()
            {
                self.over[row as usize * self.w + cc as usize] = Some((ch, cap));
            }
        }
    }

    /// styled rows: overlay cells win, then braille, then blank space
    pub fn to_text(&self) -> Vec<Vec<GSpan>> {
        let mut rows = Vec::with_capacity(self.h);
        for cy in 0..self.h {
            let mut spans: Vec<GSpan> = vec![];
            for cx in 0..self.w {
                let i = cy * self.w + cx;
                let (ch, cell) = match self.over[i] {
                    Some((ch, cap)) => (ch, (cap.fg, cap.bold, cap.bg)),
                    None if self.bits[i] != 0 => {
                        let ch = char::from_u32(0x2800 + self.bits[i] as u32).unwrap_or(' ');
                        (ch, (self.col[i].unwrap_or(Col(0, 0, 0)), false, None))
                    }
                    None => (' ', (Col(0, 0, 0), false, None)),
                };
                match spans.last_mut() {
                    Some(s) if s.fg == cell.0 && s.bold == cell.1 && s.bg == cell.2 => {
                        s.text.push(ch)
                    }
                    _ => spans.push(GSpan {
                        text: ch.to_string(),
                        fg: cell.0,
                        bold: cell.1,
                        bg: cell.2,
                    }),
                }
            }
            rows.push(spans);
        }
        rows
    }
}

pub fn count_loc(loc: &Loc) -> u32 {
    if let Some(arc) = &loc.arc {
        return match crate::archive::get_index(arc, None) {
            Ok(i) => i.children(&loc.inner).len() as u32,
            Err(_) => 0,
        };
    }
    ops::list_real(&loc.path)
        .map(|v| v.len().min(1000) as u32)
        .unwrap_or(0)
}

pub fn list_kids(loc: &Loc, show_hidden: bool) -> Vec<(Loc, String, bool)> {
    let mut out = Vec::new();
    if let Some(arc) = &loc.arc {
        if let Ok(idx) = crate::archive::get_index(arc, None) {
            for e in idx.children(&loc.inner) {
                if e.is_dir && (show_hidden || !e.name.starts_with('.')) {
                    out.push((
                        Loc::in_arc(loc.path.clone(), arc.clone(), e.inner.clone()),
                        e.name.clone(),
                        false,
                    ));
                }
            }
        }
    } else if let Ok(entries) = ops::list_real(&loc.path) {
        for e in entries {
            if !show_hidden && e.name.starts_with('.') {
                continue;
            }
            if e.is_dir {
                out.push((Loc::real(e.path.clone().unwrap()), e.name.clone(), false));
            } else if is_archive_name(&e.name) {
                out.push((
                    Loc::in_arc(loc.path.clone(), e.path.clone().unwrap(), ""),
                    e.name.clone(),
                    true,
                ));
            }
        }
    }
    out.sort_by_key(|(l, name, is_arc)| (*is_arc, name.to_lowercase(), l.path.clone()));
    out
}

pub struct Graph {
    /// the app's shared pins/anchors state, owned by main; graph only mutates
    /// pin entries through it
    pub state: *mut AppState,
    pub pal: Palette,
    pub show_hidden: bool,
    pub maxn: u32,
    pub zoom: f32,
    pub page: u32,
    /// remembered positions across dives: key → (x, y)
    pub pos: HashMap<String, (f32, f32)>,
    pub w: usize,
    pub h: usize,
    xs: f32,
    pub sc: f32,
    pub nodes: Vec<GNode>,
    pub loc: Loc,
    /// pin radii snapshot; rebuild() and step() sync it with the state file
    pins_snapshot: BTreeMap<String, f64>,
}

impl Graph {
    /// x-axis stretch factor (set_size computed)
    pub fn xs(&self) -> f32 {
        self.xs
    }
    pub fn new(
        loc: Loc,
        state: &mut AppState,
        pal: Palette,
        show_hidden: bool,
        maxn: u32,
    ) -> Graph {
        let mut g = Graph {
            state,
            pal,
            show_hidden,
            maxn,
            zoom: 1.0,
            page: 0,
            pos: HashMap::new(),
            w: 0,
            h: 0,
            xs: 1.0,
            sc: 0.5,
            nodes: vec![],
            loc,
            pins_snapshot: BTreeMap::new(),
        };
        g.rebuild();
        g
    }

    fn mk(
        &mut self,
        loc: Loc,
        name: String,
        role: Role,
        par: Option<usize>,
        count: u32,
        arc: bool,
    ) -> usize {
        let key = if role == Role::More {
            format!("{}\u{0}more", loc.display())
        } else {
            loc.display()
        };
        let (x, y, old) = match self.pos.get(&key) {
            Some(&(x, y)) => (x, y, true),
            None => (rand(-30.0, 30.0), rand(-30.0, 30.0), false),
        };
        self.nodes.push(GNode {
            loc,
            name,
            role,
            par,
            count,
            arc,
            x,
            y,
            vx: 0.0,
            vy: 0.0,
            old,
        });
        self.nodes.len() - 1
    }

    pub fn rebuild(&mut self) {
        self.pins_snapshot = unsafe { (*self.state).pins_ref().clone() };
        self.nodes = Vec::new();
        let loc = self.loc.clone();
        let c = self.mk(
            loc.clone(),
            loc.folder_name(),
            Role::Centre,
            None,
            count_loc(&loc),
            false,
        );
        self.nodes[c].x = 0.0;
        self.nodes[c].y = 0.0;
        self.nodes[c].old = false;
        if let Some(par) = loc.parent() {
            let p = self.mk(
                par.clone(),
                par.folder_name(),
                Role::Parent,
                Some(c),
                count_loc(&par),
                false,
            );
            if !self.nodes[p].old {
                self.nodes[p].x = -150.0;
                self.nodes[p].y = -110.0;
            }
        }
        let kids = list_kids(&loc, self.show_hidden);
        let pinned: Vec<_> = kids
            .iter()
            .filter(|k| self.pins_snapshot.contains_key(&k.0.display()))
            .cloned()
            .collect();
        let n_rest = kids.len() - pinned.len();
        let rest: Vec<_> = kids
            .into_iter()
            .filter(|k| !self.pins_snapshot.contains_key(&k.0.display()))
            .collect();
        let slots = 4usize.max((self.maxn as usize).saturating_sub(pinned.len()));
        let pages = ((n_rest as f32 / slots as f32).ceil() as u32).max(1);
        self.page %= pages;
        let start = self.page as usize * slots;
        let shown: Vec<_> = pinned
            .into_iter()
            .chain(rest.into_iter().skip(start).take(slots))
            .collect();
        let n_shown = shown.len();
        for (i, (kl, name, is_arc)) in shown.into_iter().enumerate() {
            let count = if is_arc { 0 } else { count_loc(&kl) };
            let k = self.mk(kl.clone(), name.clone(), Role::Kid, Some(c), count, is_arc);
            if !self.nodes[k].old {
                let t = i as f32 / 1.max(n_shown) as f32 * std::f32::consts::TAU;
                self.nodes[k].x = t.cos() * 120.0;
                self.nodes[k].y = t.sin() * 120.0;
            }
            if !is_arc {
                let grads = list_kids(&kl, self.show_hidden);
                for (gl, gname, _) in grads.into_iter().take(MAX_GRAND) {
                    let g = self.mk(gl, gname, Role::Grand, Some(k), 0, false);
                    if !self.nodes[g].old {
                        self.nodes[g].x = self.nodes[k].x + rand(-20.0, 20.0);
                        self.nodes[g].y = self.nodes[k].y + rand(-20.0, 20.0);
                    }
                }
            }
        }
        if n_rest > slots {
            let m = self.mk(
                loc.clone(),
                format!("+{} more", n_rest - slots),
                Role::More,
                Some(c),
                0,
                false,
            );
            if !self.nodes[m].old {
                self.nodes[m].x = 0.0;
                self.nodes[m].y = 140.0;
            }
        }
    }

    pub fn center(&self) -> &GNode {
        &self.nodes[0]
    }

    pub fn dive(&mut self, idx: usize) {
        let (role, dx, dy, loc) = (
            self.nodes[idx].role,
            self.nodes[idx].x,
            self.nodes[idx].y,
            self.nodes[idx].loc.clone(),
        );
        if role == Role::Centre {
            return;
        }
        if role == Role::More {
            self.pos.clear();
            for n in &self.nodes {
                if matches!(n.role, Role::Centre | Role::Parent | Role::Kid) {
                    self.pos.insert(n.key(), (n.x, n.y));
                }
            }
            self.page += 1;
            self.rebuild();
            return;
        }
        self.pos = self
            .nodes
            .iter()
            .map(|n| (n.key(), (n.x - dx, n.y - dy)))
            .collect();
        self.loc = loc;
        self.page = 0;
        self.rebuild();
    }

    pub fn up(&mut self) -> bool {
        if let Some(i) = self.nodes.iter().position(|n| n.role == Role::Parent) {
            self.dive(i);
            true
        } else {
            false
        }
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        let keep = self.pos.clone();
        self.pos = keep; // graph layout is already remembered via node keys
        self.rebuild();
    }

    pub fn reset_layout(&mut self) {
        self.pos = HashMap::new();
        self.rebuild();
    }

    pub fn set_size(&mut self, w: usize, h: usize) {
        self.w = w;
        self.h = h;
        let (wd, hd) = (w as f32 * 2.0, h as f32 * 4.0);
        self.xs = 2.0f32.min(1.0f32.max(wd / 1.0f32.max(hd) * 0.65));
        let half = 10.0f32.max((hd / 2.0).min(wd / (2.0 * self.xs)));
        self.sc = 0.05f32.max((half - 10.0) / 215.0);
    }

    pub fn to_dots(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.w as f32 + x * self.sc * self.xs * self.zoom,
            self.h as f32 * 2.0 + y * self.sc * self.zoom,
        )
    }

    pub fn to_world(&self, dx: f32, dy: f32) -> (f32, f32) {
        (
            (dx - self.w as f32) / (self.sc * self.xs * self.zoom),
            (dy - self.h as f32 * 2.0) / (self.sc * self.zoom),
        )
    }

    pub fn rdots(&self, n: &GNode) -> i32 {
        let min = if n.role == Role::Grand { 2 } else { 3 };
        min.max((n.radius() * self.sc * self.zoom) as i32)
    }

    pub fn node_at(&self, dx: f32, dy: f32) -> Option<usize> {
        for (i, n) in self.nodes.iter().enumerate().rev() {
            let (sx, sy) = self.to_dots(n.x, n.y);
            if (sx - dx).hypot(sy - dy) <= self.rdots(n).max(4) as f32 + 3.0 {
                return Some(i);
            }
        }
        None
    }

    fn target(&self, a: &GNode) -> f32 {
        match self.pins_snapshot.get(&a.key()) {
            Some(r) => *r as f32,
            None => FAR + a.count.min(10) as f32 * 5.0,
        }
    }

    pub fn step(&mut self, drag: Option<usize>) -> f32 {
        self.pins_snapshot = unsafe { (*self.state).pins_ref().clone() };
        let xl = (self.w.saturating_sub(8) as f32) / (self.sc * self.xs * self.zoom);
        let yl = ((self.h * 2).saturating_sub(8) as f32) / (self.sc * self.zoom);
        let mut energy = 0.0f32;
        for ai in 0..self.nodes.len() {
            if self.nodes[ai].role == Role::Centre || drag == Some(ai) {
                continue;
            }
            let mut fx = 0.0f32;
            let mut fy = 0.0f32;
            for bi in 0..self.nodes.len() {
                if bi == ai {
                    continue;
                }
                let dx = self.nodes[ai].x - self.nodes[bi].x;
                let dy = self.nodes[ai].y - self.nodes[bi].y;
                let d2 = dx * dx + dy * dy + 0.01;
                let d = d2.sqrt();
                let big_r = if self.nodes[ai].role == Role::Grand {
                    40.0
                } else {
                    90.0
                };
                if d < big_r * 1.6 {
                    let gg = if self.nodes[ai].role == Role::Grand
                        || self.nodes[bi].role == Role::Grand
                    {
                        260.0
                    } else {
                        1400.0
                    };
                    let f = gg / d2 * (1.0 - d / (big_r * 1.6));
                    fx += dx / d * f;
                    fy += dy / d * f;
                }
            }
            let (ax, ay) = (self.nodes[ai].x, self.nodes[ai].y);
            match self.nodes[ai].role {
                Role::Kid => {
                    let d = ax.hypot(ay).max(0.001);
                    let t = self.target(&self.nodes[ai]);
                    fx -= ax / d * (d - t) * 0.06;
                    fy -= ay / d * (d - t) * 0.06;
                }
                Role::More => {
                    let d = ax.hypot(ay).max(0.001);
                    fx -= ax / d * (d - FAR) * 0.06;
                    fy -= ay / d * (d - FAR) * 0.06;
                }
                Role::Parent => {
                    fx += (-150.0 - ax) * 0.03;
                    fy += (-110.0 - ay) * 0.03;
                }
                Role::Grand => {
                    let pi = self.nodes[ai].par.unwrap_or(0);
                    let (dx, dy) = (ax - self.nodes[pi].x, ay - self.nodes[pi].y);
                    let d = dx.hypot(dy).max(0.001);
                    fx -= dx / d * (d - 46.0) * 0.08;
                    fy -= dy / d * (d - 46.0) * 0.08;
                }
                Role::Centre => {}
            }
            if ax.abs() > xl {
                fx -= (ax.abs() - xl).copysign(ax) * 0.2;
            }
            if ay.abs() > yl {
                fy -= (ay.abs() - yl).copysign(ay) * 0.2;
            }
            let a = &mut self.nodes[ai];
            a.vx = (a.vx + fx) * 0.8;
            a.vy = (a.vy + fy) * 0.8;
            if a.vx.abs() < 0.03 && a.vy.abs() < 0.03 {
                a.vx = 0.0;
                a.vy = 0.0;
                continue;
            }
            a.x += a.vx;
            a.y += a.vy;
            energy += a.vx.abs() + a.vy.abs();
        }
        energy
    }

    pub fn pin(&mut self, idx: usize) {
        if self.nodes[idx].role == Role::Kid {
            let n = &self.nodes[idx];
            let radius = NEAR.max(n.x.hypot(n.y)) as f64;
            let key = n.key();
            unsafe { (*self.state).set_pin(&key, radius) };
            self.pins_snapshot.insert(key, radius);
        }
    }

    pub fn unpin(&mut self, idx: usize) -> bool {
        let key = self.nodes[idx].key();
        let dropped = unsafe { (*self.state).drop_pin(&key) };
        self.pins_snapshot.remove(&key);
        dropped
    }

    pub fn is_pinned(&self, key: &str) -> bool {
        self.pins_snapshot.contains_key(key)
    }

    /// label text + style key, same roles as python's label()
    pub fn label(&self, n: &GNode) -> (String, &'static str) {
        let name = n.name.chars().take(20).collect::<String>();
        let name = if n.name.chars().count() > 20 {
            format!("{}\u{2026}", n.name.chars().take(19).collect::<String>())
        } else {
            name
        };
        match n.role {
            Role::Parent => (format!("\u{25b2} {name}"), "lp"),
            Role::Centre => (name, "lc"),
            Role::More => (name, "lm"),
            Role::Kid if self.is_pinned(&n.key()) => (format!("\u{25c6} {name}"), "lpin"),
            Role::Kid if n.arc => (name, "larc"),
            Role::Grand => (name, "lg"),
            _ => (name, "lk"),
        }
    }

    /// the frame's canvas; the ui layer blits it and layers particles under it
    pub fn render(&self, hover: Option<usize>) -> Canvas {
        let (w, h) = (self.w, self.h);
        let mut cv = Canvas::new(w, h);
        if w < 8 || h < 4 || self.nodes.is_empty() {
            return cv;
        }
        let p = &self.pal;
        let near = |i: usize| -> bool {
            match hover {
                Some(hv) => {
                    hv == i
                        || self
                            .nodes
                            .get(i)
                            .map(|n| n.par == Some(hv))
                            .unwrap_or(false)
                        || self
                            .nodes
                            .get(hv)
                            .map(|n| n.par == Some(i))
                            .unwrap_or(false)
                }
                None => false,
            }
        };
        // faint dot lattice, the graph's ground
        let ground = mix(p.bg, p.faint, 0.16);
        for y in (2..h * 4).step_by(8) {
            for x in (3..w * 2).step_by(8) {
                cv.plot(x as i32, y as i32, ground, 0);
            }
        }
        // guide rings with labels
        for (ring, lab) in [(NEAR, "near"), (FAR, "far")] {
            for deg in (0..360).step_by(4) {
                if (deg / 4) % 2 == 0 {
                    let t = (deg as f32).to_radians();
                    let (x, y) = self.to_dots(t.cos() * ring, t.sin() * ring);
                    cv.plot(
                        x.round() as i32,
                        y.round() as i32,
                        mix(p.bg, p.faint, 0.45),
                        0,
                    );
                }
            }
            let t = (-62.0f32).to_radians();
            let (x, y) = self.to_dots(t.cos() * ring, t.sin() * ring);
            cv.put(
                (x as i32) >> 1,
                (y as i32) >> 2,
                lab,
                OverCell {
                    fg: p.faint,
                    bold: false,
                    bg: None,
                },
            );
        }
        // edges trimmed to bubble rims
        for (i, n) in self.nodes.iter().enumerate() {
            let Some(pi) = n.par else { continue };
            let on = hover
                .map(|hv| hv == i || n.par == Some(hv))
                .unwrap_or(false);
            let (mut x0, mut y0) = self.to_dots(self.nodes[pi].x, self.nodes[pi].y);
            let (mut x1, mut y1) = self.to_dots(n.x, n.y);
            let (dx, dy) = (x1 - x0, y1 - y0);
            let l = (dx * dx + dy * dy).sqrt();
            if l > 1.0 {
                let (ux, uy) = (dx / l, dy / l);
                x0 += ux * self.rdots(&self.nodes[pi]) as f32;
                y0 += uy * self.rdots(&self.nodes[pi]) as f32;
                x1 -= ux * self.rdots(n) as f32;
                y1 -= uy * self.rdots(n) as f32;
            }
            let edge = mix(p.bg, p.faint, 0.8);
            cv.line(
                x0,
                y0,
                x1,
                y1,
                if on { p.accent } else { edge },
                if on { 6 } else { 1 },
                if n.role == Role::Parent { 3 } else { 0 },
            );
        }
        // tinted fills under clean outlines, roles drawn back to front
        let bcol = |role: Role, arc: bool| -> Col {
            match role {
                Role::Centre => p.accent,
                Role::More => p.accent2,
                Role::Parent => p.dim,
                _ if arc => p.kind(1),
                _ => p.kind(0),
            }
        };
        let order = |role: Role| -> u8 {
            match role {
                Role::Grand => 0,
                Role::Parent => 1,
                Role::More => 2,
                Role::Kid => 3,
                Role::Centre => 4,
            }
        };
        let mut idxs: Vec<usize> = (0..self.nodes.len()).collect();
        idxs.sort_by_key(|&i| order(self.nodes[i].role));
        for &i in &idxs {
            let n = &self.nodes[i];
            let (sx, sy) = self.to_dots(n.x, n.y);
            let r = self.rdots(n);
            let dim = hover.is_some() && !near(i) && n.role != Role::Centre;
            if n.role == Role::Grand {
                cv.disc(sx, sy, 1.max(r - 1), p.faint, 4, 1);
                continue;
            }
            let mut raw = bcol(n.role, n.arc);
            let mut tint = match n.role {
                Role::Centre => 0.20,
                Role::Parent => 0.14,
                Role::More => 0.18,
                _ => 0.20,
            };
            if dim {
                raw = p.faint;
                tint = 0.10;
            } else if Some(i) == hover {
                raw = p.accent;
                tint = 0.42;
            }
            let outline = if dim { p.faint } else { bcol(n.role, n.arc) };
            cv.disc(sx, sy, 1.max(r - 1), mix(p.bg, raw, tint), 2, 2);
            if n.role == Role::Centre {
                cv.disc(sx, sy, 2.max(r / 2), outline, 3, 1);
                cv.circle(sx, sy, r + 2, outline, 3, true);
            }
            cv.circle(sx, sy, r, outline, 5, false);
            if n.role == Role::Kid && self.is_pinned(&n.key()) {
                cv.circle(sx, sy, r + 3, p.pin, 7, true);
            }
            if Some(i) == hover {
                cv.circle(sx, sy, r + 2, p.accent, 8, false);
            }
        }
        // labels with collision avoidance; centre and hovered win
        let mut placed: Vec<(usize, i32, i32)> = vec![];
        let putl = |cv: &mut Canvas, i: usize, force: bool, placed: &mut Vec<(usize, i32, i32)>| {
            let n = &self.nodes[i];
            let (text, sk) = self.label(n);
            let cap = self.style_of(sk, hover == Some(i));
            let (sx, sy) = self.to_dots(n.x, n.y);
            let tlen = text.chars().count() as i32;
            let c0 = (sx as i32) / 2 - tlen / 2;
            let row0 = ((sy + self.rdots(n) as f32 + 3.0) as i32 / 4) + 1;
            for dr in 0..3 {
                let row = row0 + dr;
                if row < 0 || row >= h as i32 {
                    break;
                }
                let c0m = c0.max(0);
                let clash = placed
                    .iter()
                    .any(|(rr, a, b)| *rr == row as usize && !(c0 + tlen < *a || c0m > *b));
                if !clash {
                    cv.put(c0m, row, &text, cap);
                    placed.push((row as usize, c0 - 1, c0 + tlen + 1));
                    return;
                }
            }
            if force && row0 >= 0 && row0 < h as i32 {
                cv.put(c0.max(0), row0, &text, cap);
            }
        };
        putl(&mut cv, 0, true, &mut placed);
        if let Some(hv) = hover {
            putl(&mut cv, hv, true, &mut placed);
        }
        for i in 0..self.nodes.len() {
            if matches!(self.nodes[i].role, Role::Kid | Role::More | Role::Parent)
                && Some(i) != hover
            {
                putl(&mut cv, i, false, &mut placed);
            }
        }
        // centre count sits in the middle of the centre bubble
        let (sx, sy) = self.to_dots(self.nodes[0].x, self.nodes[0].y);
        let num = self.nodes[0].count.to_string();
        cv.put(
            (sx as i32) / 2 - num.len().div_ceil(2) as i32,
            (sy as i32) / 4,
            &num,
            OverCell {
                fg: p.text,
                bold: true,
                bg: None,
            },
        );
        cv
    }

    fn style_of(&self, sk: &str, hovered: bool) -> OverCell {
        let p = &self.pal;
        if hovered {
            return OverCell {
                fg: p.bg,
                bold: true,
                bg: Some(p.accent),
            };
        }
        let fg = match sk {
            "lpin" => p.pin,
            "lp" => p.dim,
            "lc" => p.text,
            "lm" => p.accent2,
            "larc" => p.kind(1),
            "lg" => p.dim,
            _ => p.text,
        };
        OverCell {
            fg,
            bold: sk == "lc" || sk == "lpin",
            bg: None,
        }
    }
}

/// xorshift-based uniform in [lo, hi), no rng dependency
pub fn rand(lo: f32, hi: f32) -> f32 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEED: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
    let s = SEED.fetch_add(0x94d0_49bb_1331_11eb, Ordering::Relaxed);
    let mut z = s ^ (s >> 30);
    z = z.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = z.wrapping_mul(0x94d0_49bb_1331_11eb);
    let u = (z >> 11) as f64 / (1u64 << 53) as f64;
    u as f32 * (hi - lo) + lo
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::State;

    #[test]
    fn dive_and_up_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("vg-graph-{}", std::process::id()));
        let sub = tmp.join("sub");
        let _ = std::fs::create_dir_all(&sub);
        std::env::set_var("XDG_STATE_HOME", &tmp);
        let mut st = State::load();
        let mut g = Graph::new(
            Loc::real(tmp.clone()),
            &mut st,
            Palette::default(),
            false,
            28,
        );
        assert_eq!(g.nodes[0].role, Role::Centre);
        let ki = g
            .nodes
            .iter()
            .position(|n| n.role == Role::Kid && n.name == "sub")
            .expect("kid");
        g.dive(ki);
        assert_eq!(g.loc.path.file_name().unwrap().to_string_lossy(), "sub");
        assert!(g.up());
        assert_eq!(g.loc.path, tmp);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn size_and_canvas() {
        let tmp = std::env::temp_dir().join(format!("vg-graph2-{}", std::process::id()));
        let sub = tmp.join("sub");
        let _ = std::fs::create_dir_all(&sub);
        std::env::set_var("XDG_STATE_HOME", &tmp);
        let mut st = State::load();
        let mut g = Graph::new(
            Loc::real(tmp.clone()),
            &mut st,
            Palette::default(),
            false,
            28,
        );
        g.set_size(60, 20);
        let (sx, sy) = g.to_dots(0.0, 0.0);
        assert!(g.node_at(sx.round(), sy.round()).is_some());
        assert!(g.step(None) >= 0.0);
        let c = g.render(Some(0));
        let rows = c.to_text();
        assert_eq!(rows.len(), 20);
        assert_eq!(
            rows[0]
                .iter()
                .map(|s| s.text.chars().count())
                .sum::<usize>(),
            60
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
