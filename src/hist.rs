//! fish integration: frequent folders straight from the fish history file.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::theme::home;

fn history_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".local/share"));
    base.join("fish").join("fish_history")
}

/// most-used folders, read from fish history (paths: blocks and cd commands)
pub fn frecent_dirs(limit: usize) -> Vec<PathBuf> {
    let text = match fs::read_to_string(history_path()) {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut in_paths = false;
    for line in text.lines() {
        if let Some(cmd) = line.strip_prefix("- cmd:") {
            in_paths = false;
            if let Some(arg) = cmd.trim().strip_prefix("cd ") {
                let arg = arg.split_whitespace().next().unwrap_or_default();
                if !arg.is_empty() {
                    *counts.entry(expand(arg)).or_default() += 1;
                }
            }
        } else if line.trim() == "paths:" {
            in_paths = true;
        } else if in_paths && line.starts_with("    - ") {
            *counts
                .entry(expand(line.trim().trim_start_matches("- ")))
                .or_default() += 1;
        } else if !line.starts_with(' ') {
            in_paths = false;
        }
    }
    let mut ranked: Vec<(String, u32)> = counts.into_iter().collect();
    ranked.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    let mut out: Vec<PathBuf> = Vec::new();
    for (p, _) in ranked.iter().take(limit * 4) {
        let pp = Path::new(p);
        let d = if pp.is_dir() {
            PathBuf::from(p)
        } else {
            match pp.parent() {
                Some(par) => par.to_path_buf(),
                None => continue,
            }
        };
        if d.is_dir() && !out.contains(&d) && d != home() {
            out.push(d);
        }
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn expand(p: &str) -> String {
    if p == "~" {
        return home().display().to_string();
    }
    if let Some(rest) = p.strip_prefix("~/") {
        return home().join(rest).display().to_string();
    }
    p.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn counts_cd() {
        // frecent_dirs only keeps folders that exist, so make real ones
        let d = std::env::temp_dir().join(format!("vg-frent-{}", std::process::id()));
        let docs = d.join("Documents");
        let _ = fs::create_dir_all(&docs);
        let _ = fs::create_dir_all(d.join("fish"));
        let log = format!(
            "- cmd: cd {0}\n  when: 1\n  paths:\n    - {0}/\n- cmd: ls\n  when: 2\n",
            docs.display()
        );
        fs::write(d.join("fish").join("fish_history"), log).unwrap();
        std::env::set_var("XDG_DATA_HOME", &d); // history_path() reads this
        let out = frecent_dirs(10);
        let _ = fs::remove_dir_all(&d);
        assert!(out.iter().any(|p| p.ends_with("Documents")), "{out:?}");
    }
}
