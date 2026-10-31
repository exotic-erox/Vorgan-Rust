//! Preview builders (run in worker threads), including the real-pixel image
//! body that feeds either sixel or the half-block renderer.

use std::sync::Arc;

use image::imageops::FilterType;

use crate::archive;
use crate::model::{self, human_size, sort_entries, Entry, ICONS_NERD, ICONS_PLAIN, TAGS};
use crate::ops;
use crate::theme::Palette;

/// a styled run of one preview line
#[derive(Clone, Debug)]
pub struct Span {
    pub text: String,
    /// 0xRRGGBB
    pub fg: u32,
    pub bold: bool,
}

impl Span {
    fn s(text: impl Into<String>, fg: u32) -> Span {
        Span {
            text: text.into(),
            fg,
            bold: false,
        }
    }
    fn b(text: impl Into<String>, fg: u32) -> Span {
        Span {
            text: text.into(),
            fg,
            bold: true,
        }
    }
}

#[derive(Clone)]
pub struct Line(pub Vec<Span>);

pub enum Body {
    Text(Vec<Line>),
    /// stretched to the pane's device pixels, ready for sixel or halfblocks
    Pixels(Arc<image::RgbaImage>, (usize, usize)),
}

pub struct Preview {
    pub head: Line,
    pub meta: Vec<(String, String)>,
    pub body: Body,
    pub need_password: bool,
}

pub fn rgb(c: crate::theme::Col) -> u32 {
    (c.0 as u32) << 16 | (c.1 as u32) << 8 | c.2 as u32
}

fn kcol(pal: &Palette, kind: usize) -> u32 {
    rgb(pal.kind(kind))
}

fn listing_lines(entries: Vec<Entry>, pal: &Palette, nerd: bool, limit: usize) -> Vec<Line> {
    let es = sort_entries(entries, "name", false);
    let total = es.len();
    if total == 0 {
        return vec![Line(vec![Span::s("(empty)", rgb(pal.faint))])];
    }
    let mut lines: Vec<Line> = Vec::new();
    for e in es.into_iter().take(limit) {
        let icon = if nerd {
            ICONS_NERD[e.kind]
        } else {
            ICONS_PLAIN[e.kind]
        };
        let mut sp = vec![
            Span::s(format!("{icon} "), kcol(pal, e.kind)),
            Span::s(
                e.name,
                if e.is_dir {
                    kcol(pal, model::KIND_DIR)
                } else {
                    rgb(pal.dim)
                },
            ),
        ];
        if !e.is_dir {
            sp.push(Span::s(format!("  {}", human_size(e.size)), rgb(pal.faint)));
        }
        lines.push(Line(sp));
    }
    if total > limit {
        lines.push(Line(vec![Span::s(
            format!("… and {} more", total - limit),
            rgb(pal.faint),
        )]));
    }
    lines
}

fn text_preview(data: &[u8], pal: &Palette) -> Vec<Line> {
    if data.iter().take(4096).any(|b| *b == 0) {
        return vec![Line(vec![Span::s("binary file", rgb(pal.faint))])];
    }
    let code = String::from_utf8_lossy(&data[..data.len().min(64 * 1024)]);
    let lines: Vec<Line> = code
        .lines()
        .take(200)
        .enumerate()
        .map(|(n, raw)| {
            Line(vec![
                Span::s(format!("{:>3} ", n + 1), rgb(pal.faint)),
                Span::s(raw.to_string(), rgb(pal.text)),
            ])
        })
        .collect();
    lines
}

fn read_up_to(path: &std::path::Path, limit: u64) -> Vec<u8> {
    use std::io::Read;
    std::fs::File::open(path)
        .map(|f| {
            let mut v = Vec::new();
            let _ = f.take(limit).read_to_end(&mut v);
            v
        })
        .unwrap_or_default()
}

/// python halfblock stretched to the pane; the sixel body keeps that behaviour
fn stretch_to(
    im: &image::DynamicImage,
    px: (usize, usize),
    bg: crate::theme::Col,
) -> image::RgbaImage {
    let mut buf = image::RgbaImage::from_pixel(
        px.0.max(1) as u32,
        px.1.max(1) as u32,
        image::Rgba([bg.0, bg.1, bg.2, 255]),
    );
    let fitted = image::imageops::resize(
        im,
        px.0.max(1) as u32,
        px.1.max(1) as u32,
        FilterType::Lanczos3,
    );
    image::imageops::overlay(&mut buf, &fitted, 0, 0);
    buf
}

fn meta_row(meta: &[(String, String)], pal: &Palette) -> Vec<Line> {
    meta.iter()
        .map(|(k, v)| {
            Line(vec![
                Span::s(format!("{k:<10}"), rgb(pal.faint)),
                Span::s(v.clone(), rgb(pal.text)),
            ])
        })
        .collect()
}

/// build the preview for one entry; runs in a worker thread
#[allow(clippy::too_many_arguments)]
pub fn build_preview(
    e: Option<&Entry>,
    pal: &Palette,
    cols: usize,
    rows: usize,
    nerd: bool,
    password: Option<&str>,
    cell: (u16, u16),
    want_pixels: bool,
) -> Preview {
    let nothing = Preview {
        head: Line(vec![Span::s("nothing selected", rgb(pal.faint))]),
        meta: vec![],
        body: Body::Text(vec![]),
        need_password: false,
    };
    let Some(e) = e else { return nothing };
    let icon = if nerd {
        ICONS_NERD[e.kind]
    } else {
        ICONS_PLAIN[e.kind]
    };
    let head = Line(vec![
        Span::s(format!("{icon} "), kcol(pal, e.kind)),
        Span::b(e.name.clone(), rgb(pal.text)),
    ]);
    let when = if e.mtime > 0 {
        model::fmt_local_time(e.mtime)
    } else {
        "unknown".into()
    };
    let mut meta = vec![
        ("type".to_string(), TAGS[e.kind].to_string()),
        (
            "size".to_string(),
            if e.is_dir {
                "folder".into()
            } else {
                human_size(e.size)
            },
        ),
        ("modified".to_string(), when),
        ("perms".to_string(), model::fmt_perms(e.mode)),
    ];
    let mut need_password = false;
    let body: Body = (|| {
        // archive-internal entries: browse / index / member read
        if e.virtual_() {
            let arc = e.arc.clone().unwrap();
            let idx = match archive::get_index(&arc, password) {
                Ok(i) => i,
                Err(er) => {
                    need_password = er.need_password;
                    return Body::Text(err_lines(&er.msg, pal));
                }
            };
            if e.is_dir {
                return Body::Text(listing_lines(idx.children(&e.inner), pal, nerd, 60));
            }
            if e.kind == model::KIND_ARCHIVE {
                meta.push(("files".into(), idx.count.to_string()));
                meta.push(("unpacked".into(), human_size(idx.total)));
                return Body::Text(listing_lines(idx.children(&e.inner), pal, nerd, 60));
            }
            let lim = if e.kind == 2 { 20_000_000 } else { 262_144 };
            let data = match archive::read_member(&arc, &e.inner, lim, password) {
                Ok(d) => d,
                Err(er) => {
                    need_password = er.need_password;
                    return Body::Text(err_lines(&er.msg, pal));
                }
            };
            return content_body(e, Some(data), &meta, pal, cols, rows, cell, want_pixels);
        }
        if e.is_dir {
            match ops::list_real(e.path.as_ref().unwrap()) {
                Ok(entries) => return Body::Text(listing_lines(entries, pal, nerd, 60)),
                Err(msg) => return Body::Text(err_lines(&msg, pal)),
            }
        }
        if e.kind == model::KIND_ARCHIVE {
            let path = e.path.clone().unwrap();
            let idx = match archive::get_index(&path, password) {
                Ok(i) => i,
                Err(er) => {
                    need_password = er.need_password;
                    return Body::Text(err_lines(&er.msg, pal));
                }
            };
            meta.push(("files".into(), idx.count.to_string()));
            meta.push(("unpacked".into(), human_size(idx.total)));
            Body::Text(listing_lines(idx.children(""), pal, nerd, 60))
        } else {
            let data = if e.kind == 2 {
                Some(read_up_to(e.path.as_ref().unwrap(), 20_000_000))
            } else if matches!(e.kind, 5 | 7 | 8 | 9) {
                Some(read_up_to(e.path.as_ref().unwrap(), 65_536))
            } else {
                None
            };
            content_body(e, data, &meta, pal, cols, rows, cell, want_pixels)
        }
    })();
    Preview {
        head,
        meta,
        body,
        need_password,
    }
}

fn err_lines(msg: &str, pal: &Palette) -> Vec<Line> {
    vec![Line(vec![Span::s(msg, rgb(pal.err))])]
}

#[allow(clippy::too_many_arguments)]
fn content_body(
    e: &Entry,
    data: Option<Vec<u8>>,
    meta: &[(String, String)],
    pal: &Palette,
    cols: usize,
    rows: usize,
    cell: (u16, u16),
    want_pixels: bool,
) -> Body {
    if e.kind == 2 {
        let blob = data.unwrap_or_default();
        match image::load_from_memory(&blob).map_err(|er| er.to_string()) {
            Ok(im) => {
                let dims = format!("{}×{}", im.width(), im.height());
                let mut m: Vec<(String, String)> = meta.to_vec();
                m.push(("pixels".into(), dims));
                if want_pixels {
                    let pane_px = (
                        cols.max(2) * cell.0 as usize,
                        rows.saturating_sub(6).max(2) * cell.1 as usize,
                    );
                    let fitted = stretch_to(&im, pane_px, pal.soft());
                    return Body::Pixels(Arc::new(fitted), pane_px);
                }
                return meta_as_body(&m, pal);
            }
            Err(msg) => return Body::Text(err_lines(&msg, pal)),
        }
    }
    if matches!(e.kind, 5 | 7 | 8 | 9) {
        let mut lines = meta_row(meta, pal);
        lines.push(Line(vec![]));
        lines.extend(text_preview(&data.unwrap_or_default(), pal));
        return Body::Text(lines);
    }
    meta_as_body(meta, pal)
}

fn meta_as_body(meta: &[(String, String)], pal: &Palette) -> Body {
    Body::Text(
        meta.iter()
            .map(|(k, v)| {
                Line(vec![
                    Span::s(format!("{k:<10}"), rgb(pal.faint)),
                    Span::s(v.clone(), rgb(pal.text)),
                ])
            })
            .collect(),
    )
}
