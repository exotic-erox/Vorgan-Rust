//! Particle field for the graph, ported from the author's music visualizer
//! vizr (Field.step/render): springs toward per-node ring homes, jitter,
//! damping 0.88^f, shock bursts, per-cell energy → colour, sparkle on the
//! hottest cells.
//!
//! Particles live in dot-offset space (dots from the canvas centre) — the
//! same mapping graph.Graph.to_dots uses: centre + (x, y) directly in dots.

use crate::graph::{Canvas, GNode, Graph, Role};
use crate::theme::{mix, Palette};

/// density → particle budget, vizr's auto-count rule scaled for a pane
const DENSITY_DIV: u32 = 2400;
const CAP: usize = 640;

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
    pub energy: Vec<f32>,
    w: u16,
    h: u16,
    /// engine clock (seconds, 30 Hz ticks)
    pub t: f32,
}

#[inline]
fn frand() -> f32 {
    crate::graph::rand(0.0, 1.0)
}

impl ParticleField {
    pub fn new(w: u16, h: u16) -> ParticleField {
        let n = ((w as u32 * h as u32 / DENSITY_DIV).clamp(90, CAP as u32)) as usize;
        let mut f = ParticleField {
            x: vec![0.0; n],
            y: vec![0.0; n],
            vx: vec![0.0; n],
            vy: vec![0.0; n],
            node: vec![0; n],
            ang: vec![0.0; n],
            rad: vec![0.0; n],
            ph: vec![0.0; n],
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
        let n = ((w as u32 * h as u32 / DENSITY_DIV).clamp(90, CAP as u32)) as usize;
        self.x.resize(n, 0.0);
        self.y.resize(n, 0.0);
        self.vx.resize(n, 0.0);
        self.vy.resize(n, 0.0);
        self.node.resize(n, 0);
        self.ang.resize(n, 0.0);
        self.rad.resize(n, 0.0);
        self.ph.resize(n, 0.0);
        self.energy.resize(n, 0.0);
        self.scatter();
    }

    /// scatter at random; used on rebuilds and resizes
    pub fn scatter(&mut self) {
        let n = self.x.len();
        for i in 0..n {
            self.ang[i] = (i as f32 / n.max(1) as f32) * std::f32::consts::TAU;
            self.rad[i] = frand();
            self.ph[i] = frand() * std::f32::consts::TAU;
            self.energy[i] = 0.0;
            self.x[i] = (frand() - 0.5) * self.w as f32 * 2.0;
            self.y[i] = (frand() - 0.5) * self.h as f32 * 2.0;
            self.vx[i] = 0.0;
            self.vy[i] = 0.0;
        }
    }

    /// reassign particles to the graph's nodes evenly after a rebuild
    pub fn retarget(&mut self, g: &Graph) {
        let _ = self.w;
        let n = g.nodes.len();
        if n == 0 {
            return;
        }
        for (i, slot) in self.node.iter_mut().enumerate() {
            *slot = (i % n) as u16;
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
            let r = node.radius() * (0.7 + self.rad[i] * 0.75) * g.sc * g.zoom;
            let a = self.ang[i] + self.t * 0.25;
            let hx = node.x * g.sc * g.xs() * g.zoom
                + a.cos() * r
                + (self.t * 1.3 + self.ph[i] * 40.0).sin() * (2.0 + self.rad[i] * 3.0);
            let hy = node.y * g.sc * g.zoom
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

    /// strew the dots across the canvas; per-cell mean energy sets the
    /// brightness, the owning node tints it, sparkle lands on hot cells
    pub fn render(&self, cv: &mut Canvas, pal: &Palette, nodes: &[GNode], sparkle: f32) {
        let (dots_x, dots_y) = (cv.w * 2, cv.h * 4);
        let mut cnt: Vec<u32> = vec![0; dots_x * dots_y];
        let mut energy_sum: Vec<u32> = vec![0; dots_x * dots_y];
        let mut node_of: Vec<u16> = vec![0; dots_x * dots_y];
        for i in 0..self.x.len() {
            // canvas centre in dot coords
            let ix = cv.w as i32 + self.x[i].round() as i32;
            let iy = cv.h as i32 * 2 + self.y[i].round() as i32;
            if ix < 0 || iy < 0 || ix >= dots_x as i32 || iy >= dots_y as i32 {
                continue;
            }
            let ci = iy as usize * dots_x + ix as usize;
            cnt[ci] += 1;
            energy_sum[ci] += (self.energy[i].min(16.0) * 256.0) as u32;
            node_of[ci] = self.node[i];
        }
        let mut nonzero: Vec<u32> = cnt.iter().copied().filter(|c| *c > 0).collect();
        nonzero.sort_unstable();
        let hot_cut = nonzero.get(nonzero.len() * 88 / 100).copied().unwrap_or(0);
        let twinkle_chance = sparkle * 0.09 * (0.5 + 0.5 * (self.t * 7.3).sin());
        let mut rng = 0x9e37_79b9_7f4a_7c15u64;
        for cy in 0..dots_y {
            for cx in 0..dots_x {
                let ci = cy * dots_x + cx;
                if cnt[ci] == 0 {
                    continue;
                }
                let se = (energy_sum[ci] as f32 / 256.0) / cnt[ci] as f32;
                let lvl = (((0.3 + se * 0.55) * 15.0) as i32).clamp(0, 15);
                let ncol = nodes
                    .get(node_of[ci] as usize)
                    .map(|n| match n.role {
                        Role::Kid if n.arc => pal.kind(1),
                        Role::Kid => pal.kind(0),
                        Role::Parent => pal.dim,
                        Role::More => pal.accent2,
                        _ => pal.accent,
                    })
                    .unwrap_or(pal.accent);
                let mut col = mix(pal.bg, ncol, lvl as f32 / 15.0 * 0.45);
                if cnt[ci] > hot_cut && hot_cut > 0 && twinkle_chance > 0.01 {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    if ((rng >> 33) as f32 / (1u32 << 31) as f32) < twinkle_chance {
                        col = pal.text; // sparkle: vizr's bright twinkles
                    }
                }
                cv.plot(cx as i32, cy as i32, col, 1);
            }
        }
    }
}
