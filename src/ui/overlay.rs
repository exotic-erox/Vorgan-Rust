//! Inline overlays: forms, context menus, help — no modal machinery, the
//! event loop owns dispatching while one of these is open.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::theme::Palette;

#[derive(Clone)]
pub enum Field {
    Input {
        label: String,
        text: String,
    },
    Password {
        label: String,
        text: String,
    },
    Select {
        label: String,
        options: Vec<String>,
        idx: usize,
    },
    Check {
        label: String,
        on: bool,
    },
}

impl Field {
    /// value collected on submit; checks pack "1"/"" like python truthiness
    pub fn value(&self) -> String {
        match self {
            Field::Input { text, .. } | Field::Password { text, .. } => text.clone(),
            Field::Select { options, idx, .. } => options.get(*idx).cloned().unwrap_or_default(),
            Field::Check { on, .. } => if *on { "1" } else { "" }.into(),
        }
    }
    fn label(&self) -> &str {
        match self {
            Field::Input { label, .. }
            | Field::Password { label, .. }
            | Field::Select { label, .. }
            | Field::Check { label, .. } => label,
        }
    }
    pub fn key(&mut self, ch: char) {
        match self {
            Field::Input { text, .. } | Field::Password { text, .. } => text.push(ch),
            _ => {}
        }
    }
    pub fn backspace(&mut self) {
        match self {
            Field::Input { text, .. } | Field::Password { text, .. } => {
                text.pop();
            }
            _ => {}
        }
    }
    pub fn cycle(&mut self, d: i32) {
        if let Field::Select { options, idx, .. } = self {
            let n = options.len() as i32;
            if n > 0 {
                *idx = (*idx as i32 + d).rem_euclid(n) as usize;
            }
        }
    }
    pub fn toggle(&mut self) {
        if let Field::Check { on, .. } = self {
            *on = !*on;
        }
    }
}

pub struct Form {
    pub title: String,
    pub note: String,
    pub fields: Vec<Field>,
    pub focus: usize,
    /// what to do when submitted; the App matches on this
    pub action: FormAction,
}

#[derive(Clone)]
pub enum FormAction {
    Dismiss,
    Rename(std::path::PathBuf),
    NewDir,
    NewFile,
    Delete(Vec<std::path::PathBuf>),
    Password(std::path::PathBuf),
    Search,
    Extract {
        jobs: Vec<(std::path::PathBuf, Option<Vec<String>>)>,
        whole: bool,
    },
    Compress {
        names: Vec<String>,
        base: std::path::PathBuf,
    },
}

pub enum Overlay {
    Form(Form),
    Menu {
        items: Vec<Option<(String, String, bool)>>,
        hover: usize,
        x: u16,
        y: u16,
    },
    Help,
}

impl Form {
    pub fn open_key(&mut self, code: crossterm::event::KeyCode) -> bool {
        use crossterm::event::KeyCode as K;

        let submit: bool = match code {
            K::Enter => true,
            K::Backspace => {
                if let Some(f) = self.fields.get_mut(self.focus) {
                    f.backspace();
                }
                false
            }
            K::Tab => {
                self.focus = (self.focus + 1).min(self.fields.len().saturating_sub(1));
                false
            }
            K::Left | K::Right => {
                if let Some(f) = self.fields.get_mut(self.focus) {
                    f.cycle(if code == K::Right { 1 } else { -1 });
                }
                false
            }
            K::Up => {
                self.focus = self.focus.saturating_sub(1);
                false
            }
            K::Down => {
                self.focus = (self.focus + 1).min(self.fields.len().saturating_sub(1));
                false
            }
            _ => false,
        };
        submit
    }

    pub fn open_char(&mut self, ch: char) {
        if let Some(f) = self.fields.get_mut(self.focus) {
            f.key(ch);
        }
    }

    pub fn toggle_field(&mut self) {
        if let Some(f) = self.fields.get_mut(self.focus) {
            if matches!(f, Field::Check { .. }) {
                f.toggle();
            }
        }
    }
}

/// the python Form's visual shape: a rounded box, 2/3 of width, centred;
/// labels at left, values at right, focus follows typing
pub fn draw_form(f: &mut ratatui::Frame, pal: &Palette, form: &Form) {
    let w = (form.title.chars().count() + 24).clamp(46, 66) as u16;
    let h: u16 = 4 + form.fields.len() as u16 + if form.note.is_empty() { 0 } else { 1 };
    let area = f.area();
    let x0 = area.x + (area.width.saturating_sub(w)) / 2;
    let y0 = area.y + (area.height.saturating_sub(h + 4)) / 2;
    let rect = ratatui::layout::Rect::new(
        x0.max(area.x),
        y0.max(area.y),
        w.min(area.width).max(20),
        (h + 4).min(area.height).max(5),
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Rgb(pal.line.0, pal.line.1, pal.line.2)))
        .style(Style::new().bg(Color::Rgb(pal.panel2.0, pal.panel2.1, pal.panel2.2)));
    f.render_widget(block, rect);
    let inner = rect.inner(ratatui::layout::Margin {
        vertical: 1,
        horizontal: 2,
    });
    let rows: Vec<Line> = form
        .fields
        .iter()
        .enumerate()
        .map(|(i, field)| {
            let focused = i == form.focus;
            let mut s: Vec<Span> = vec![Span::styled(" ", Style::default())];
            match field {
                Field::Check { label, on } => {
                    let mark = if *on { "\u{2611} " } else { "\u{2610} " };
                    s.push(Span::styled(
                        mark.to_string(),
                        Style::new().fg(crate::col(pal.accent2)).bg(panel_bg(pal)),
                    ));
                    s.push(Span::styled(
                        label.clone(),
                        Style::new().fg(txt(pal)).bg(panel_bg(pal)),
                    ));
                }
                _ => {
                    let lab = format!("{:<16}", field.label());
                    s.push(Span::styled(
                        lab,
                        Style::new().fg(crate::col(pal.faint)).bg(panel_bg(pal)),
                    ));
                    let text = match field {
                        Field::Input { text, .. } | Field::Password { text, .. } => {
                            if matches!(field, Field::Password { .. }) {
                                "\u{2022}".repeat(text.chars().count())
                            } else {
                                text.clone()
                            }
                        }
                        Field::Select { options, idx, .. } => {
                            let cur = options.get(*idx).cloned().unwrap_or_default();
                            format!("\u{2039} {cur} \u{203a}")
                        }
                        _ => String::new(),
                    };
                    let mut shown = text;
                    let cap = inner.width.saturating_sub(16) as usize;
                    if shown.width() > cap {
                        shown = shown
                            .chars()
                            .take(cap.saturating_sub(1))
                            .collect::<String>()
                            + "\u{2026}";
                    }
                    if focused {
                        s.push(Span::styled(
                            format!("{shown}\u{2588}"),
                            Style::new()
                                .fg(txt(pal))
                                .bg(Color::Rgb(pal.sel.0, pal.sel.1, pal.sel.2))
                                .add_modifier(Modifier::BOLD),
                        ));
                    } else {
                        s.push(Span::styled(
                            shown,
                            Style::new().fg(crate::col(pal.dim)).bg(panel_bg(pal)),
                        ));
                    }
                }
            }
            Line::from(s)
        })
        .collect();
    let mut lines = Vec::with_capacity(rows.len() + 3);
    lines.push(Line::from(vec![Span::styled(
        form.title.clone(),
        Style::new()
            .fg(crate::col(pal.accent))
            .bg(panel_bg(pal))
            .add_modifier(Modifier::BOLD),
    )]));
    if !form.note.is_empty() {
        for nl in form.note.split('\n') {
            lines.push(Line::from(vec![Span::styled(
                shorten(nl, inner.width as usize),
                Style::new().fg(crate::col(pal.faint)).bg(panel_bg(pal)),
            )]));
        }
    }
    lines.push(Line::default());
    lines.extend(rows);
    lines.push(Line::from(vec![
        Span::styled(
            " enter",
            Style::new()
                .fg(crate::col(pal.bg))
                .bg(crate::col(pal.accent))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " submits · ",
            Style::new().fg(crate::col(pal.faint)).bg(panel_bg(pal)),
        ),
        Span::styled(
            "esc",
            Style::new()
                .fg(crate::col(pal.bg))
                .bg(crate::col(pal.accent))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " cancels",
            Style::new().fg(crate::col(pal.faint)).bg(panel_bg(pal)),
        ),
    ]));
    f.render_widget(Paragraph::new(lines), inner);
}

fn panel_bg(pal: &Palette) -> Color {
    Color::Rgb(pal.panel2.0, pal.panel2.1, pal.panel2.2)
}
fn txt(pal: &Palette) -> Color {
    Color::Rgb(pal.text.0, pal.text.1, pal.text.2)
}

pub fn shorten(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut out = String::new();
    for ch in s.chars() {
        if out.width() + ch.width().unwrap_or(1) > w.saturating_sub(1) {
            break;
        }
        out.push(ch);
    }
    out.push('\u{2026}');
    out
}

/// the context menu / pick list; returns its rect for click mapping
pub fn draw_menu(
    f: &mut ratatui::Frame,
    pal: &Palette,
    items: &[Option<(String, String, bool)>],
    hover: usize,
    x: u16,
    y: u16,
) -> Rect {
    if items.is_empty() {
        return Rect::new(0, 0, 0, 0);
    }
    let w = items
        .iter()
        .filter_map(|i| i.as_ref().map(|(l, ..)| l.width() + 4))
        .max()
        .unwrap_or(20) as u16;
    let h = (items.len() + 2) as u16;
    let area = f.area();
    let x = x.min(area.width.saturating_sub(w)).max(area.x);
    let y = y.min(area.height.saturating_sub(h)).max(area.y);
    let rect = ratatui::layout::Rect::new(
        x + area.x,
        y + area.y,
        w.min(area.width),
        h.min(area.height),
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Rgb(pal.line.0, pal.line.1, pal.line.2)))
        .style(Style::new().bg(Color::Rgb(pal.panel3.0, pal.panel3.1, pal.panel3.2)));
    f.render_widget(block, rect);
    let rows: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, it)| match it {
            None => Line::from(Span::styled(
                "─".repeat(w.saturating_sub(2) as usize),
                Style::new()
                    .fg(Color::Rgb(pal.line.0, pal.line.1, pal.line.2))
                    .bg(panel3_bg(pal)),
            )),
            Some((label, _a, enabled)) => {
                let sel = i == hover && *enabled;
                let bg = if sel { pal.sel } else { pal.panel3 };
                let fg = if *enabled { pal.text } else { pal.faint };
                Line::from(Span::styled(
                    format!(" {}", pad(label, w.saturating_sub(3) as usize)),
                    Style::new()
                        .fg(Color::Rgb(fg.0, fg.1, fg.2))
                        .bg(Color::Rgb(bg.0, bg.1, bg.2)),
                ))
            }
        })
        .collect();
    f.render_widget(
        Paragraph::new(rows),
        rect.inner(ratatui::layout::Margin {
            vertical: 1,
            horizontal: 1,
        }),
    );
    rect
}

fn pad(s: &str, w: usize) -> String {
    let l = s.width();
    if l >= w {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(w - l))
    }
}

fn panel3_bg(pal: &Palette) -> Color {
    Color::Rgb(pal.panel3.0, pal.panel3.1, pal.panel3.2)
}

pub const HELP: [(&str, &str); 8] = [
    ("Move", "j k / arrows · l Enter open · h Backspace up · g G top bottom"),
    ("Graph", "Tab bubble graph · click dive · drag to pin · right-click unpin"),
    ("Jump", "f hint tags · m + letter pin folder · ' + letter warp · ~ folders"),
    ("Find", "/ live filter · F search subfolders · . hidden files · s sort"),
    ("Select", "space toggle · ctrl+a all · ctrl/shift click · drag onto folder"),
    ("Files", "y copy · x cut · p paste · r rename · a new folder · A new file · d trash · D delete · u undo"),
    ("Archives", "Enter browses inside · e extract · z compress (name, format, path)"),
    ("Other", "t terminal · ctrl+r refresh · q quit and cd · Q quit"),
];

pub fn draw_help(f: &mut ratatui::Frame, pal: &Palette) {
    let area = f.area();
    let w = 92.min(area.width.saturating_sub(2));
    let h = (HELP.len() + 5) as u16;
    let rect = ratatui::layout::Rect::new(
        area.x + (area.width.saturating_sub(w)) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        w,
        h,
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Rgb(pal.line.0, pal.line.1, pal.line.2)))
        .style(Style::new().bg(Color::Rgb(pal.panel2.0, pal.panel2.1, pal.panel2.2)));
    f.render_widget(block, rect);
    let mut lines = vec![Line::from(Span::styled(
        "Vorgan \u{00b7} keys and mouse".to_string(),
        Style::new()
            .fg(Color::Rgb(pal.accent.0, pal.accent.1, pal.accent.2))
            .bg(panel_bg(pal))
            .add_modifier(Modifier::BOLD),
    ))];
    for (k, v) in HELP {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{k:<9}"),
                Style::new()
                    .fg(Color::Rgb(pal.pin.0, pal.pin.1, pal.pin.2))
                    .bg(panel_bg(pal))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(v.to_string(), Style::new().fg(txt(pal)).bg(panel_bg(pal))),
        ]));
    }
    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "any key closes".to_string(),
        Style::new()
            .fg(Color::Rgb(pal.faint.0, pal.faint.1, pal.faint.2))
            .bg(panel_bg(pal)),
    )));
    f.render_widget(
        Paragraph::new(lines),
        rect.inner(ratatui::layout::Margin {
            vertical: 1,
            horizontal: 2,
        }),
    );
}
