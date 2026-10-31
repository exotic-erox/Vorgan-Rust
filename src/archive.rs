//! Archives via bsdtar (libarchive). Archives behave like folders — the same
//! subprocess shape as the python app, so caches and passwords behave alike.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use crate::model::{Entry, Loc};

pub const FORMATS: [&str; 5] = ["zip", "tar.gz", "tar.zst", "tar.xz", "7z"];

pub fn fmt_args(fmt: &str) -> Vec<&'static str> {
    match fmt {
        "zip" => vec!["--format", "zip"],
        "tar.gz" => vec!["-z"],
        "tar.zst" => vec!["--zstd"],
        "tar.xz" => vec!["-J"],
        "7z" => vec!["--format", "7zip"],
        _ => vec![],
    }
}

#[derive(Debug)]
pub struct ArchiveError {
    pub msg: String,
    pub need_password: bool,
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

fn password_error(err: &str) -> bool {
    let low = err.to_lowercase();
    low.contains("passphrase") || low.contains("password") || low.contains("encrypted")
}

fn run(args: &[String], passphrase: Option<&str>) -> Result<String, ArchiveError> {
    let mut cmd = Command::new("bsdtar");
    cmd.args(args);
    if let Some(pw) = passphrase {
        cmd.arg("--passphrase").arg(pw);
    }
    cmd.env("LC_ALL", "C.UTF-8");
    let out = cmd.output().map_err(|_| ArchiveError {
        msg: "bsdtar is not installed (sudo pacman -S libarchive)".into(),
        need_password: false,
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(ArchiveError {
            msg: err.lines().last().unwrap_or("archive error").into(),
            need_password: password_error(&err),
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// midnight epoch of the year-month-day triple (civil → days, Hinnant)
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let yy = if m <= 2 { y - 1 } else { y };
    let era = (if yy >= 0 { yy } else { yy - 399 }) / 400;
    let yoe = yy - era * 400;
    let mp = (m - 3) % 12;
    let doy = ((153 * mp + 2) / 5) + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// parse bsdtar's `-tvf` time field: either "Nov 14 22:13" (this year) or "Nov 14 2023"
fn parse_mtime(month: &str, day: &str, tail: &str) -> i64 {
    let Some(mpos) = MONTHS.iter().position(|m| *m == month) else {
        return 0;
    };
    let Ok(d) = day.parse::<i64>() else { return 0 };

    let (hh, mm) = (0, 0);
    let year: i64 = if tail.contains(':') {
        {
            // current month/year from wall clock, via /proc-free math
            let secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let days = secs.div_euclid(86400);
            // inverse civil is overkill; a year estimate within ±1 is enough for sorting
            1970 + days / 365
        }
    } else {
        tail.parse().unwrap_or(0)
    };
    let _ = (hh, mm);
    days_from_civil(year, mpos as i64 + 1, d) * 86400
}

/// (name, is_dir, size, mtime) one row each; hardlinks skipped, symlinks kept
fn parse_listing(text: &str) -> Vec<(String, bool, u64, i64)> {
    let mut out = vec![];
    for line in text.lines() {
        let ws: Vec<&str> = line.split_whitespace().collect();
        if ws.len() < 9 {
            continue;
        }
        let perms = ws[0];
        if perms.starts_with('h') && line.contains(" link to ") {
            continue;
        }
        let mut name = ws[8..].join(" ");
        if perms.starts_with('l') {
            name = name.split(" -> ").next().unwrap_or_default().to_string();
        }
        let is_dir = perms.starts_with('d') || name.ends_with('/');
        let size: u64 = ws[4].parse().unwrap_or(0);
        let mtime = parse_mtime(ws[5], ws[6], ws[7]);
        out.push((name, is_dir, size, mtime));
    }
    out
}

#[derive(Default)]
pub struct ArcIndex {
    pub kids: HashMap<String, Vec<Entry>>,
    pub count: u64,
    pub total: u64,
}

impl ArcIndex {
    fn build(arc: &Path, raw: &[(String, bool, u64, i64)]) -> ArcIndex {
        let mut idx = ArcIndex::default();
        idx.kids.insert(String::new(), vec![]);
        let mk = |slot: &mut Vec<Entry>, e: Entry| {
            if !slot.iter().any(|x| x.name == e.name) {
                slot.push(e);
            }
        };
        for (name, is_dir, size, mt) in raw {
            let mut p = name.trim_matches('/').to_string();
            while p.starts_with("./") {
                p = p[2..].to_string();
            }
            if p.is_empty() || p == "." {
                continue;
            }
            let parts: Vec<&str> = p.split('/').collect();
            for k in 1..parts.len() {
                let d = parts[..k].join("/");
                let parent = if k == 1 {
                    String::new()
                } else {
                    parts[..k - 1].join("/")
                };
                let mut e = Entry::new(parts[k - 1], true);
                e.arc = Some(arc.to_path_buf());
                e.inner = d.clone();
                e.mtime = *mt;
                mk(idx.kids.entry(parent).or_default(), e);
                idx.kids.entry(d).or_default();
            }
            let parent = if parts.len() == 1 {
                String::new()
            } else {
                parts[..parts.len() - 1].join("/")
            };
            let slot = idx.kids.entry(parent).or_default();
            let base = mk;
            let mut e = Entry::new(*parts.last().unwrap_or(&""), *is_dir);
            e.size = if *is_dir { 0 } else { *size };
            e.mtime = *mt;
            e.arc = Some(arc.to_path_buf());
            e.inner = p.clone();
            e.kind = crate::model::kind_of(&e.name, false);
            base(slot, e);
            if *is_dir {
                idx.kids.entry(p.clone()).or_default();
            } else {
                idx.count += 1;
                idx.total += size;
            }
        }
        for v in idx.kids.values_mut() {
            v.sort_by(|a, b| {
                a.is_dir
                    .cmp(&b.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
        }
        idx
    }

    pub fn children(&self, inner: &str) -> Vec<Entry> {
        let key = inner.trim().trim_start_matches('/');
        self.kids.get(key).cloned().unwrap_or_default()
    }
}

type CacheKey = (PathBuf, u64, u64);

fn cache_key(p: &Path) -> Option<CacheKey> {
    let st = std::fs::symlink_metadata(p).ok()?;
    let mt = st
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some((p.to_path_buf(), mt, st.len()))
}

/// indexes of up to 8 archives, invalidated by (path, mtime, size) like python
pub fn get_index(arc: &Path, passphrase: Option<&str>) -> Result<Arc<ArcIndex>, ArchiveError> {
    let key = cache_key(arc).ok_or_else(|| ArchiveError {
        msg: "cannot stat archive".into(),
        need_password: false,
    })?;
    struct Cache(Mutex<HashMap<CacheKey, Arc<ArcIndex>>>);
    static CACHE: OnceLock<Cache> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Cache(Mutex::new(HashMap::new())));
    if let Some(idx) = cache.0.lock().unwrap().get(&key) {
        return Ok(idx.clone());
    }
    let list = run(&["-tvf".into(), arc.display().to_string()], passphrase)?;
    let raw = parse_listing(&list);
    let idx = Arc::new(ArcIndex::build(arc, &raw));
    {
        let mut m = cache.0.lock().unwrap();
        if m.len() > 8 {
            m.clear();
        }
        m.insert(key, idx.clone());
    }
    Ok(idx)
}

pub fn list_loc(loc: &Loc, passphrase: Option<&str>) -> Result<Vec<Entry>, ArchiveError> {
    let Some(a) = &loc.arc else {
        return Err(ArchiveError {
            msg: "not an archive".into(),
            need_password: false,
        });
    };
    Ok(get_index(a, passphrase)?.children(&loc.inner))
}

pub fn extract(
    arc: &Path,
    dest: &Path,
    members: Option<&[String]>,
    passphrase: Option<&str>,
    done_one: &mut dyn FnMut(u64),
) -> Result<(), ArchiveError> {
    std::fs::create_dir_all(dest).map_err(|e| ArchiveError {
        msg: e.to_string(),
        need_password: false,
    })?;
    // -v so the UI can count finished entries; python ran quiet but had no progress
    let mut args = vec![
        "-xvf".into(),
        arc.display().to_string(),
        "-C".into(),
        dest.display().to_string(),
    ];
    if let Some(m) = members {
        args.extend(m.iter().cloned());
    }
    match run(&args, passphrase) {
        Ok(stdout) => {
            let n = stdout.lines().count() as u64;
            done_one(n);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

pub fn compress(
    out: &Path,
    base: &Path,
    names: &[String],
    fmt: &str,
    passphrase: Option<&str>,
) -> Result<(), ArchiveError> {
    if !FORMATS.contains(&fmt) {
        return Err(ArchiveError {
            msg: format!("unknown format {fmt}"),
            need_password: false,
        });
    }
    if let Some(dir) = out.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let mut args: Vec<String> = vec!["-c".into()];
    args.extend(fmt_args(fmt).into_iter().map(String::from));
    args.extend([
        "-f".into(),
        out.display().to_string(),
        "-C".into(),
        base.display().to_string(),
    ]);
    if let Some(_pw) = passphrase.filter(|_| fmt == "zip") {
        args.extend(["--options".into(), "zip:encryption=aes256".into()]);
    }
    if passphrase.is_some() && fmt != "zip" {
        return Err(ArchiveError {
            msg: "passwords are only supported for zip here".into(),
            need_password: false,
        });
    }
    args.push("--".into());
    args.extend(names.iter().cloned());
    if let Err(e) = run(&args, passphrase.filter(|_| fmt == "zip")) {
        let _ = std::fs::remove_file(out);
        return Err(e);
    }
    Ok(())
}

/// first `limit` bytes of one member, as the preview does
pub fn read_member(
    arc: &Path,
    inner: &str,
    limit: u64,
    passphrase: Option<&str>,
) -> Result<Vec<u8>, ArchiveError> {
    let mut cmd = Command::new("bsdtar");
    cmd.arg("-xOf").arg(arc).arg("--").arg(inner);
    if let Some(pw) = passphrase {
        cmd.arg("--passphrase").arg(pw);
    }
    cmd.env("LC_ALL", "C.UTF-8");
    let out = cmd.output().map_err(|_| ArchiveError {
        msg: "bsdtar is not installed".into(),
        need_password: false,
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(ArchiveError {
            msg: err.lines().last().unwrap_or("archive error").into(),
            need_password: password_error(&err),
        });
    }
    Ok(out.stdout.into_iter().take(limit as usize).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_build_nests() {
        let raw = vec![
            ("docs/".into(), true, 0, 1000),
            ("docs/readme.md".into(), false, 42, 1000),
        ];
        let idx = ArcIndex::build(Path::new("/a/x.zip"), &raw);
        assert_eq!(idx.count, 1);
        assert_eq!(idx.total, 42);
        assert_eq!(idx.children("docs").len(), 1);
        assert!(idx
            .children("")
            .iter()
            .any(|e| e.name == "docs" && e.is_dir));
    }

    #[test]
    fn listing_parse_skips_links_and_spaces_in_names() {
        let text = "\n\
            -rw-r--r--  1 n n     42 Nov 14  2023 a.txt\n\
            hdified     1 n n      0 Nov 14  2023 lnk link to a.txt\n\
            -rw-r--r--  1 n n      5 Nov 14  2023 my file.txt\n";
        let out = parse_listing(text);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "a.txt");
        assert_eq!(out[0].2, 42);
        assert_eq!(out[1].0, "my file.txt");
    }

    #[test]
    fn mtime_year() {
        let t = parse_mtime("Nov", "14", "2023");
        assert_eq!(t, 1_699_920_000); // 2023-11-14 00:00 UTC
        assert!(t > 1_500_000_000);
    }
}
