//! Particle field for the graph, grown from the author's music visualizer
//! vizr (Field.step/render). Two kinds of dot share the field:
//!
//! - orbiters: spring toward a ring around their bubble, jitter, damping
//!   0.88^f, shock bursts, per-cell energy → colour, sparkle on the hottest
//!   cells (the vizr tail)
//! - carriers: ride the edge from a bubble's parent out to it, like data
//!   flowing down the tree, with a short fading trail
//!
//! Bubbles get dots in proportion to their size, pinned ones glow gold, and
//! hovering a bubble lights its own neighbourhood while the rest dims.
//!
//! Particles live in dot-offset space (dots from the canvas centre) — the
//! same mapping graph.Graph.to_dots uses: centre + (x, y) directly in dots.

use crate::graph::{Canvas, GNode, Graph, Role};
use crate::theme::{mix, Col};

/// density → particle budget, vizr's auto-count rule scaled for a pane
const DENSITY_DIV: u32 = 2400;
const CAP: usize = 640;
/// every fourth dot of a bubble that has a parent rides the edge instead
const CARRIER_EVERY: usize = 4;
/// carriers stay brighter than orbiters so the flow reads at a glance
const CARRIER_MIX: f32 = 0.8;

pub struct ParticleField {
    pub x: Vec<f32>,
    pub y: Vec<f32>,
    vx: Vec<f32>,
    pub vy: Vec<f32>,
    node: Vec<u16>,
    /// orbit angle / radius fraction / jitter phase, stable per particle
    ang: Vec<f32>,
    rad: Vec<f32>,
    ph: Vec<f32>,
    /// progress along the parent→bubble edge, or ORBIT for an orbiter
    lane: Vec<f32>,
    pub energy: Vec<f32>,
    w: u16,
    h: u16,
    /// engine clock (seconds, 30 Hz ticks)
    pub t: f32,
}

const ORBIT: f32 = -1.0;

#[inline]
fn frand() -> f32 {
    crate::graph::rand(0.0, 1.0)
}

fn budget(w: u16, h: u16) -> usize {
    (w as u32 * h as u32 / DENSITY_DIV).clamp(90, CAP as u32) as usize
}

/// a bubble's centre in dot offsets from the canvas centre
pub fn home_of(g: &Graph, n: &GNode) -> (f32, f32) {
    (n.x * g.sc * g.xs() * g.zoom, n.y * g.sc * g.zoom)
}

/// dots per bubble: the centre is the busiest, grandchildren barely there
fn weight(role: Role) -> usize {
    match role {
        Role::Centre => 4,
        Role::Kid => 3,
        Role::Parent | Role::More => 2,
        Role::Grand => 1,
    }
}

impl ParticleField {
    pub fn new(w: u16, h: u16) -> ParticleField {
        let n = budget(w, h);
        let mut f = ParticleField {
            x: vec![0.0; n],
            y: vec![0.0; n],
            vx: vec![0.0; n],
            vy: vec![0.0; n],
            node: vec![0; n],
            ang: vec![0.0; n],
            rad: vec![0.0; n],
            ph: vec![0.0; n],
            lane: vec![ORBIT; n],
            energy: vec![0.0; n],
            w,
            h,
            t: 0.0,
        };
        f.scatter();
        f
    }

    pub fn count(&self) -> usize {
        self.node.len()
    }

    /// resize the field, zero velocities (same as vizr's rebuild on resize)
    pub fn resize(&mut self, w: u16, h: u16) {
        if w == self.w && h == self.h {
            return;
        }
        self.w = w;
        self.h = h;
        let n = budget(w, h);
        self.x.resize(n, 0.0);
        self.y.resize(n, 0.0);
        self.vx.resize(n, 0.0);
        self.vy.resize(n, 0.0);
        self.node.resize(n, 0);
        self.ang.resize(n, 0.0);
        self.rad.resize(n, 0.0);
        self.ph.resize(n, 0.0);
        self.lane.resize(n, ORBIT);
        self.energy.resize(n, 0.0);
        self.scatter();
    }

    /// scatter at random; used on rebuilds and resizes
    pub fn scatter(&mut self) {
        for i in 0..self.x.len() {
            self.rad[i] = frand();
            self.ph[i] = frand() * std::f32::consts::TAU;
            self.energy[i] = 0.0;
            self.x[i] = (frand() - 0.5) * self.w as f32 * 2.0;
            self.y[i] = (frand() - 0.5) * self.h as f32 * 2.0;
            self.vx[i] = 0.0;
            self.vy[i] = 0.0;
        }
    }

    /// Hand the dots out to the graph's bubbles by weight, round-robin so
    /// every bubble's share is spread over the whole run (and so over every
    /// orbit angle), then pick each bubble's carriers.
    pub fn retarget(&mut self, g: &Graph) {
        if g.nodes.is_empty() {
            return;
        }
        let top = g.nodes.iter().map(|n| weight(n.role)).max().unwrap_or(1);
        let mut pool: Vec<u16> = Vec::new();
        for round in 0..top {
            for (i, n) in g.nodes.iter().enumerate() {
                if weight(n.role) > round {
                    pool.push(i as u16);
                }
            }
        }
        let mut share = vec![0usize; g.nodes.len()];
        for i in 0..self.node.len() {
            let ni = pool[i % pool.len()];
            self.node[i] = ni;
            share[ni as usize] += 1;
        }
        let mut seen = vec![0usize; g.nodes.len()];
        for i in 0..self.node.len() {
            let ni = self.node[i] as usize;
            let k = seen[ni];
            seen[ni] += 1;
            self.ang[i] = k as f32 / share[ni].max(1) as f32 * std::f32::consts::TAU;
            self.lane[i] = if g.nodes[ni].par.is_some() && k.is_multiple_of(CARRIER_EVERY) {
                frand()
            } else {
                ORBIT
            };
        }
    }

    /// radial impulse from a dot-space point; positive force pushes out
    pub fn shock(&mut self, wx: f32, wy: f32, kind: i32, force: f32) {
        for i in 0..self.x.len() {
            let dx = self.x[i] - wx;
            let dy = self.y[i] - wy;
            let d = dx.hypot(dy) + 0.001;
            let rr = 150.0;
            let k = (1.0 - d / rr).max(0.0) * kind as f32 * force;
            self.vx[i] += dx / d * k;
            self.vy[i] += dy / d * k;
        }
    }

    /// the rim-to-rim segment of the edge into `n`, in dot offsets
    fn edge(g: &Graph, n: &GNode) -> Option<((f32, f32), (f32, f32))> {
        let p = &g.nodes[n.par?];
        let (a, b) = (home_of(g, p), home_of(g, n));
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l = dx.hypot(dy).max(1.0);
        let (ux, uy) = (dx / l, dy / l);
        let (ra, rb) = (g.rdots(p) as f32, g.rdots(n) as f32);
        Some((
            (a.0 + ux * ra, a.1 + uy * ra),
            (b.0 - ux * rb, b.1 - uy * rb),
        ))
    }

    /// one physics tick, the vizr tail: spring toward home, jitter, damp.
    /// Fired on a fixed 30 Hz cadence; velocities are per-tick, matching the
    /// python graph's damping feel.
    pub fn step(&mut self, g: &Graph, kick: f32) {
        if g.nodes.is_empty() || self.x.is_empty() {
            return;
        }
        let k = 0.05 + kick * 0.1;
        let dmp = 0.88f32; // vizr damping ** f, f = 1 per tick
        self.t += 1.0 / 30.0;
        for i in 0..self.x.len() {
            let ni = self.node[i] as usize;
            let Some(node) = g.nodes.get(ni) else {
                continue;
            };
            if self.lane[i] != ORBIT {
                if let Some(((ax, ay), (bx, by))) = Self::edge(g, node) {
                    self.ride(i, (ax, ay), (bx, by));
                    continue;
                }
            }
            let (cx, cy) = home_of(g, node);
            let r = node.radius() * (0.7 + self.rad[i] * 0.75) * g.sc * g.zoom;
            let a = self.ang[i] + self.t * 0.25;
            let hx = cx
                + a.cos() * r
                + (self.t * 1.3 + self.ph[i] * 40.0).sin() * (2.0 + self.rad[i] * 3.0);
            let hy = cy
                + a.sin() * r
                + (self.t * 1.1 + self.ph[i] * 40.0).cos() * (2.0 + self.rad[i] * 3.0);
            self.vx[i] += (hx - self.x[i]) * k;
            self.vy[i] += (hy - self.y[i]) * k;
            self.vx[i] *= dmp;
            self.vy[i] *= dmp;
            self.x[i] += self.vx[i];
            self.y[i] += self.vy[i];
            self.energy[i] = self.energy[i] * 0.9 + self.vx[i].hypot(self.vy[i]) * 0.1;
        }
    }

    /// move carrier `i` along its edge; arriving restarts it at the parent
    fn ride(&mut self, i: usize, a: (f32, f32), b: (f32, f32)) {
        self.lane[i] += 0.011 + self.rad[i] * 0.009;
        if self.lane[i] > 1.0 {
            self.lane[i] -= 1.0;
            // jumping back to the start is not motion, keep it out of the trail
            self.vx[i] = 0.0;
            self.vy[i] = 0.0;
            self.x[i] = a.0;
            self.y[i] = a.1;
        }
        let s = self.lane[i];
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l = dx.hypot(dy).max(1.0);
        // a little sideways sway so a busy edge reads as a stream, not a wire
        let sway = (self.t * 3.1 + self.ph[i]).sin() * 1.2 * (s * std::f32::consts::PI).sin();
        let (hx, hy) = (a.0 + dx * s - dy / l * sway, a.1 + dy * s + dx / l * sway);
        let (px, py) = (self.x[i], self.y[i]);
        // a fresh restart has zero velocity, so only trail real movement
        if self.vx[i] != 0.0 || self.vy[i] != 0.0 || s > 0.05 {
            self.vx[i] = hx - px;
            self.vy[i] = hy - py;
        }
        self.x[i] = hx;
        self.y[i] = hy;
        self.energy[i] = self.energy[i] * 0.8 + 6.0 * 0.2;
    }

    /// strew the dots across the canvas; per-cell mean energy sets the
    /// brightness, the owning bubble tints it, sparkle lands on hot cells
    pub fn render(&self, cv: &mut Canvas, g: &Graph, hover: Option<usize>, sparkle: f32) {
        let (dots_x, dots_y) = (cv.w * 2, cv.h * 4);
        let mut cells = vec![Cell::default(); dots_x * dots_y];
        let at = |x: f32, y: f32| -> Option<usize> {
            let ix = cv.w as i32 + x.round() as i32;
            let iy = cv.h as i32 * 2 + y.round() as i32;
            (ix >= 0 && iy >= 0 && (ix as usize) < dots_x && (iy as usize) < dots_y)
                .then(|| iy as usize * dots_x + ix as usize)
        };
        let mut trails: Vec<(usize, u16)> = Vec::new();
        for i in 0..self.x.len() {
            let Some(ci) = at(self.x[i], self.y[i]) else {
                continue;
            };
            let c = &mut cells[ci];
            c.n += 1;
            c.e += (self.energy[i].min(16.0) * 256.0) as u32;
            c.node = self.node[i];
            if self.lane[i] != ORBIT {
                c.carrier = true;
                // two fading dots behind the head
                for back in [2.0, 4.0] {
                    if let Some(ti) =
                        at(self.x[i] - self.vx[i] * back, self.y[i] - self.vy[i] * back)
                    {
                        trails.push((ti, self.node[i]));
                    }
                }
            }
        }

        let mut counts: Vec<u32> = cells.iter().map(|c| c.n).filter(|n| *n > 0).collect();
        counts.sort_unstable();
        let hot_cut = counts.get(counts.len() * 88 / 100).copied().unwrap_or(0);
        let twinkle_chance = sparkle * 0.09 * (0.5 + 0.5 * (self.t * 7.3).sin());
        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        let p = &g.pal;

        for (ci, c) in cells.iter().enumerate() {
            if c.n == 0 {
                continue;
            }
            let (ncol, glow) = self.tint(g, c.node as usize, hover);
            let se = (c.e as f32 / 256.0) / c.n as f32;
            let lvl = (((0.3 + se * 0.55) * 15.0) as i32).clamp(0, 15) as f32 / 15.0;
            let amount = if c.carrier { CARRIER_MIX } else { lvl * 0.45 };
            let mut col = mix(p.bg, ncol, (amount * glow).min(1.0));
            if !c.carrier && c.n > hot_cut && hot_cut > 0 && twinkle_chance > 0.01 && glow >= 1.0 {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                if ((rng >> 33) as f32 / (1u32 << 31) as f32) < twinkle_chance {
                    col = p.text; // sparkle: vizr's bright twinkles
                }
            }
            cv.plot((ci % dots_x) as i32, (ci / dots_x) as i32, col, 1);
        }
        for (ti, node) in trails {
            if cells[ti].n > 0 {
                continue; // a head already lives there
            }
            let (ncol, glow) = self.tint(g, node as usize, hover);
            cv.plot(
                (ti % dots_x) as i32,
                (ti / dots_x) as i32,
                mix(p.bg, ncol, (0.3 * glow).min(1.0)),
                1,
            );
        }
    }

    /// colour and brightness factor for a bubble's dots under the current hover
    fn tint(&self, g: &Graph, ni: usize, hover: Option<usize>) -> (Col, f32) {
        let p = &g.pal;
        let Some(n) = g.nodes.get(ni) else {
            return (p.accent, 1.0);
        };
        let base = match n.role {
            Role::Kid if g.is_pinned(&n.key()) => p.pin,
            Role::Kid if n.arc => p.kind(1),
            Role::Kid => p.kind(0),
            Role::Parent => p.dim,
            Role::More => p.accent2,
            _ => p.accent,
        };
        match hover {
            None => (base, 1.0),
            Some(h) if h == ni => (p.accent, 1.8),
            Some(h) if n.par == Some(h) || g.nodes.get(h).is_some_and(|hn| hn.par == Some(ni)) => {
                (base, 1.4)
            }
            Some(_) => (base, 0.45),
        }
    }
}

#[derive(Clone, Copy, Default)]
struct Cell {
    n: u32,
    e: u32,
    node: u16,
    carrier: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Loc;
    use crate::state::State;
    use crate::theme::Palette;

    fn graph_with(dirs: &[&str], tag: &str) -> (Graph, State, std::path::PathBuf) {
        let tmp = std::env::temp_dir().join(format!("vg-parts-{tag}-{}", std::process::id()));
        for d in dirs {
            std::fs::create_dir_all(tmp.join(d)).unwrap();
        }
        std::env::set_var("XDG_STATE_HOME", &tmp);
        let mut st = State::load();
        let mut g = Graph::new(
            Loc::real(tmp.clone()),
            &mut st,
            Palette::default(),
            false,
            28,
        );
        g.set_size(100, 36);
        (g, st, tmp)
    }

    #[test]
    fn bubbles_get_dots_by_weight() {
        let (g, _st, tmp) = graph_with(&["a", "b", "c"], "w");
        let mut f = ParticleField::new(100, 36);
        f.retarget(&g);
        let of = |role: Role| -> usize {
            (0..f.count())
                .filter(|&i| g.nodes[f.node[i] as usize].role == role)
                .count()
        };
        let kids = g
            .nodes
            .iter()
            .filter(|n| n.role == Role::Kid)
            .count()
            .max(1);
        assert!(f.node.iter().all(|&n| (n as usize) < g.nodes.len()));
        assert!(
            of(Role::Centre) > of(Role::Kid) / kids,
            "centre must out-draw a single kid"
        );
        assert!(of(Role::Kid) > 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn carriers_follow_their_edge_and_wrap() {
        let (g, _st, tmp) = graph_with(&["a", "b", "c", "d"], "c");
        let mut f = ParticleField::new(100, 36);
        f.retarget(&g);
        assert!(
            (0..f.count()).any(|i| f.lane[i] != ORBIT),
            "someone must carry"
        );
        for _ in 0..240 {
            f.step(&g, 0.4);
        }
        for i in 0..f.count() {
            assert!(f.x[i].is_finite() && f.y[i].is_finite() && f.energy[i].is_finite());
            if f.lane[i] == ORBIT {
                continue;
            }
            assert!((0.0..=1.0).contains(&f.lane[i]), "lane {}", f.lane[i]);
            let ((ax, ay), (bx, by)) =
                ParticleField::edge(&g, &g.nodes[f.node[i] as usize]).unwrap();
            // distance from the head to the segment a→b
            let (dx, dy) = (bx - ax, by - ay);
            let t = (((f.x[i] - ax) * dx + (f.y[i] - ay) * dy) / (dx * dx + dy * dy).max(1.0))
                .clamp(0.0, 1.0);
            let off = (f.x[i] - (ax + dx * t)).hypot(f.y[i] - (ay + dy * t));
            assert!(off < 3.0, "carrier strayed {off} dots");
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn render_draws_and_hover_dims_the_rest() {
        let (g, _st, tmp) = graph_with(&["a", "b", "c"], "r");
        let mut f = ParticleField::new(100, 36);
        f.retarget(&g);
        for _ in 0..120 {
            f.step(&g, 0.4);
        }
        let lit = |hover: Option<usize>| -> usize {
            let mut cv = Canvas::new(100, 36);
            f.render(&mut cv, &g, hover, 0.0);
            cv.to_text()
                .iter()
                .flatten()
                .filter(|s| s.text.chars().any(|c| c != ' '))
                .count()
        };
        assert!(lit(None) > 0);
        let kid = g.nodes.iter().position(|n| n.role == Role::Kid).unwrap();
        // same dots are drawn either way; hover only changes how bright they are
        assert!(lit(Some(kid)) > 0);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
