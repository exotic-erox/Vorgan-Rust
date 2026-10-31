//! File list rows: icon, fitted name, right column, hint tags, scrolling.

use std::collections::{BTreeMap, HashSet};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use crate::model::{human_size, Entry, ICONS_NERD, ICONS_PLAIN};
use crate::theme::mix;
use crate::theme::Palette;

pub const HINT_KEYS: &str = "asdfghjklqwertyuiop";

/// one- or two-letter jump tags, python's make_tags
pub fn make_tags(n: usize) -> Vec<String> {
    if n <= HINT_KEYS.len() {
        return HINT_KEYS.chars().take(n).map(String::from).collect();
    }
    let base: Vec<char> = "asdfghjkl".chars().collect();
    base.iter()
        .flat_map(|a| base.iter().map(move |b| format!("{a}{b}")))
        .take(n)
        .collect()
}

fn cell_len(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(1)).sum()
}

/// `…` truncation, python's _fit
fn fit(s: &str, w: usize) -> String {
    if w == 0 {
        return String::new();
    }
    if cell_len(s) <= w {
        return format!("{s}{}", " ".repeat(w - cell_len(s)));
    }
    let mut out = String::new();
    for ch in s.chars() {
        if cell_len(&out) + cwidth(ch) > w.saturating_sub(1) {
            break;
        }
        out.push(ch);
    }
    let trail = w.saturating_sub(cell_len(&out) + 1);
    format!("{out}\u{2026}{}", " ".repeat(trail))
}

fn cwidth(ch: char) -> usize {
    ch.width().unwrap_or(1)
}

/// seconds → " %b %d"
fn date_of(mtime: i64) -> String {
    if mtime == 0 {
        return String::new();
    }
    let (_, mo, d) = crate::model::civil_date(
        mtime.div_euclid(86400) + crate::model::local_offset().div_euclid(86400),
    );
    const M: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mi = (mo.clamp(1, 12) - 1) as usize;
    format!(" {} {:02}", M[mi], d as u32)
}

pub struct ListView {
    pub rows: usize,
    pub top: usize,
    pub cursor: usize,
    pub view: Vec<Entry>,
    pub selected: HashSet<String>,
    pub hints: Option<BTreeMap<usize, String>>,
    pub drop_row: Option<usize>,
    pub current_name: String,
    pub is_main: bool,
    pub width: usize,
}

impl ListView {
    pub fn new(is_main: bool) -> ListView {
        ListView {
            rows: 0,
            top: 0,
            cursor: 0,
            view: vec![],
            selected: HashSet::new(),
            hints: None,
            drop_row: None,
            current_name: String::new(),
            is_main,
            width: 0,
        }
    }

    pub fn entry(&self) -> Option<&Entry> {
        self.view.get(self.cursor)
    }

    pub fn ensure_visible(&mut self) {
        if self.rows == 0 {
            return;
        }
        if self.cursor < self.top {
            self.top = self.cursor;
        }
        if self.cursor >= self.top + self.rows {
            self.top = self.cursor + 1 - self.rows;
        }
    }

    pub fn clear_sel(&mut self) {
        self.selected.clear();
    }

    /// one row → a stylistic Line; hint tags replace the icon when live
    pub fn row(&self, pal: &Palette, nerd: bool, r: usize) -> Line<'static> {
        let w = self.width;
        let Some(e) = self.view.get(r) else {
            return Line::default();
        };
        let is_cur = r == self.cursor;
        let bgc = if self.drop_row == Some(r) && self.is_main {
            mix(pal.panel, pal.ok, 0.45)
        } else if is_cur {
            mix(pal.panel, pal.sel, 0.85)
        } else if self.selected.contains(&e.name) {
            mix(pal.panel, pal.sel, 0.4)
        } else if self.is_main {
            pal.panel
        } else {
            pal.side()
        };
        let bg = Color::Rgb(bgc.0, bgc.1, bgc.2);
        let st = |color: crate::theme::Col, bold: bool| {
            Style::new()
                .fg(crate::col(color))
                .bg(bg)
                .add_modifier(if bold {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                })
        };

        let mut right = if e.is_dir {
            String::new()
        } else {
            format!(" {}", human_size(e.size))
        };
        if self.is_main && w >= 34 {
            right = format!("       {right}");
        }
        if self.is_main && w >= 70 && e.mtime > 0 {
            right = format!("{}{right}", date_of(e.mtime));
        }
        let right_len = cell_len(&right);

        let mut spans: Vec<Span> = vec![Span::styled(" ".to_string(), st(pal.panel, false))];
        if let Some(tag) = self.hints.as_ref().and_then(|h| h.get(&r)) {
            let tag = format!("{tag:<2}");
            spans.push(Span::styled(
                tag,
                Style::new()
                    .fg(crate::col(pal.bg))
                    .bg(crate::col(pal.accent))
                    .add_modifier(Modifier::BOLD),
            ));
        } else {
            let icons = if nerd { &ICONS_NERD } else { &ICONS_PLAIN };
            spans.push(Span::styled(
                format!("{} ", icons[e.kind]),
                st(pal.kind(e.kind), false),
            ));
        }
        let name_w = w.saturating_sub(2 + right_len);
        let is_dir_color = pal.kind(0);
        let name_style = st(if e.is_dir { is_dir_color } else { pal.text }, is_cur);
        let mut name_style2 = name_style;
        if e.link {
            name_style2 = name_style2.add_modifier(Modifier::ITALIC);
        }
        spans.push(Span::styled(fit(&e.name, name_w), name_style2));
        if !right.is_empty() {
            spans.push(Span::styled(
                right,
                st(if is_cur { pal.dim } else { pal.faint }, false),
            ));
        }
        Line::from(spans)
    }

    /// the whole visible block (already clipped to `rows`)
    pub fn lines(&self, pal: &Palette, nerd: bool) -> Vec<Line<'static>> {
        let lo = self.top;
        let hi = (self.top + self.rows).min(self.view.len());
        (lo..hi).map(|r| self.row(pal, nerd, r)).collect()
    }
}
