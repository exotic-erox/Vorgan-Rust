//! Config file (TOML) and persisted state (pins, anchors, cols, undo journal).

use std::fs;
use std::path::PathBuf;

use crate::theme::home;

/// action → default keys, same as the python app
pub const KEYMAP: [(&str, &[&str]); 33] = [
    ("down", &["j", "down"]),
    ("up", &["k", "up"]),
    ("enter", &["l", "right", "enter"]),
    ("back", &["h", "left", "backspace"]),
    ("top", &["g", "home"]),
    ("bottom", &["G", "end"]),
    ("graph", &["tab"]),
    ("hint", &["f"]),
    ("search", &["/"]),
    ("find", &["F"]),
    ("hidden", &["."]),
    ("pin", &["m"]),
    ("warp", &["'"]),
    ("toggle", &["space"]),
    ("selall", &["ctrl+a"]),
    ("copy", &["y"]),
    ("cut", &["x"]),
    ("paste", &["p"]),
    ("rename", &["r", "f2"]),
    ("newdir", &["a", "f10"]),
    ("newfile", &["A"]),
    ("trash", &["d", "delete"]),
    ("delete", &["D", "shift+delete"]),
    ("extract", &["e"]),
    ("compress", &["z"]),
    ("quit_cd", &["q"]),
    ("quit", &["Q", "ctrl+q"]),
    ("help", &["?"]),
    ("sort", &["s"]),
    ("refresh", &["ctrl+r"]),
    ("frecent", &["~"]),
    ("terminal", &["t"]),
    ("clear", &["escape"]),
];

#[derive(serde::Deserialize, Default)]
struct RawConfig {
    icons: Option<String>,
    show_hidden: Option<bool>,
    sort: Option<String>,
    archive_format: Option<String>,
    graph_max_nodes: Option<u32>,
    keys: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

pub struct Config {
    pub icons: String, // nerd | plain
    pub show_hidden: bool,
    pub sort: String,
    pub archive_format: String,
    pub graph_max_nodes: u32,
    /// action → key override(s), straight from [keys] in the toml
    pub key_overrides: Vec<(String, Vec<String>)>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            icons: "nerd".into(),
            show_hidden: false,
            sort: "name".into(),
            archive_format: "zip".into(),
            graph_max_nodes: 28,
            key_overrides: vec![],
        }
    }
}

fn conf_dir() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".config"));
    base.join("vorgan")
}

pub fn state_dir() -> PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".local/state"));
    base.join("vorgan")
}

impl Config {
    pub fn load() -> Config {
        let path = conf_dir().join("config.toml");
        let mut c = Config::default();
        let Ok(text) = fs::read_to_string(&path) else {
            return c;
        };
        let Ok(raw) = toml::from_str::<RawConfig>(&text) else {
            return c;
        };
        if let Some(v) = raw.icons {
            c.icons = v;
        }
        if let Some(v) = raw.show_hidden {
            c.show_hidden = v;
        }
        let sorts = ["name", "size", "date", "type"];
        if let Some(v) = raw.sort {
            if sorts.contains(&v.as_str()) {
                c.sort = v;
            }
        }
        if let Some(v) = raw.archive_format {
            c.archive_format = v;
        }
        if let Some(v) = raw.graph_max_nodes {
            c.graph_max_nodes = v;
        }
        if let Some(ks) = raw.keys {
            for (action, val) in ks {
                let keys = match val {
                    serde_json::Value::String(s) => vec![s],
                    serde_json::Value::Array(a) => a
                        .into_iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect(),
                    _ => continue,
                };
                if KEYMAP.iter().any(|(a, _)| a == &action) {
                    c.key_overrides.push((action, keys));
                }
            }
        }
        c
    }
}

/// python-style key string ("ctrl+a", "shift+delete", "tab", "G") → a key event shape
pub fn parse_key(s: &str) -> Option<(crossterm::event::KeyCode, crossterm::event::KeyModifiers)> {
    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    let (mods, name) = match s.split_once('+') {
        Some((m, n)) => {
            let mut mods = M::empty();
            let ok = m.split('+').all(|p| match p.to_lowercase().as_str() {
                "ctrl" => {
                    mods |= M::CONTROL;
                    true
                }
                "shift" => {
                    mods |= M::SHIFT;
                    true
                }
                "alt" => {
                    mods |= M::ALT;
                    true
                }
                _ => false,
            });
            if !ok {
                return None;
            }
            (mods, n)
        }
        None => (M::empty(), s),
    };
    let lower = name.to_lowercase();
    let code = match lower.as_str() {
        "enter" | "return" => K::Enter,
        "space" => K::Char(' '),
        "tab" => K::Tab,
        "backspace" => K::Backspace,
        "escape" | "esc" => K::Esc,
        "left" => K::Left,
        "right" => K::Right,
        "up" => K::Up,
        "down" => K::Down,
        "home" => K::Home,
        "end" => K::End,
        "delete" => K::Delete,
        "insert" => K::Insert,
        "pgup" => K::PageUp,
        "pgdn" => K::PageDown,
        "f2" => K::F(2),
        "f10" => K::F(10),
        "'" => K::Char('\''),
        "?" => K::Char('?'),
        "." => K::Char('.'),
        _ => {
            let mut ch = name.chars();
            if name.len() > 1 {
                return None;
            }
            match ch.next() {
                Some(c) if c.is_ascii_graphic() => K::Char(c),
                _ => return None,
            }
        }
    };
    Some((code, mods))
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct UndoOp {
    /// "trash" (a = original location, b = trash target), "move" (a → b),
    /// "copy" (a copy exists at b, undo deletes it)
    pub kind: String,
    pub a: String,
    pub b: String,
}

pub struct State {
    dir: PathBuf,
    pub pins: std::collections::BTreeMap<String, f64>,
    pub anchors: std::collections::BTreeMap<String, String>,
    pub cols: std::collections::BTreeMap<String, i64>,
    pub undo: Vec<UndoOp>,
}

impl State {
    /// graph.rs reads pins through this without needing the field public
    pub fn pins_ref(&self) -> &std::collections::BTreeMap<String, f64> {
        &self.pins
    }

    pub fn load() -> State {
        let dir = state_dir();
        let pins = read_map(dir.join("pins.json"));
        let anchors = read_map(dir.join("anchors.json"));
        let cols = read_map(dir.join("cols.json"));
        let undo: Vec<UndoOp> = fs::read_to_string(dir.join("undo.json"))
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        State {
            dir,
            pins,
            anchors,
            cols,
            undo,
        }
    }

    fn write<T: serde::Serialize>(&self, name: &str, value: &T) {
        let _ = fs::create_dir_all(&self.dir);
        let text = serde_json::to_string(value).unwrap_or_default();
        let tmp = self.dir.join(format!("{name}.tmp"));
        if fs::write(&tmp, text).is_ok() {
            let _ = fs::rename(&tmp, self.dir.join(name));
        }
    }

    pub fn set_pin(&mut self, key: &str, radius: f64) {
        self.pins.insert(key.into(), radius);
        self.write("pins.json", &self.pins);
    }
    pub fn drop_pin(&mut self, key: &str) -> bool {
        if self.pins.remove(key).is_none() {
            return false;
        }
        self.write("pins.json", &self.pins);
        true
    }
    pub fn set_anchor(&mut self, letter: &str, path: &str) {
        self.anchors.insert(letter.into(), path.into());
        self.write("anchors.json", &self.anchors);
    }
    pub fn set_cols(&mut self, cols: std::collections::BTreeMap<String, i64>) {
        self.cols = cols;
        self.write("cols.json", &self.cols);
    }
    /// newest first, capped at 100
    pub fn push_undo(&mut self, op: UndoOp) {
        self.undo.insert(0, op);
        if self.undo.len() > 100 {
            self.undo.truncate(100);
        }
        self.write("undo.json", &self.undo);
    }
    pub fn pop_undo(&mut self) -> Option<UndoOp> {
        if self.undo.is_empty() {
            return None;
        }
        let op = self.undo.remove(0);
        self.write("undo.json", &self.undo);
        Some(op)
    }
}

fn read_map<T: serde::de::DeserializeOwned + Default>(p: PathBuf) -> T {
    fs::read_to_string(p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode as K, KeyModifiers as M};
    #[test]
    fn keys_parse() {
        let (c, m) = parse_key("ctrl+a").unwrap();
        assert_eq!(c, K::Char('a'));
        assert_eq!(m, M::CONTROL);
        assert_eq!(parse_key("tab").unwrap(), (K::Tab, M::empty()));
        assert_eq!(parse_key("'").unwrap(), (K::Char('\''), M::empty()));
        let (c, m) = parse_key("shift+delete").unwrap();
        assert_eq!(c, K::Delete);
        assert_eq!(m, M::SHIFT);
        assert!(parse_key("bogus+key").is_none());
    }
}
