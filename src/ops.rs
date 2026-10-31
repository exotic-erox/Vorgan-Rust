//! Real file system operations. No UI code here.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::model::Entry;
use crate::theme::home;

pub fn list_real(path: &Path) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    let it = fs::read_dir(path).map_err(|e| e.to_string())?;
    for de in it {
        let Ok(de) = de else { continue };
        let meta = match de.metadata() {
            Ok(m) => m,
            Err(_) => match de.file_type() {
                Ok(_) => continue,
                Err(_) => continue,
            },
        };
        let is_symlink = fs::symlink_metadata(de.path())
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        let st = fs::symlink_metadata(de.path()).unwrap_or(meta);
        let mode = std::os::unix::fs::MetadataExt::mode(&st);
        out.push(Entry {
            name: de.file_name().to_string_lossy().into_owned(),
            is_dir: de.file_type().map(|t| t.is_dir()).unwrap_or(false),
            size: st.len(),
            mtime: st
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            mode,
            link: is_symlink,
            kind: 0,
            path: Some(de.path()),
            arc: None,
            inner: String::new(),
        });
    }
    for e in out.iter_mut() {
        e.kind = crate::model::kind_of(&e.name, e.is_dir);
    }
    Ok(out)
}

pub fn list_subdirs(path: &Path, show_hidden: bool) -> Vec<PathBuf> {
    let mut ds = Vec::new();
    let Ok(it) = fs::read_dir(path) else {
        return ds;
    };
    for de in it.flatten() {
        let is_dir = de.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            let name = de.file_name().to_string_lossy().into_owned();
            if show_hidden || !name.starts_with('.') {
                ds.push(de.path());
            }
        }
    }
    ds.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    ds
}

pub fn unique_path(folder: &Path, name: &str) -> PathBuf {
    let target = folder.join(name);
    if !lexists(&target) {
        return target;
    }
    // python rule: "x.tar.gz" → "x (2).tar.gz" — the ".tar." pair stays glued
    let mut suffix = Path::new(name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let stem_full = name[..name.len().saturating_sub(suffix.len())].to_string();
    let mut stem = stem_full.clone();
    if !suffix.is_empty() && stem_full.to_lowercase().ends_with(".tar") {
        let at = stem_full.len() - 4;
        suffix = format!("{}{}", &stem_full[at..], suffix); // ".tar" + ".gz"
        stem = stem_full[..at].to_string();
    }
    let mut k = 2;
    loop {
        let cand = folder.join(format!("{stem} ({k}){suffix}"));
        if !lexists(&cand) {
            return cand;
        }
        k += 1;
    }
}

pub fn lexists(p: &Path) -> bool {
    p.symlink_metadata().is_ok()
}

fn inside(child: &Path, parent: &Path) -> bool {
    match (child.canonicalize(), parent.canonicalize()) {
        (Ok(c), Ok(p)) => c.starts_with(&p),
        _ => false,
    }
}

/// copy one file with progress: f(inc_bytes) called per chunk
fn copy_file(src: &Path, dst: &Path, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
    let mut f = fs::File::open(src).map_err(|e| e.to_string())?;
    let mut buf = [0u8; 256 * 1024];
    let mut total = 0u64;
    let mut g = fs::File::create(dst).map_err(|e| e.to_string())?;
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        g.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        total += n as u64;
        progress(n as u64);
    }
    let _ = total;
    Ok(())
}

/// recursive copy mirroring shutil.copytree(symlinks=True), with progress
fn copy_tree(src: &Path, dst: &Path, progress: &mut dyn FnMut(u64)) -> Result<(), String> {
    fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let t = entry.path();
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let name = entry.file_name();
        let target = dst.join(&name);
        if ty.is_symlink() {
            let dest = fs::read_link(&t).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&dest, &target).map_err(|e| e.to_string())?;
        } else if ty.is_dir() {
            copy_tree(&t, &target, progress)?;
        } else {
            copy_file(&t, &target, progress)?;
        }
    }
    Ok(())
}

pub struct CopyMove {
    pub files: u64,
    pub bytes: u64,
    pub errs: Vec<String>,
    /// where each source landed (only for the successful ones), for undo
    pub targets: Vec<(PathBuf, PathBuf)>,
}

pub fn copy_paths(srcs: &[PathBuf], dst: &Path, progress: &mut dyn FnMut(u64)) -> CopyMove {
    let mut r = CopyMove {
        files: 0,
        bytes: 0,
        errs: vec![],
        targets: vec![],
    };
    for s in srcs {
        let is_dir = s
            .symlink_metadata()
            .map(|m| m.is_dir() && !m.file_type().is_symlink())
            .unwrap_or(false);
        let target = unique_path(
            dst,
            s.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .as_deref()
                .unwrap_or("?"),
        );
        let res = if is_dir {
            if inside(dst, s) {
                Err("cannot copy a folder into itself".into())
            } else {
                copy_tree(s, &target, progress)
            }
        } else {
            copy_file(s, &target, progress)
        };
        match res {
            Ok(()) => {
                r.files += 1;
                r.targets.push((s.clone(), target));
            }
            Err(e) => r.errs.push(format!("{}: {}", s.display(), e)),
        }
    }
    r
}

pub fn move_paths(srcs: &[PathBuf], dst: &Path, progress: &mut dyn FnMut(u64)) -> CopyMove {
    let mut r = CopyMove {
        files: 0,
        bytes: 0,
        errs: vec![],
        targets: vec![],
    };
    for s in srcs {
        let same = dst.canonicalize().ok() == s.parent().and_then(|p| p.canonicalize().ok());
        if same {
            continue;
        }
        let is_dir = s
            .symlink_metadata()
            .map(|m| m.is_dir() && !m.file_type().is_symlink())
            .unwrap_or(false);
        if inside(dst, s) {
            r.errs
                .push(format!("{}: cannot move a folder into itself", s.display()));
            continue;
        }
        let target = unique_path(
            dst,
            s.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .as_deref()
                .unwrap_or("?"),
        );
        // rename within one filesystem, else copy+delete like shutil.move
        let res = match fs::rename(s, &target) {
            Ok(()) => Ok::<(), String>(()),
            Err(_) => {
                let c = if is_dir {
                    copy_tree(s, &target, &mut |inc| progress(inc))
                } else {
                    copy_file(s, &target, progress)
                };
                let res = c.and_then(|()| {
                    if is_dir {
                        fs::remove_dir_all(s).map_err(|e| e.to_string())
                    } else {
                        fs::remove_file(s).map_err(|e| e.to_string())
                    }
                });
                if res.is_err() {
                    let _ = fs::remove_file(&target);
                }
                res
            }
        };
        match res {
            Ok(()) => {
                r.files += 1;
                r.targets.push((s.clone(), target));
            }
            Err(e) => r.errs.push(format!("{}: {}", s.display(), e)),
        }
    }
    r
}

fn data_home() -> PathBuf {
    std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".local/share"))
}

fn trash_fallback(p: &Path) -> Result<(), String> {
    let base = data_home().join("Trash");
    fs::create_dir_all(base.join("files")).map_err(|e| e.to_string())?;
    fs::create_dir_all(base.join("info")).map_err(|e| e.to_string())?;
    let dest = unique_path(
        &base.join("files"),
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .as_deref()
            .unwrap_or("?"),
    );
    let date = {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        // minimal ISO format from seconds
        let secs = t.as_secs() as i64;
        // cmd `date` is fine on linux: keep simple, use libc-free math via std
        format_iso(secs)
    };
    let info = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        p.canonicalize()
            .map(|c| c.display().to_string())
            .unwrap_or(p.display().to_string()),
        date
    );
    let ipath = base.join("info").join(format!(
        "{}.trashinfo",
        dest.file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    ));
    fs::write(&ipath, info).map_err(|e| e.to_string())?;
    fs::rename(p, &dest).map_err(|e| e.to_string())
}

/// tiny ISO-8601 local time formatter (no chrono dependency)
fn format_iso(secs: i64) -> String {
    // days since epoch → civil date (Howard Hinnant's algorithm)
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}")
}

pub fn trash_paths(paths: &[PathBuf]) -> Vec<String> {
    journal_trash(paths).0
}

/// trash + report where each item went (for the undo journal)
pub fn journal_trash(paths: &[PathBuf]) -> (Vec<String>, Vec<(PathBuf, PathBuf)>) {
    let mut moved = vec![];
    let (errs, _watch) = trash_impl(paths, Some(&mut moved));
    (errs, moved)
}

pub fn trash_impl(
    paths: &[PathBuf],
    moved: Option<&mut Vec<(PathBuf, PathBuf)>>,
) -> (Vec<String>, Vec<(PathBuf, PathBuf)>) {
    let mut errs = vec![];
    let mut m = Vec::new();
    for p in paths {
        let want = p.display().to_string();
        let r = Command::new("gio")
            .arg("trash")
            .arg("--")
            .arg(p)
            .output()
            .map(|o| {
                if o.status.success() {
                    Ok(())
                } else {
                    Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
                }
            })
            .unwrap_or(Err("gio missing".into()));
        let trashed = data_home().join("Trash").join("files").join(
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "?".into()),
        );
        if r.is_err() {
            match trash_fallback(p) {
                Ok(()) => m.push((p.clone(), trashed)),
                Err(e) => {
                    errs.push(format!("{}: {}", p.display(), e));
                    continue;
                }
            }
        } else {
            // gio doesn't say where it landed; the fallback name is what it was
            // called — but only record when our own naming applies
            m.push((p.clone(), trashed));
        }
        let _ = want;
    }
    if let Some(out) = moved {
        *out = m.clone();
    }
    (errs, m)
}

pub fn delete_paths(paths: &[PathBuf]) -> Vec<String> {
    let mut errs = vec![];
    for p in paths {
        let meta = fs::symlink_metadata(p);
        let is_dir = meta
            .as_ref()
            .map(|m| m.is_dir() && !m.file_type().is_symlink())
            .unwrap_or(false);
        let r = if is_dir {
            fs::remove_dir_all(p)
        } else {
            fs::remove_file(p)
        };
        if let Err(e) = r {
            errs.push(format!("{}: {}", p.display(), e));
        }
    }
    errs
}

pub fn rename_path(p: &Path, new: &str) -> Result<PathBuf, String> {
    let new = new.trim();
    if new.is_empty() || new.contains('/') {
        return Err("invalid name".into());
    }
    let t = p.with_file_name(new);
    if lexists(&t) {
        return Err(format!("{new} already exists"));
    }
    fs::rename(p, &t).map_err(|e| e.to_string()).map(|_| t)
}

pub fn make_dir(folder: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() || name.contains('/') {
        return Err("invalid name".into());
    }
    let d = unique_path(folder, name);
    fs::create_dir(&d).map_err(|e| e.to_string()).map(|_| d)
}

pub fn make_file(folder: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() || name.contains('/') {
        return Err("invalid name".into());
    }
    let f = unique_path(folder, name);
    fs::File::create(&f).map_err(|e| e.to_string()).map(|_| f)
}

/// path under ~/.local/share/Trash/files that the XDG trashinfo in info/ points at
pub fn trash_files_dir() -> PathBuf {
    data_home().join("Trash").join("files")
}

/// undo one trashed item: read its .trashinfo, move back, drop the info file
pub fn restore_from_trash(trashed: &Path) -> Result<(), String> {
    let name = trashed
        .file_name()
        .ok_or("bad trash path")?
        .to_string_lossy()
        .into_owned();
    let info = data_home()
        .join("Trash")
        .join("info")
        .join(format!("{name}.trashinfo"));
    let text = fs::read_to_string(&info).unwrap_or_default();
    let orig = text
        .lines()
        .find_map(|l| l.strip_prefix("Path="))
        .map(uriloc::decode)
        .ok_or("no trashinfo path")?;
    if lexists(&orig) {
        return Err(format!("{} already exists", orig.display()));
    }
    if let Some(par) = orig.parent() {
        let _ = fs::create_dir_all(par);
    }
    fs::rename(trashed, &orig).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(&info);
    Ok(())
}

/// undo fox moves: rename back (used for cut-paste and drag-drop recovery)
pub fn undo_move_back(orig: &Path, new: &Path) -> Result<(), String> {
    if !lexists(new) {
        return Err(format!("{} is gone", new.display()));
    }
    if let Some(par) = orig.parent() {
        let _ = fs::create_dir_all(par);
    }
    fs::rename(new, orig).map_err(|e| e.to_string())
}

/// percent-decode a uri path from .trashinfo files
mod uriloc {
    pub fn decode(s: &str) -> std::path::PathBuf {
        let b = s.as_bytes();
        let mut out = Vec::with_capacity(b.len());
        let mut i = 0;
        while i < b.len() {
            match b[i] {
                b'%' if i + 2 < b.len() => {
                    if let (Some(h), Some(l)) = (
                        b[i + 1..].first().and_then(|c| (*c as char).to_digit(16)),
                        b[i + 2..].first().and_then(|c| (*c as char).to_digit(16)),
                    ) {
                        out.push(h as u8 * 16 + l as u8);
                        i += 3;
                        continue;
                    }
                    out.push(b[i]);
                    i += 1;
                }
                c => {
                    out.push(c);
                    i += 1;
                }
            }
        }
        String::from_utf8_lossy(&out).into_owned().into()
    }
}

pub fn which(prog: &str) -> bool {
    Command::new("which")
        .arg(prog)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unique_naming() {
        let dir = std::env::temp_dir().join(format!("vg-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let a = dir.join("notes.txt");
        fs::write(&a, "x").unwrap();
        assert_eq!(unique_path(&dir, "notes.txt"), dir.join("notes (2).txt")); // python parity
        fs::write(dir.join("t.tar.gz"), "").unwrap();
        assert_eq!(unique_path(&dir, "t.tar.gz"), dir.join("t (2).tar.gz"));
        let _ = fs::remove_dir_all(&dir);
    }
    #[test]
    fn iso_format() {
        assert_eq!(format_iso(0), "1970-01-01T00:00:00");
        assert_eq!(format_iso(1700000000), "2023-11-14T22:13:20");
    }
}
