//! Sixel raster at the terminal's true cell size, written straight to the
//! terminal. We own the whole render loop, so the picture parks in cells
//! nobody repaints (see ui/preview.rs for the erase/repaint bookkeeping).

use std::mem::zeroed;

/// terminal cell size in pixels (TIOCGWINSZ, foot reports it), else 8×16
pub fn cell_px() -> (u16, u16) {
    unsafe {
        let mut ws: libc::winsize = zeroed();
        if libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) == 0
            && ws.ws_row > 0
            && ws.ws_col > 0
            && ws.ws_xpixel > 4
            && ws.ws_ypixel > 4
        {
            return (
                4.max(ws.ws_xpixel / ws.ws_col),
                4.max(ws.ws_ypixel / ws.ws_row),
            );
        }
    }
    (8, 16)
}

// 6 × 7 × 6 colour cube = 252 registers, green gets the extra level because
// the eye is most sensitive there. Floyd–Steinberg dithering hides the steps.
const RL: i32 = 6;
const GL: i32 = 7;
const BL: i32 = 6;
const REGS: usize = (RL * GL * BL) as usize;
/// "no dot": the pixel matched the pane background, leave the cell alone
const NONE: u8 = 255;

fn step(levels: i32) -> f32 {
    255.0 / (levels - 1) as f32
}

fn level_of(v: i32, levels: i32) -> i32 {
    ((v as f32 / step(levels)).round() as i32).clamp(0, levels - 1)
}

fn level_value(l: i32, levels: i32) -> i32 {
    (l as f32 * step(levels)).round() as i32
}

pub fn register_of(r: u8, g: u8, b: u8) -> u8 {
    let (r, g, b) = (
        level_of(r as i32, RL),
        level_of(g as i32, GL),
        level_of(b as i32, BL),
    );
    ((r * GL + g) * BL + b) as u8
}

/// sixel colour components are percentages, not 0..255
fn percent(v: i32) -> i32 {
    (v * 100 + 127) / 255
}

fn register_def(reg: usize) -> String {
    let b = reg as i32 % BL;
    let g = (reg as i32 / BL) % GL;
    let r = reg as i32 / (BL * GL);
    format!(
        "#{reg};2;{};{};{}",
        percent(level_value(r, RL)),
        percent(level_value(g, GL)),
        percent(level_value(b, BL))
    )
}

/// RGBA → one register per pixel. Pixels that composite to exactly `bg` get
/// NONE, so letterbox bars and transparent PNG areas show the pane itself
/// instead of a slightly different rectangle.
fn quantize(img: &[u8], w: usize, h: usize, bg: (u8, u8, u8)) -> Vec<u8> {
    let mut idx = vec![NONE; w * h];
    let bg = [bg.0 as i32, bg.1 as i32, bg.2 as i32];
    // error rows are scaled by 16 and padded by one pixel on each side
    let mut cur = vec![[0i32; 3]; w + 2];
    let mut next = vec![[0i32; 3]; w + 2];
    for y in 0..h {
        for x in 0..w {
            let p = (y * w + x) * 4;
            let a = img[p + 3] as i32;
            let mut rgb = [0i32; 3];
            for c in 0..3 {
                rgb[c] = (img[p + c] as i32 * a + bg[c] * (255 - a)) / 255;
            }
            if rgb == bg {
                continue;
            }
            let mut got = [0i32; 3];
            let mut lv = [0i32; 3];
            for c in 0..3 {
                let levels = [RL, GL, BL][c];
                let want = (rgb[c] + cur[x + 1][c] / 16).clamp(0, 255);
                lv[c] = level_of(want, levels);
                got[c] = level_value(lv[c], levels);
                let err = want - got[c];
                cur[x + 2][c] += err * 7;
                next[x][c] += err * 3;
                next[x + 1][c] += err * 5;
                next[x + 2][c] += err;
            }
            idx[y * w + x] = ((lv[0] * GL + lv[1]) * BL + lv[2]) as u8;
        }
        std::mem::swap(&mut cur, &mut next);
        next.iter_mut().for_each(|e| *e = [0; 3]);
    }
    idx
}

/// one register's columns for a band, run-length packed with `!n` so no
/// escape byte ever lands inside the DCS string
fn push_runs(out: &mut Vec<u8>, cols: &[u8]) {
    let end = cols.iter().rposition(|&c| c != 0).map_or(0, |i| i + 1);
    let mut x = 0;
    while x < end {
        let c = cols[x];
        let mut n = 1;
        while x + n < end && cols[x + n] == c {
            n += 1;
        }
        let ch = b'?' + c;
        if n > 3 {
            out.extend_from_slice(format!("!{n}").as_bytes());
            out.push(ch);
        } else {
            out.extend(std::iter::repeat_n(ch, n));
        }
        x += n;
    }
}

/// encode one RGBA buffer of `w`×`h` device pixels. Pixels are composited
/// over `bg` (the pane colour) because sixel has no per-dot alpha.
pub fn encode(img: &[u8], w: u16, h: u16, bg: (u8, u8, u8)) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let idx = quantize(img, w, h, bg);

    let mut used = vec![false; REGS];
    idx.iter()
        .filter(|&&r| r != NONE)
        .for_each(|&r| used[r as usize] = true);

    let mut out = Vec::with_capacity(w * h / 3 + 4096);
    // P2 = 1: dots we never set stay as they were, which is what keeps the
    // letterbox and transparent areas seamless
    out.extend_from_slice(format!("\x1bP0;1;0q\"1;1;{w};{h}").as_bytes());
    for reg in (0..REGS).filter(|&r| used[r]) {
        out.extend_from_slice(register_def(reg).as_bytes());
    }

    let mut cols: Vec<Vec<u8>> = vec![Vec::new(); REGS];
    for y0 in (0..h).step_by(6) {
        let mut seen: Vec<usize> = Vec::new();
        for y in y0..(y0 + 6).min(h) {
            for x in 0..w {
                let r = idx[y * w + x];
                if r == NONE {
                    continue;
                }
                let c = &mut cols[r as usize];
                if c.is_empty() {
                    c.resize(w, 0);
                    seen.push(r as usize);
                }
                c[x] |= 1 << (y - y0);
            }
        }
        for (n, &reg) in seen.iter().enumerate() {
            out.extend_from_slice(format!("#{reg}").as_bytes());
            push_runs(&mut out, &cols[reg]);
            cols[reg].clear();
            out.push(if n + 1 < seen.len() { b'$' } else { b'-' });
        }
        if seen.is_empty() {
            out.push(b'-'); // blank band still has to advance
        }
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// Paint a sixel with its top-left at terminal (row, col), both 1-based.
/// Sixel scrolling is switched off (DECSDM) so a picture touching the last
/// row is clipped instead of scrolling the whole screen, and the cursor is
/// put back where ratatui left it.
pub fn place_at(row: u16, col: u16, data: &[u8]) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(format!("\x1b7\x1b[?80h\x1b[{row};{col}H").as_bytes());
    let _ = out.write_all(data);
    let _ = out.write_all(b"\x1b[?80l\x1b8");
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// minimal sixel reader: returns (w, h, pixels) with None for unset dots
    fn decode(data: &[u8]) -> (usize, usize, Vec<Option<(u8, u8, u8)>>) {
        let s = std::str::from_utf8(data).unwrap();
        let body = &s[s.find('q').unwrap() + 1..s.len() - 2];
        let mut chars = body.chars().peekable();
        let num = |it: &mut std::iter::Peekable<std::str::Chars>| -> usize {
            let mut n = 0;
            while let Some(d) = it.peek().and_then(|c| c.to_digit(10)) {
                n = n * 10 + d as usize;
                it.next();
            }
            n
        };
        let (mut w, mut h) = (0, 0);
        let mut palette = vec![(0u8, 0u8, 0u8); 256];
        let (mut reg, mut x, mut band, mut rep) = (0usize, 0usize, 0usize, 1usize);
        let mut dots: Vec<(usize, usize, (u8, u8, u8))> = Vec::new();
        while let Some(c) = chars.next() {
            match c {
                '"' => {
                    num(&mut chars);
                    chars.next();
                    num(&mut chars);
                    chars.next();
                    w = num(&mut chars);
                    chars.next();
                    h = num(&mut chars);
                }
                '#' => {
                    reg = num(&mut chars);
                    if chars.peek() == Some(&';') {
                        chars.next();
                        assert_eq!(num(&mut chars), 2);
                        let mut v = [0u8; 3];
                        for c in v.iter_mut() {
                            chars.next();
                            let p = num(&mut chars);
                            assert!(p <= 100, "component {p} is not a percentage");
                            *c = (p * 255 / 100) as u8;
                        }
                        palette[reg] = (v[0], v[1], v[2]);
                    }
                }
                '!' => rep = num(&mut chars),
                '$' => x = 0,
                '-' => {
                    x = 0;
                    band += 1;
                }
                '?'..='~' => {
                    let bits = c as u8 - b'?';
                    for _ in 0..rep {
                        for r in 0..6 {
                            if bits >> r & 1 == 1 {
                                dots.push((x, band * 6 + r, palette[reg]));
                            }
                        }
                        x += 1;
                    }
                    rep = 1;
                }
                other => panic!("unexpected {other:?} inside sixel data"),
            }
        }
        let mut px = vec![None; w * h];
        for (x, y, c) in dots {
            if y < h {
                px[y * w + x] = Some(c);
            }
        }
        (w, h, px)
    }

    #[test]
    fn registers() {
        assert_eq!(register_of(0, 0, 0), 0);
        assert_eq!(register_of(255, 255, 255) as usize, REGS - 1);
        assert_eq!(percent(255), 100);
        assert_eq!(percent(0), 0);
    }

    #[test]
    fn data_has_no_stray_escapes() {
        // wide blank gaps are exactly what used to emit CSI B inside the DCS
        let (w, h) = (40usize, 14usize);
        let mut img = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in [0usize, 39] {
                let p = (y * w + x) * 4;
                img[p..p + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let data = encode(&img, w as u16, h as u16, (13, 14, 18));
        assert!(data.starts_with(b"\x1bP"));
        assert!(data.ends_with(b"\x1b\\"));
        assert!(data[2..data.len() - 2].iter().all(|&b| b != 0x1b));
    }

    #[test]
    fn round_trip_keeps_shape_and_leaves_background_unset() {
        let (w, h) = (30usize, 20usize);
        let bg = (26u8, 27u8, 38u8);
        let mut img = vec![0u8; w * h * 4];
        for y in 5..15 {
            for x in 4..16 {
                let p = (y * w + x) * 4;
                img[p..p + 4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        // fully transparent pixels composite to bg and must not be drawn
        let data = encode(&img, w as u16, h as u16, bg);
        let (dw, dh, px) = decode(&data);
        assert_eq!((dw, dh), (w, h));
        for y in 0..h {
            for x in 0..w {
                let inside = (5..15).contains(&y) && (4..16).contains(&x);
                let got = px[y * w + x];
                if inside {
                    let c = got.expect("red block must be drawn");
                    assert!(c.0 > 240 && c.1 < 15 && c.2 < 15, "{c:?}");
                } else {
                    assert!(got.is_none(), "background dot drawn at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn gradient_survives_dithering() {
        let (w, h) = (64usize, 12usize);
        let mut img = vec![255u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let v = (x * 255 / (w - 1)) as u8;
                let p = (y * w + x) * 4;
                img[p..p + 3].copy_from_slice(&[v, v, v]);
            }
        }
        let (_, _, px) = decode(&encode(&img, w as u16, h as u16, (200, 10, 10)));
        let avg = |x0: usize, x1: usize| -> f32 {
            let mut s = 0.0;
            for y in 0..h {
                for x in x0..x1 {
                    s += px[y * w + x].map_or(0.0, |c| c.1 as f32);
                }
            }
            s / (h * (x1 - x0)) as f32
        };
        let (a, b, c) = (avg(0, 8), avg(28, 36), avg(56, 64));
        assert!(a < b && b < c, "{a} {b} {c}");
        assert!((b - 127.0).abs() < 25.0, "midpoint drifted: {b}");
    }
}
