//! Sixel raster at the terminal's true cell size, written straight to the
//! terminal. This is the sixel the python app dropped: here we own the whole
//! render loop, so the image parks in cells nobody repaints.

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

// sixel colour registers, 0..255 by terminal convention: fixed 6×6×6 cube
// (registers 0..215), deterministic and cheap to compute
fn levels(v: u8) -> u8 {
    let d = (((v as u32 + 25) / 51) as u8).min(5); // nearest cube level, midpoint ties up
    d * 51
}

pub fn register_of(r: u8, g: u8, b: u8) -> u8 {
    levels(r) / 51 * 36 + levels(g) / 51 * 6 + levels(b) / 51
}

/// runs of blank columns skip ahead with CSI B instead of '?' padding
fn push_row(out: &mut Vec<u8>, row: &[u8]) {
    let mut x = 0;
    while x < row.len() {
        if row[x] == 0 {
            let start = x;
            while x < row.len() && row[x] == 0 {
                x += 1;
            }
            let n = x - start;
            if n > 2 {
                out.extend_from_slice(format!("\x1b[{}B", n).as_bytes());
            } else {
                out.extend(std::iter::repeat_n(b'?', n));
            }
        } else {
            out.push(b'?' + (row[x] & 0x3f));
            x += 1;
        }
    }
}

/// encode one pre-sized RGBA buffer. `w`/`h` are device pixels; pixels are
/// alpha-composited over `bg` because foot gives sixel no per-dot alpha.
pub fn encode(img: &[u8], w: u16, h: u16, bg: (u8, u8, u8)) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut out = Vec::with_capacity(w * h / 2 + 4096);
    out.extend_from_slice(b"\x1bPq\"1;1;1;0"); // pixel aspect 1:1

    let bands = h.div_ceil(6);
    for band in 0..bands {
        let y0 = band * 6;
        let rows = 6.min(h - y0);
        if band > 0 {
            out.push(b'-'); // next six-row band
        }
        // reg → its dot byte for every (row, column) in this band
        let mut per_reg: Vec<Vec<Vec<u8>>> = Vec::new();
        for y in 0..rows {
            for x in 0..w {
                let p = (y0 + y) * w * 4 + x * 4;
                let a = img[p + 3] as u32;
                let rgb = [
                    ((img[p] as u32 * a + bg.0 as u32 * (255 - a)) / 255) as u8,
                    ((img[p + 1] as u32 * a + bg.1 as u32 * (255 - a)) / 255) as u8,
                    ((img[p + 2] as u32 * a + bg.2 as u32 * (255 - a)) / 255) as u8,
                ];
                let reg = register_of(rgb[0], rgb[1], rgb[2]) as usize;
                if per_reg.len() <= reg {
                    per_reg.resize(reg + 1, vec![]);
                }
                let pr = &mut per_reg[reg];
                if pr.len() <= y {
                    pr.resize(y + 1, vec![0u8; w]);
                }
                pr[y][x] |= 1 << y;
            }
        }
        for (reg, rows_dots) in per_reg.iter().enumerate() {
            if rows_dots.iter().all(|r| r.is_empty()) {
                continue;
            }
            let ri = (reg / 36) * 51;
            let gi = (reg % 36 / 6) * 51;
            let bi = (reg % 6) * 51;
            out.extend_from_slice(format!("#{};2;{};{};{}", reg, ri, gi, bi).as_bytes());
            for row in rows_dots {
                push_row(&mut out, row);
                out.push(b'$');
            }
        }
    }
    out.extend_from_slice(b"\x1b\\");
    out
}

/// place a sixel image with its top-left at terminal (row, col), both 1-based
pub fn place_at(row: u16, col: u16, data: &[u8]) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(format!("\x1b[{};{}H", row, col).as_bytes());
    let _ = out.write_all(data);
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registers() {
        assert_eq!(register_of(0, 0, 0), 0);
        assert_eq!(register_of(255, 255, 255), 215); // 5*36 + 5*6 + 5
        assert_eq!(register_of(60, 0, 0), 36); // nearest level 51
        assert_eq!(register_of(43, 0, 0) / 36, 1); // 51
    }
    #[test]
    fn blank_runs_use_csi_b() {
        let img = vec![0u8; 4 * 4]; // 4 columns × 1 row, fully transparent
        let data = encode(&img, 4, 1, (13, 14, 18));
        let s = String::from_utf8(data).unwrap();
        assert!(s.contains("Pq"));
        assert!(s.ends_with("\x1b\\"));
    }
}
