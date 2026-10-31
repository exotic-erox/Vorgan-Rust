//! Data model: entries, locations, file kinds, icons. No UI code here.

use std::path::{Path, PathBuf};

pub const ARC_EXTS: [&str; 16] = [
    ".zip", ".tar", ".tar.gz", ".tgz", ".tar.bz2", ".tbz2", ".tar.xz", ".txz", ".tar.zst", ".tzst",
    ".7z", ".rar", ".iso", ".cpio", ".jar", ".cbz",
];
// .whl too
pub const ARC_EXTRA: [&str; 1] = [".whl"];
pub const IMG_EXTS: [&str; 9] = [
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tif", ".tiff", ".ico",
];
pub const AUDIO_EXTS: [&str; 7] = [".mp3", ".flac", ".ogg", ".opus", ".wav", ".m4a", ".aac"];
pub const VIDEO_EXTS: [&str; 6] = [".mp4", ".mkv", ".webm", ".avi", ".mov", ".m4v"];
pub const CODE_EXTS: [&str; 32] = [
    ".py", ".js", ".ts", ".tsx", ".jsx", ".rs", ".go", ".c", ".h", ".cpp", ".hpp", ".java", ".kt",
    ".lua", ".sh", ".fish", ".bash", ".zsh", ".html", ".css", ".scss", ".json", ".toml", ".yaml",
    ".yml", ".sql", ".php", ".rb", ".cs", ".vue", ".svelte", ".ini",
];
pub const TEXT_EXTS: [&str; 9] = [
    ".txt", ".md", ".rst", ".log", ".csv", ".tsv", ".conf", ".env", ".cfg",
];
pub const CONFIG_NAMES: [&str; 8] = [
    "config",
    "hyprland.conf",
    ".gitconfig",
    ".bashrc",
    ".zshrc",
    "makefile",
    "dockerfile",
    ".env",
];

pub const KIND_DIR: usize = 0;
pub const KIND_ARCHIVE: usize = 1;

pub const KIND_COUNT: usize = 10;
pub const TAGS: [&str; 10] = [
    "DIR", "ARC", "IMG", "AUD", "VID", "SRC", "PDF", "TXT", "CFG", "---",
];
pub const ICONS_NERD: [&str; 10] = [
    "\u{f07b}", "\u{f1c6}", "\u{f1c5}", "\u{f1c7}", "\u{f1c8}", "\u{f1c9}", "\u{f1c1}", "\u{f0f6}",
    "\u{f013}", "\u{f15b}",
];
pub const ICONS_PLAIN: [&str; 10] = ["▸", "▤", "▣", "♪", "▶", "◆", "▥", "≡", "⚙", "·"];

pub fn is_archive_name(name: &str) -> bool {
    let low = name.to_lowercase();
    ARC_EXTS
        .iter()
        .chain(ARC_EXTRA.iter())
        .any(|e| low.ends_with(e))
}

pub fn strip_arc_ext(name: &str) -> String {
    let low = name.to_lowercase();
    let mut best: Option<&str> = None;
    for e in ARC_EXTS.iter().chain(ARC_EXTRA.iter()) {
        if low.ends_with(e) && best.map(|b| e.len() > b.len()).unwrap_or(true) {
            best = Some(e);
        }
    }
    match best {
        Some(e) => name[..name.len() - e.len()].to_string(),
        None => name.to_string(),
    }
}

pub fn kind_of(name: &str, is_dir: bool) -> usize {
    if is_dir {
        return KIND_DIR;
    }
    if is_archive_name(name) {
        return KIND_ARCHIVE;
    }
    let ext: String = match Path::new(name).extension() {
        Some(e) => format!(".{}", e.to_string_lossy().to_lowercase()),
        None => String::new(),
    };
    let low = name.to_lowercase();
    let has = |set: &[&str]| set.contains(&ext.as_str());
    if has(&IMG_EXTS) {
        return 2;
    }
    if has(&AUDIO_EXTS) {
        return 3;
    }
    if has(&VIDEO_EXTS) {
        return 4;
    }
    if ext == ".pdf" {
        return 6;
    }
    if CONFIG_NAMES.contains(&low.as_str()) {
        return 8;
    }
    if has(&CODE_EXTS) || low.ends_with(".zsh") {
        return 5;
    }
    if has(&TEXT_EXTS) {
        return 7;
    }
    9
}

pub fn human_size(n: u64) -> String {
    let mut x = n as f64;
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    for (i, unit) in UNITS.iter().enumerate() {
        if x < 1024.0 || i == UNITS.len() - 1 {
            let unit = *unit;
            return if unit == "B" || x >= 100.0 {
                format!("{}{}", x as i64, unit)
            } else {
                format!("{:.1}{}", x, unit)
            };
        }
        x /= 1024.0;
    }
    format!("{n}B")
}

/// local UTC offset in seconds, read once from `date +%z`; UTC when it fails
pub fn local_offset() -> i64 {
    static OFF: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *OFF.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .and_then(|s| {
                let s = s.trim(); // "+HHMM" or "-HHMM"
                if s.len() != 5 {
                    return None;
                }
                let sign = if s.starts_with('-') { -1 } else { 1 };
                let h: i64 = s[1..3].parse().ok()?;
                let m: i64 = s[3..5].parse().ok()?;
                Some(sign * (h * 3600 + m * 60))
            })
            .unwrap_or(0)
    })
}

/// "YYYY-MM-DD HH:MM" in local time, no chrono dependency
pub fn fmt_local_time(secs: i64) -> String {
    let s = secs + local_offset();
    let days = s.div_euclid(86400);
    let rem = s.rem_euclid(86400);
    let (yy, mm, dd) = civil_date(days);
    format!(
        "{yy:04}-{mm:02}-{dd:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// days since epoch → year, month, day (Howard Hinnant's civil algorithm)
pub fn civil_date(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// ls-style permission string, 'd'/'l'/'-' prefix inside the ten chars
pub fn fmt_perms(mode: u32) -> String {
    if mode == 0 {
        return String::new();
    } // python shows nothing without a mode
    let table: [(u32, char); 9] = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    let mut out = String::new();
    for (mask, ch) in table {
        out.push(if mode & mask != 0 { ch } else { '-' });
    }
    // the file type is a 4-bit field, not independent flags (a regular file
    // is 0o100000, which overlaps the symlink bits)
    let kind = match mode & 0o170000 {
        0o040000 => 'd',
        0o120000 => 'l',
        _ => '-',
    };
    format!("{kind}{out}")
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// mtime as unix seconds
    pub mtime: i64,
    pub mode: u32,
    pub link: bool,
    pub kind: usize,
    pub path: Option<PathBuf>, // real file on disk
    pub arc: Option<PathBuf>,  // archive holding this entry
    pub inner: String,         // path inside the archive
}

impl Entry {
    pub fn new(name: impl Into<String>, is_dir: bool) -> Entry {
        Entry {
            name: name.into(),
            is_dir,
            size: 0,
            mtime: 0,
            mode: 0,
            link: false,
            kind: 0,
            path: None,
            arc: None,
            inner: String::new(),
        }
    }
    pub fn hidden(&self) -> bool {
        self.name.starts_with('.')
    }
    pub fn virtual_(&self) -> bool {
        self.arc.is_some()
    }
}

/// A place you can be: a real folder, or a folder inside an archive.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Loc {
    pub path: PathBuf,
    pub arc: Option<PathBuf>,
    pub inner: String,
}

impl Loc {
    pub fn real(path: PathBuf) -> Loc {
        Loc {
            path,
            arc: None,
            inner: String::new(),
        }
    }
    pub fn in_arc(path: PathBuf, arc: PathBuf, inner: impl Into<String>) -> Loc {
        Loc {
            path,
            arc: Some(arc),
            inner: inner.into(),
        }
    }
    pub fn parent(&self) -> Option<Loc> {
        if let Some(a) = &self.arc {
            return match self.inner.rfind('/') {
                Some(i) => Some(Loc::in_arc(
                    self.path.clone(),
                    a.clone(),
                    self.inner[..i].to_string(),
                )),
                None => Some(Loc::real(
                    a.parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| PathBuf::from("/")),
                )),
            };
        }
        if let Some(p) = self.path.parent() {
            if p != self.path {
                return Some(Loc::real(p.to_path_buf()));
            }
        }
        None
    }
    pub fn child(&self, e: &Entry) -> Loc {
        // caller passes entry mostly from this loc's listing; archive entries keep their inner path
        if let Some(a) = &self.arc {
            return Loc::in_arc(self.path.clone(), a.clone(), e.inner.clone());
        }
        let p = e.path.clone().unwrap_or_else(|| self.path.join(&e.name));
        if e.kind == KIND_ARCHIVE && !e.virtual_() {
            return Loc::in_arc(self.path.clone(), p, "");
        }
        Loc::real(p)
    }
    pub fn crumbs(&self) -> Vec<(String, Loc)> {
        let mut out = Vec::new();
        let parts: Vec<std::ffi::OsString> = self
            .path
            .components()
            .map(|c| c.as_os_str().to_os_string())
            .collect();
        for (i, part) in parts.iter().enumerate() {
            let loc = Loc::real(parts[..=i].iter().collect());
            let disp = if i == 0 {
                "/".to_string()
            } else {
                part.to_string_lossy().into_owned()
            };
            out.push((disp, loc));
        }
        if self.path.as_os_str().is_empty() && !out.is_empty() {
            out.pop();
        }
        if let Some(a) = &self.arc {
            out.push((
                a.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                Loc::in_arc(self.path.clone(), a.clone(), ""),
            ));
            let mut acc = String::new();
            for seg in self.inner.split('/').filter(|s| !s.is_empty()) {
                acc = if acc.is_empty() {
                    seg.into()
                } else {
                    format!("{acc}/{seg}")
                };
                out.push((
                    seg.to_string(),
                    Loc::in_arc(self.path.clone(), a.clone(), acc.clone()),
                ));
            }
        }
        out
    }
    pub fn display(&self) -> String {
        let s = self
            .arc
            .as_ref()
            .map(|a| a.display().to_string())
            .unwrap_or_else(|| self.path.display().to_string());
        if self.arc.is_some() && !self.inner.is_empty() {
            format!("{s}::{}", self.inner)
        } else {
            s
        }
    }
    /// name of the current folder (for the parent list highlight)
    pub fn folder_name(&self) -> String {
        if let Some(a) = &self.arc {
            if self.inner.is_empty() {
                return a
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
            }
            return self
                .inner
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
        }
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".into())
    }
}

pub fn sort_entries(entries: Vec<Entry>, key: &str, reverse: bool) -> Vec<Entry> {
    let mut out = entries;
    out.sort_by(|a, b| {
        let dir = |e: &Entry| e.is_dir;
        if dir(a) != dir(b) {
            return if dir(a) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            };
        };

        match key {
            "size" => b
                .size
                .cmp(&a.size)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            "date" => b
                .mtime
                .cmp(&a.mtime)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            "type" => a
                .kind
                .cmp(&b.kind)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });
    if reverse {
        // python reverses within dirs and within files, keeping dirs first
        let mut dirs: Vec<Entry> = out.iter().filter(|e| e.is_dir).cloned().collect();
        let mut files: Vec<Entry> = out.into_iter().filter(|e| !e.is_dir).collect();
        dirs.reverse();
        files.reverse();
        dirs.extend(files);
        out = dirs;
    }
    out
}

pub fn fuzzy(name: &str, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let n = name.to_lowercase();
    let q = q.to_lowercase();
    if n.contains(&q) {
        return true;
    }
    let mut it = n.chars();
    q.chars().all(|ch| it.any(|c| c == ch))
}

#[cfg(test)]
mod tests {
    #[test]
    fn perms_match_stat_filemode() {
        assert_eq!(fmt_perms(0o100644), "-rw-r--r--");
        assert_eq!(fmt_perms(0o040755), "drwxr-xr-x");
        assert_eq!(fmt_perms(0o120777), "lrwxrwxrwx");
        assert_eq!(fmt_perms(0), "");
    }

    use super::*;
    #[test]
    fn kinds_and_suffixes() {
        assert_eq!(kind_of("a.zip", false), 1);
        assert_eq!(kind_of("A.WHL", false), 1);
        assert_eq!(strip_arc_ext("x.tar.zst"), "x");
        assert_eq!(kind_of("Makefile", false), 8);
        assert_eq!(kind_of("d", true), 0);
        assert_eq!(kind_of("song.flac", false), 3);
    }
    #[test]
    fn sizes_and_loc() {
        assert_eq!(human_size(1536), "1.5K");
        assert_eq!(human_size(7), "7B");
        let l = Loc::in_arc(
            PathBuf::from("/a/s.tar"),
            PathBuf::from("/a/s.tar"),
            "sub/dir",
        );
        assert_eq!(l.parent().unwrap().inner, "sub");
        assert_eq!(l.display(), "/a/s.tar::sub/dir");
    }
}
