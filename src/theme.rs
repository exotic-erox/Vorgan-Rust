//! Caelestia scheme reader and the colour palette, byte-for-byte the same
//! defaults as the Python vorgan.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Col(pub u8, pub u8, pub u8);

impl Col {
    pub fn from_hex(s: &str, default: &str) -> Col {
        parse_hex(s).unwrap_or_else(|| parse_hex(default).unwrap())
    }
    pub fn rgb(self) -> ratatui::style::Color {
        ratatui::style::Color::Rgb(self.0, self.1, self.2)
    }
}

pub fn parse_hex(s: &str) -> Option<Col> {
    let v = s.trim().trim_start_matches('#');
    if v.len() != 6 || !v.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let n = u32::from_str_radix(v, 16).ok()?;
    Some(Col((n >> 16) as u8, (n >> 8) as u8, n as u8))
}

pub fn mix(a: Col, b: Col, t: f32) -> Col {
    let f = |x: f32, y: f32| (x + (y - x) * t).round() as u8;
    Col(
        f(a.0 as f32, b.0 as f32),
        f(a.1 as f32, b.1 as f32),
        f(a.2 as f32, b.2 as f32),
    )
}

pub fn scheme_path() -> PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".local/state"));
    base.join("caelestia").join("scheme.json")
}

pub fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// name → colours are fixed at ten kinds, so the map is an array.
pub const KIND_NAMES: [&str; 10] = [
    "dir", "archive", "image", "audio", "video", "code", "pdf", "text", "config", "other",
];

#[derive(Clone)]
pub struct Palette {
    pub bg: Col,
    pub panel: Col,
    pub panel2: Col,
    pub panel3: Col,
    pub sel: Col,
    pub accent: Col,
    pub accent2: Col,
    pub text: Col,
    pub dim: Col,
    pub faint: Col,
    pub line: Col,
    pub err: Col,
    pub ok: Col,
    pub pin: Col,
    pub dark: bool,
    /// colours per KIND_NAMES order
    pub kinds: [Col; 10],
}

fn kind(i: usize) -> &'static str {
    KIND_NAMES[i]
}

impl Default for Palette {
    fn default() -> Self {
        Palette {
            bg: Col(0x0d, 0x0e, 0x12),
            panel: Col(0x12, 0x13, 0x18),
            panel2: Col(0x17, 0x19, 0x1f),
            panel3: Col(0x1d, 0x1f, 0x26),
            sel: Col(0x44, 0x52, 0x74),
            accent: Col(0xb8, 0xc6, 0xee),
            accent2: Col(0xc0, 0xc6, 0xdc),
            text: Col(0xe4, 0xe5, 0xf0),
            dim: Col(0xa9, 0xaa, 0xb5),
            faint: Col(0x73, 0x75, 0x7f),
            line: Col(0x45, 0x48, 0x50),
            err: Col(0xfa, 0x74, 0x6f),
            ok: Col(0xb5, 0xcc, 0xba),
            pin: Col(0xff, 0xd8, 0x66),
            dark: true,
            kinds: [
                Col(0x8e, 0xad, 0xd5),
                Col(0xde, 0xc5, 0xf5),
                Col(0x44, 0xde, 0xf5),
                Col(0xa9, 0x9c, 0xff),
                Col(0xbb, 0xb9, 0xf8),
                Col(0x89, 0xec, 0xff),
                Col(0xfa, 0x74, 0x6f),
                Col(0xe4, 0xe5, 0xf0),
                Col(0xff, 0xdc, 0xf2),
                Col(0xa9, 0xaa, 0xb5),
            ],
        }
    }
}

impl Palette {
    pub fn kind(&self, i: usize) -> Col {
        self.kinds[i]
    }

    pub fn side(&self) -> Col {
        mix(self.panel, self.bg, 0.45)
    }
    pub fn soft(&self) -> Col {
        mix(self.panel, self.bg, 0.22)
    }

    pub fn from_scheme(data: &serde_json::Value) -> Palette {
        use serde_json as j;
        let c = data.get("colours").and_then(|c| c.as_object());
        let Some(c) = c else {
            return Palette::default();
        };
        let g = |k: &str, default: Col| -> Col {
            c.get(k)
                .and_then(|v| v.as_str())
                .and_then(crate::theme::parse_hex)
                .unwrap_or(default)
        };
        let d = Palette::default();
        let mode_light = data.get("mode").and_then(|m| m.as_str()) == Some("light");
        let mut p = Palette {
            bg: g("surface", d.bg),
            panel: g("surfaceContainerLow", d.panel),
            panel2: g("surfaceContainer", d.panel2),
            panel3: g("surfaceContainerHigh", d.panel3),
            sel: g("primaryContainer", d.sel),
            accent: g("primary", d.accent),
            accent2: g("secondary", d.accent2),
            text: g("onSurface", d.text),
            dim: g("onSurfaceVariant", d.dim),
            faint: g("outline", d.faint),
            line: g("outlineVariant", d.line),
            err: g("error", d.err),
            ok: g("success", d.ok),
            pin: if c.contains_key(kind(8)) {
                g("term3", d.pin)
            } else {
                d.pin
            },
            dark: !mode_light,
            kinds: d.kinds,
        };
        // the scheme may omit 'term3' etc; mirror python: pin = term3 else tertiary
        if c.contains_key("term3") {
            p.pin = g("term3", d.pin);
        } else if c.contains_key("tertiary") {
            p.pin = g("tertiary", d.pin);
        }
        let k = |i: usize, key: &str, dv: Col| -> Col {
            let alt = if c.contains_key(key) { g(key, dv) } else { dv };
            match i {
                0 => {
                    if c.contains_key("sapphire") {
                        g("sapphire", alt)
                    } else {
                        g("primary", alt)
                    }
                }
                _ => alt,
            }
        };
        for (i, _name) in KIND_NAMES.iter().enumerate() {
            let fallback_json: serde_json::Value = j::json!({}); // placeholder, python table below
            let _ = &fallback_json;
            p.kinds[i] = match i {
                0 => k(0, "sapphire", d.kinds[0]),
                1 => {
                    if c.contains_key("tertiary") {
                        g("tertiary", d.kinds[1])
                    } else {
                        d.kinds[1]
                    }
                }
                2 => {
                    if c.contains_key("term2") {
                        g("term2", d.kinds[2])
                    } else {
                        d.kinds[2]
                    }
                }
                3 => {
                    if c.contains_key("term1") {
                        g("term1", d.kinds[3])
                    } else {
                        d.kinds[3]
                    }
                }
                4 => {
                    if c.contains_key("term13") {
                        g("term13", d.kinds[4])
                    } else {
                        d.kinds[4]
                    }
                }
                5 => {
                    if c.contains_key("term10") {
                        g("term10", d.kinds[5])
                    } else {
                        d.kinds[5]
                    }
                }
                6 => {
                    if c.contains_key("error") {
                        g("error", d.kinds[6])
                    } else {
                        d.kinds[6]
                    }
                }
                7 => {
                    if c.contains_key("onSurface") {
                        g("onSurface", d.kinds[7])
                    } else {
                        d.kinds[7]
                    }
                }
                8 => {
                    if c.contains_key("term3") {
                        g("term3", d.kinds[8])
                    } else {
                        d.kinds[8]
                    }
                }
                _ => {
                    if c.contains_key("onSurfaceVariant") {
                        g("onSurfaceVariant", d.kinds[9])
                    } else {
                        d.kinds[9]
                    }
                }
            };
        }
        p
    }

    pub fn load(path: &Path) -> (Palette, Option<SystemTime>) {
        match fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(data) => {
                let mt = fs::metadata(path).and_then(|m| m.modified()).ok();
                (Palette::from_scheme(&data), mt)
            }
            None => (Palette::default(), None),
        }
    }
}
