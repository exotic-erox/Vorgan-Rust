//! Preview pane: head, meta and body in one scrolling column (like the
//! python VerticalScroll). Pictures are fitted into the free box under the
//! meta rows and painted as sixels after the frame is flushed.
//!
//! A sixel lives outside ratatui's buffer, so the pane keeps its own books on
//! what is physically on screen: it knows when a picture has to be blanked
//! (`erase`), when something drew over it (`taint`) and when it can skip
//! the repaint because nothing changed.

use std::sync::Arc;

use image::imageops::{self, FilterType};
use image::{Rgba, RgbaImage};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::preview::{Body, Preview};
use crate::theme::{mix, Col, Palette};

pub struct PreviewPane {
    pub pv: Preview,
    pub scroll: usize,
    /// bumped on every new preview so a repainted file never looks "unchanged"
    gen: u64,
    /// the encoded picture for the current box: (key, sixel bytes)
    cache: Option<(String, Arc<Vec<u8>>)>,
    painted: Option<Painted>,
    /// cells to blank before the next paint (set when a picture goes away)
    pub erase: Option<Rect>,
    /// (total rows, visible rows) of the scrolling column, for the wheel clamp
    extent: (usize, usize),
}

struct Painted {
    key: String,
    rect: Rect,
    /// an overlay or menu wrote over part of it
    holes: bool,
}

pub struct SixReq {
    pub row: u16,
    pub col: u16,
    pub data: Arc<Vec<u8>>,
    key: String,
    rect: Rect,
}

impl Default for PreviewPane {
    fn default() -> Self {
        PreviewPane {
            pv: Preview {
                head: crate::preview::Line(vec![]),
                meta: vec![],
                body: Body::Text(vec![]),
                need_password: false,
            },
            scroll: 0,
            gen: 0,
            cache: None,
            painted: None,
            erase: None,
            extent: (0, 0),
        }
    }
}

fn style(fg: u32, bold: bool) -> Style {
    Style::new()
        .fg(Color::Rgb((fg >> 16) as u8, (fg >> 8) as u8, fg as u8))
        .add_modifier(if bold {
            Modifier::BOLD
        } else {
            Modifier::empty()
        })
}

fn line_of(l: &crate::preview::Line) -> Line<'static> {
    Line::from(
        l.0.iter()
            .map(|s| Span::styled(s.text.clone(), style(s.fg, s.bold)))
            .collect::<Vec<_>>(),
    )
}

fn rgb(c: Col) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

/// flatten over the pane colour first (resizing straight alpha leaves halos),
/// then fit the picture inside the box, centred, never stretched
fn fit(src: &RgbaImage, bw: u32, bh: u32, bg: Col) -> RgbaImage {
    let base = Rgba([bg.0, bg.1, bg.2, 255]);
    let mut flat = RgbaImage::from_pixel(src.width(), src.height(), base);
    imageops::overlay(&mut flat, src, 0, 0);
    let s = (bw as f32 / src.width() as f32).min(bh as f32 / src.height() as f32);
    let w = ((src.width() as f32 * s).round() as u32).clamp(1, bw);
    let h = ((src.height() as f32 * s).round() as u32).clamp(1, bh);
    let scaled = imageops::resize(&flat, w, h, FilterType::Lanczos3);
    let mut canvas = RgbaImage::from_pixel(bw, bh, base);
    imageops::overlay(
        &mut canvas,
        &scaled,
        ((bw - w) / 2) as i64,
        ((bh - h) / 2) as i64,
    );
    canvas
}

impl PreviewPane {
    /// a freshly built preview arrived: start at the top, drop the old picture
    pub fn set(&mut self, pv: Preview) {
        self.pv = pv;
        self.scroll = 0;
        self.refresh();
    }

    /// same content, but the picture must be re-fitted and repainted (palette change)
    pub fn refresh(&mut self) {
        self.gen += 1;
        self.cache = None;
    }

    /// the wheel: positive scrolls down
    pub fn scroll_by(&mut self, d: i32) {
        let max = self.extent.0.saturating_sub(self.extent.1) as i32;
        self.scroll = (self.scroll as i32 + d).clamp(0, max) as usize;
    }

    /// something was drawn over the picture; repaint it when the way is clear
    pub fn taint(&mut self) {
        if let Some(p) = &mut self.painted {
            p.holes = true;
        }
    }

    /// the terminal was cleared under us (resize, graph screen)
    pub fn forget(&mut self) {
        self.painted = None;
        self.erase = None;
    }

    pub fn mark_painted(&mut self, req: &SixReq) {
        self.painted = Some(Painted {
            key: req.key.clone(),
            rect: req.rect,
            holes: false,
        });
    }

    /// Draw the pane. `paint` is false while something modal is on top or the
    /// pane edge is being dragged. Some(SixReq) must be written after the flush.
    pub fn draw(
        &mut self,
        f: &mut ratatui::Frame,
        area: Rect,
        pal: &Palette,
        paint: bool,
    ) -> Option<SixReq> {
        let soft = pal.soft();
        f.render_widget(Clear, area);
        f.render_widget(Block::new().style(Style::new().bg(rgb(soft))), area);
        // python's `padding: 1 2`
        let inner = Rect {
            x: area.x + 2,
            y: area.y + 1,
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(1),
        };
        if inner.width < 6 || inner.height < 3 {
            self.leave_picture();
            return None;
        }

        let mut lines: Vec<Line> = vec![line_of(&self.pv.head)];
        if !self.pv.meta.is_empty() {
            lines.push(Line::default());
        }
        for (k, v) in &self.pv.meta {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{k:<10}"),
                    style(crate::preview::rgb(pal.faint), false),
                ),
                Span::styled(v.clone(), style(crate::preview::rgb(pal.text), false)),
            ]));
        }
        lines.push(Line::default());

        match &self.pv.body {
            Body::Text(body) => {
                lines.extend(body.iter().map(line_of));
                self.leave_picture();
                self.text(f, area, inner, lines, pal);
                None
            }
            Body::Pixels(src) => {
                let head_rows = lines.len() as u16;
                let rect = Rect {
                    x: inner.x,
                    y: inner.y + head_rows,
                    width: inner.width,
                    height: inner.height.saturating_sub(head_rows),
                };
                self.extent = (0, 0);
                self.scroll = 0;
                f.render_widget(Paragraph::new(lines), inner);
                if rect.height < 2 {
                    self.leave_picture();
                    return None;
                }
                let (cw, ch) = crate::sixel::cell_px();
                let key = format!(
                    "{}|{},{},{},{}|{cw}x{ch}|{},{},{}",
                    self.gen, rect.x, rect.y, rect.width, rect.height, soft.0, soft.1, soft.2
                );

                // anything already on screen that this paint will not cover
                // exactly has to be blanked first (unset dots are transparent)
                let stale = self
                    .painted
                    .as_ref()
                    .is_some_and(|p| p.key != key || p.rect != rect);
                if stale {
                    self.erase = self.painted.take().map(|p| p.rect);
                }
                if !paint {
                    return None;
                }
                if self.painted.as_ref().is_some_and(|p| !p.holes) {
                    return None; // still there, nothing touched it
                }
                let data = match &self.cache {
                    Some((k, d)) if *k == key => d.clone(),
                    _ => {
                        let canvas = fit(
                            src,
                            rect.width as u32 * cw as u32,
                            rect.height as u32 * ch as u32,
                            soft,
                        );
                        let d = Arc::new(crate::sixel::encode(
                            canvas.as_raw(),
                            canvas.width() as u16,
                            canvas.height() as u16,
                            (soft.0, soft.1, soft.2),
                        ));
                        self.cache = Some((key.clone(), d.clone()));
                        d
                    }
                };
                Some(SixReq {
                    row: rect.y + 1,
                    col: rect.x + 1,
                    data,
                    key,
                    rect,
                })
            }
        }
    }

    /// the picture is no longer wanted here: blank what is on screen
    fn leave_picture(&mut self) {
        if let Some(p) = self.painted.take() {
            self.erase = Some(p.rect);
        }
    }

    fn text(
        &mut self,
        f: &mut ratatui::Frame,
        area: Rect,
        inner: Rect,
        lines: Vec<Line<'static>>,
        pal: &Palette,
    ) {
        let (total, view) = (lines.len(), inner.height as usize);
        self.extent = (total, view);
        self.scroll = self.scroll.min(total.saturating_sub(view));
        let shown: Vec<Line> = lines.into_iter().skip(self.scroll).take(view).collect();
        f.render_widget(Paragraph::new(shown), inner);
        if total > view {
            let bar = Rect {
                x: area.x + area.width - 1,
                y: area.y,
                width: 1,
                height: area.height,
            };
            let mut st = ScrollbarState::new(total)
                .viewport_content_length(view)
                .position(self.scroll);
            f.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None)
                    .track_symbol(Some(" "))
                    .thumb_symbol("┃")
                    .thumb_style(Style::new().fg(rgb(mix(pal.soft(), pal.faint, 0.8))))
                    .track_style(Style::new().bg(rgb(pal.soft()))),
                bar,
                &mut st,
            );
        }
    }
}
