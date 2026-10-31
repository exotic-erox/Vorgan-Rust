//! Preview pane: draws text/meta previews from the worker; images render
//! through the sixel backend with damage tracking — repainted only when
//! the entry, the pane or the cell pixel size changes.

use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use crate::model::Entry;
use crate::preview::{Body, Preview};

pub struct PreviewPane {
    pub pv: Preview,
    /// damage key of the last emitted sixel
    pub drawn: Option<String>,
}

impl Default for PreviewPane {
    fn default() -> Self {
        PreviewPane {
            pv: empty(),
            drawn: None,
        }
    }
}

fn empty() -> Preview {
    Preview {
        head: crate::preview::Line(vec![]),
        meta: vec![],
        body: Body::Text(vec![]),
        need_password: false,
    }
}

pub struct SixReq {
    pub row: u16,
    pub col: u16,
    pub data: Vec<u8>,
}

/// 0xRRGGBB → ratatui fg style
fn fgu(fg: u32, bold: bool) -> Style {
    Style::new()
        .fg(Color::Rgb(
            ((fg >> 16) & 0xff) as u8,
            ((fg >> 8) & 0xff) as u8,
            (fg & 0xff) as u8,
        ))
        .add_modifier(if bold {
            Modifier::BOLD
        } else {
            Modifier::empty()
        })
}

impl PreviewPane {
    /// build the pane's lines; Some(SixReq) must be painted after the flush
    pub fn draw(
        &mut self,
        f: &mut ratatui::Frame,
        area: Rect,
        pal: &crate::theme::Palette,
        e: Option<&Entry>,
    ) -> Option<SixReq> {
        if area.width < 10 || area.height < 5 {
            return None;
        }
        f.render_widget(Clear, area);
        let mut lines: Vec<Line> = vec![
            Line::from(
                self.pv
                    .head
                    .0
                    .iter()
                    .map(|s| Span::styled(s.text.clone(), fgu(s.fg, s.bold)))
                    .collect::<Vec<_>>(),
            ),
            Line::default(),
        ];
        for (k, v) in &self.pv.meta {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{k:<9}"),
                    fgu(crate::preview::rgb(pal.faint), false),
                ),
                Span::styled(v.clone(), fgu(crate::preview::rgb(pal.text), false)),
            ]));
        }
        lines.push(Line::default());
        let used = lines.len() as u16;
        match &self.pv.body {
            Body::Text(ls) => {
                for l in ls.iter().take(area.height.saturating_sub(used) as usize) {
                    lines.push(Line::from(
                        l.0.iter()
                            .map(|s| Span::styled(s.text.clone(), fgu(s.fg, s.bold)))
                            .collect::<Vec<_>>(),
                    ));
                }
                f.render_widget(Paragraph::new(lines), area);
                self.drawn = None; // text replaced the picture
                None
            }
            Body::Pixels(img, _dims) => {
                let bar = Line::from(vec![Span::styled(
                    " ".repeat(area.width as usize),
                    Style::new().bg(Color::Rgb(pal.panel.0, pal.panel.1, pal.panel.2)),
                )]);
                while (lines.len() as u16) < area.height {
                    lines.push(bar.clone());
                }
                f.render_widget(Paragraph::new(lines), area);
                let cell = crate::sixel::cell_px();
                let desc = e.map(|e| format!("{:?}", e.path)).unwrap_or_default();
                let key = format!(
                    "{desc}|{},{},{},{},{},{}",
                    area.x, area.y, area.width, area.height, cell.0, cell.1
                );
                if self.drawn.as_ref() == Some(&key) {
                    return None; // nothing changed since the last paint
                }
                self.drawn = Some(key);
                let data = crate::sixel::encode(
                    img.as_ref(),
                    (img.width() as u16).min(2000),
                    (img.height() as u16).min(4000),
                    (pal.bg.0, pal.bg.1, pal.bg.2),
                );
                Some(SixReq {
                    row: area.y + 1,
                    col: area.x + 1,
                    data,
                })
            }
        }
    }
}
