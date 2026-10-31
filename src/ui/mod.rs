//! The app: panes + trail/tools/status bars, the event loop, all actions
//! from the python app, overlays and the graph screen. Workers do heavy
//! loads over channels; the loop stays responsive.

pub mod graphview;
pub mod list;
pub mod overlay;
pub mod preview;

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use ratatui::Terminal;
use unicode_width::UnicodeWidthStr;

use crate::archive;
use crate::graph::Graph;
use crate::hist::frecent_dirs;
use crate::model::{fuzzy, human_size, sort_entries, strip_arc_ext, Entry, Loc};
use crate::ops as fsops;
use crate::preview::build_preview;
use crate::sixel;
use crate::state::{parse_key, Config, State, UndoOp, KEYMAP};
use crate::theme::{mix, Palette};

pub const SORTS: [&str; 4] = ["name", "size", "date", "type"];

enum Msg {
    Loaded {
        key: String,
        entries: Vec<Entry>,
        parents: Vec<Entry>,
        err: Option<(String, bool)>,
    },
    Preview {
        seq: u64,
        pv: crate::preview::Preview,
    },
    Progress {
        job: u64,
        done: u64,
    },
    Done {
        job: u64,
        res: BgRes,
    },
    Found {
        hits: Vec<Entry>,
    },
}

enum BgRes {
    Finished(Vec<(PathBuf, PathBuf)>), // undo pairs (trash/compress-rm)
    Moved(fsops::CopyMove, bool),      // moved=true → journal "move" undo
    Failed(String),
    NeedPassword(PathBuf),
}

#[derive(Clone)]
struct Job {
    id: u64,
    label: String,
    totals: u64,
    kind: JobKind,
    done_text: String,
    select: Option<String>,
    after: Option<PathBuf>,
}
#[derive(Clone)]
enum JobKind {
    CopyMove(Vec<PathBuf>, PathBuf, bool),
    Trash(Vec<PathBuf>),
    Delete(Vec<PathBuf>),
    Extract(
        Vec<(PathBuf, Option<Vec<String>>)>,
        PathBuf,
        bool,
        bool,
        BTreeMap<String, String>,
    ),
    Compress(PathBuf, PathBuf, Vec<String>, String, Option<String>, bool),
    Find(Loc, String),
}

struct Click {
    x0: u16,
    x1: u16,
    y: u16,
    act: String,
}

pub struct App {
    pub cfg: Config,
    pub state: State,
    pub pal: Palette,
    scheme: Option<PathBuf>,
    pub loc: Loc,
    entries: Vec<Entry>,
    pub filter: String,
    pub show_hidden: bool,
    sort_idx: usize,
    clip: Option<(&'static str, Vec<PathBuf>)>,
    hint: Option<String>,
    pending: Option<String>,
    msg: String,
    msg_until: Instant,
    passwords: BTreeMap<String, String>,
    want: Option<String>,
    anchor: Option<usize>,
    last_click: Option<(usize, Instant)>,
    pub main: list::ListView,
    pub parent_list: list::ListView,
    pct: (i64, i64),
    pub pv: preview::PreviewPane,
    pv_seq: u64,
    pv_due: Option<Instant>,
    pub overlay: Option<overlay::Overlay>,
    pub graph: Option<graphview::GraphView>,
    pub quit: Option<Option<String>>,
    input_focus: bool,
    drag: Option<(Vec<Entry>, bool)>,
    drag_pending: Option<(usize, u16, u16)>,
    sizer: Option<(u8, u16, u16, u16)>,
    clicks: Vec<Click>,
    geo: Option<Geo>,
    graph_pane: Rect,
    pending_six: Option<preview::SixReq>,
    menu_rect: Rect,
    pal_mtime: Option<std::time::SystemTime>,
    tx: std::sync::mpsc::Sender<Msg>,
    rx: Receiver<Msg>,
    next_job: u64,
    running_job: Option<(u64, String, u64)>,
    job_done: u64,
    cur_meta: Option<(String, Option<String>, Option<Loc>)>,
    retry_job: Option<Job>,
    dir_mtime: i64,
    last_watch: Instant,
    last_tick: Instant,
    keys: BTreeMap<String, &'static str>,
    dirty: bool,
}

#[derive(Clone, Copy)]
struct Geo {
    cols: Rect,
    parent: Rect,
    main: Rect,
    prev: Rect,
    sz0: u16,
    sz1: u16,
}

const WHEEL_STEP: i32 = 2;

// ---------- key canonicalisation ----------

fn label_of(code: KeyCode) -> String {
    match code {
        KeyCode::Char(ch) => ch.to_string(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Tab => "tab".into(),
        KeyCode::Backspace => "backspace".into(),
        KeyCode::Esc => "escape".into(),
        KeyCode::Left => "left".into(),
        KeyCode::Right => "right".into(),
        KeyCode::Up => "up".into(),
        KeyCode::Down => "down".into(),
        KeyCode::Home => "home".into(),
        KeyCode::End => "end".into(),
        KeyCode::Delete => "delete".into(),
        KeyCode::Insert => "insert".into(),
        KeyCode::PageUp => "pgup".into(),
        KeyCode::PageDown => "pgdn".into(),
        KeyCode::F(n) => format!("f{n}"),
        _ => String::new(),
    }
}

fn canon(code: KeyCode, m: KeyModifiers) -> String {
    let pre = if m.contains(KeyModifiers::CONTROL) {
        "ctrl+"
    } else if m.contains(KeyModifiers::ALT) {
        "alt+"
    } else if m.contains(KeyModifiers::SHIFT) && !matches!(code, KeyCode::Char(_)) {
        "shift+"
    } else {
        ""
    };
    format!("{pre}{}", label_of(code))
}

fn build_keys(cfg: &Config) -> BTreeMap<String, &'static str> {
    let mut m = BTreeMap::new();
    for (action, keys) in KEYMAP.iter() {
        for k in *keys {
            if let Some((code, mods)) = parse_key(k) {
                m.insert(canon(code, mods), *action);
            }
        }
    }
    for (action, keys) in &cfg.key_overrides {
        let Some(d0) = KEYMAP
            .iter()
            .find(|(a, _)| *a == action.as_str())
            .map(|(a, _)| *a)
        else {
            continue;
        };
        for k in keys {
            if let Some((code, mods)) = parse_key(k) {
                m.insert(canon(code, mods), d0);
            }
        }
    }
    m
}

fn cpx(c2: crate::theme::Col) -> Style {
    Style::new().fg(Color::Rgb(c2.0, c2.1, c2.2))
}

fn bgp(c2: crate::theme::Col) -> Style {
    Style::new().bg(Color::Rgb(c2.0, c2.1, c2.2))
}

fn chip(text: &str, fg: crate::theme::Col, bg: crate::theme::Col, bold: bool) -> Span<'static> {
    Span::styled(
        text.to_string(),
        Style::new()
            .fg(Color::Rgb(fg.0, fg.1, fg.2))
            .bg(Color::Rgb(bg.0, bg.1, bg.2))
            .add_modifier(if bold {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    )
}

fn canonical(p: &Path) -> PathBuf {
    p.canonicalize().unwrap_or_else(|_| p.to_path_buf())
}

fn mtime_of(p: &Path) -> i64 {
    std::fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn saved_pct(state: &State) -> (i64, i64) {
    (
        state.cols.get("parent").copied().unwrap_or(30),
        state.cols.get("preview").copied().unwrap_or(36),
    )
}

fn homey(p: &Path) -> String {
    let home = crate::theme::home();
    let s = p.display().to_string();
    s.strip_prefix(&home.display().to_string())
        .map(|r| format!("~{r}"))
        .unwrap_or(s)
}

impl App {
    pub fn new(
        cfg: Config,
        state: State,
        pal: Palette,
        start: PathBuf,
        scheme: Option<PathBuf>,
    ) -> App {
        let (tx, rx) = mpsc::channel();
        let show_hidden = cfg.show_hidden;
        let sort_idx = SORTS.iter().position(|s| *s == cfg.sort).unwrap_or(0);
        let select = if start.is_file() {
            Some(
                start
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap(),
            )
        } else {
            None
        };
        let start = if start.is_file() {
            start
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            start
        };
        App {
            keys: build_keys(&cfg),
            cfg,
            pal,
            scheme,
            loc: Loc::real(canonical(&start)),
            entries: vec![],
            filter: String::new(),
            show_hidden,
            sort_idx,
            clip: None,
            hint: None,
            pending: None,
            msg: String::new(),
            msg_until: Instant::now(),
            passwords: BTreeMap::new(),
            want: select,
            anchor: None,
            last_click: None,
            main: list::ListView::new(true),
            parent_list: list::ListView::new(false),
            pct: saved_pct(&state),
            pv: preview::PreviewPane::default(),
            pv_seq: 0,
            pv_due: None,
            overlay: None,
            graph: None,
            quit: None,
            input_focus: false,
            drag: None,
            drag_pending: None,
            sizer: None,
            clicks: vec![],
            geo: None,
            graph_pane: Rect::default(),
            pending_six: None,
            menu_rect: Rect::default(),
            pal_mtime: None,
            tx,
            rx,
            next_job: 1,
            running_job: None,
            job_done: 0,
            cur_meta: None,
            retry_job: None,
            dir_mtime: 0,
            last_watch: Instant::now(),
            last_tick: Instant::now(),
            state,
            dirty: true,
        }
    }

    fn notify(&mut self, text: &str) {
        self.msg = text.to_string();
        self.msg_until = Instant::now() + Duration::from_secs(4);
        self.dirty = true;
    }

    fn err(&mut self, text: &str) {
        self.notify(&format!("✗ {text}"));
    }

    fn loc_key(loc: &Loc) -> String {
        format!(
            "{}\u{1}{}\u{1}{}",
            loc.path.display(),
            loc.arc
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_default(),
            loc.inner
        )
    }

    fn pw_for(&self, loc: &Loc) -> Option<String> {
        loc.arc
            .as_ref()
            .and_then(|a| self.passwords.get(&a.display().to_string()).cloned())
    }

    // ---------- loading ----------

    pub fn reload(&mut self) {
        self.reload_sel(None)
    }

    fn reload_sel(&mut self, keep: Option<String>) {
        self.want = Some(
            keep.or_else(|| self.main.entry().map(|e| e.name.clone()))
                .unwrap_or_default(),
        );
        let loc = self.loc.clone();
        let key = Self::loc_key(&loc);
        let pw = self.pw_for(&loc);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut entries = vec![];
            let mut err: Option<(String, bool)> = None;
            if let Some(arc) = loc.arc.clone() {
                if let Err(er) = archive::get_index(&arc, pw.as_deref()) {
                    err = Some((er.msg, er.need_password));
                }
            }
            if err.is_none() {
                if loc.arc.is_some() {
                    entries = archive::list_loc(&loc, pw.as_deref())
                        .map_err(|e| (e.msg, e.need_password))
                        .unwrap_or_else(|e| {
                            err = Some(e);
                            vec![]
                        });
                } else {
                    entries = fsops::list_real(&loc.path).unwrap_or_else(|m| {
                        err = Some((m, false));
                        vec![]
                    });
                }
            }
            let mut parents = vec![];
            if let Some(par) = loc.parent() {
                if err.is_none() {
                    if par.arc.is_some() {
                        parents = archive::list_loc(&par, None).unwrap_or_default();
                    } else {
                        parents = fsops::list_real(&par.path).unwrap_or_default();
                    }
                    parents.retain(|e| e.is_dir);
                }
            }
            let _ = tx.send(Msg::Loaded {
                key,
                entries,
                parents,
                err,
            });
        });
        self.dirty = true;
    }

    // ---------- viewport updates ----------

    fn set_list(lv: &mut list::ListView, es: Vec<Entry>, keep: Option<String>) {
        let cur = keep.or_else(|| lv.view.get(lv.cursor).map(|e| e.name.clone()));
        lv.view = es;
        lv.cursor = cur
            .and_then(|n| lv.view.iter().position(|e| e.name == n))
            .unwrap_or(0);
        lv.reveal();
    }

    fn refresh_view(&mut self, keep: Option<String>) {
        let cur = keep.or_else(|| self.main.entry().map(|e| e.name.clone()));
        let mut es: Vec<Entry> = self
            .entries
            .iter()
            .filter(|e| {
                (self.show_hidden || !e.name.starts_with('.')) && fuzzy(&e.name, &self.filter)
            })
            .cloned()
            .collect();
        es = sort_entries(es, SORTS[self.sort_idx], false);
        Self::set_list(&mut self.main, es, cur);
        self.pv_due = Some(Instant::now() + Duration::from_millis(90));
        self.dirty = true;
    }

    pub fn goto(&mut self, loc: Loc) {
        self.goto_sel(loc, None)
    }

    fn goto_sel(&mut self, loc: Loc, select: Option<String>) {
        self.loc = loc;
        self.want = select;
        self.filter.clear();
        self.hint = None;
        self.pending = None;
        self.main.selected.clear();
        self.main.hints = None;
        self.input_focus = false;
        self.dirty = true;
        self.reload();
    }

    fn move_cur(&mut self, d: i32) {
        let n = self.main.view.len() as i32;
        if n == 0 {
            return;
        }
        self.main.cursor = (self.main.cursor as i32 + d).clamp(0, n - 1) as usize;
        self.main.reveal();
        self.after_move();
    }

    fn after_move(&mut self) {
        self.pv_due = Some(Instant::now() + Duration::from_millis(90));
        self.dirty = true;
    }

    fn act_enter(&mut self) {
        let Some(e) = self.main.entry().cloned() else {
            return;
        };
        if e.is_dir || (e.kind == crate::model::KIND_ARCHIVE && !e.virtual_()) {
            self.goto(self.loc.child(&e));
        } else if e.virtual_() {
            self.notify("extract it first (press e)");
        } else if let Some(p) = e.path.clone() {
            self.spawn(&["xdg-open".into(), p.display().to_string()]);
        }
    }

    fn spawn(&mut self, args: &[String]) {
        let _ = Command::new(&args[0])
            .args(&args[1..])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }

    fn act_back(&mut self) {
        if let Some(par) = self.loc.parent() {
            let name = self.loc.folder_name();
            self.goto_sel(par, Some(name));
        }
    }

    // ---------- selection ----------

    fn targets(&self) -> Vec<Entry> {
        if !self.main.selected.is_empty() {
            self.main
                .view
                .iter()
                .filter(|e| self.main.selected.contains(&e.name))
                .cloned()
                .collect()
        } else {
            self.main.entry().cloned().into_iter().collect()
        }
    }

    fn real_targets(&self) -> Vec<Entry> {
        self.targets()
            .into_iter()
            .filter(|e| !e.virtual_())
            .collect()
    }

    // ---------- actions ----------

    pub fn do_action(&mut self, action: &str) {
        match action {
            "down" => self.move_cur(1),
            "up" => self.move_cur(-1),
            "top" => {
                self.main.cursor = 0;
                self.main.reveal();
                self.after_move();
            }
            "bottom" => {
                self.main.cursor = self.main.view.len().saturating_sub(1);
                self.main.reveal();
                self.after_move();
            }
            "enter" => self.act_enter(),
            "back" => self.act_back(),
            "graph" => self.act_graph(),
            "hint" => self.act_hint(),
            "search" => {
                self.input_focus = true;
                self.dirty = true;
            }
            "find" => self.act_find(),
            "hidden" => {
                self.show_hidden = !self.show_hidden;
                self.reload();
            }
            "sort" => {
                self.sort_idx = (self.sort_idx + 1) % SORTS.len();
                self.refresh_view(None);
                self.notify(&format!("sort by {}", SORTS[self.sort_idx]));
            }
            "pin" | "warp" => {
                self.pending = Some(action.to_string());
                self.dirty = true;
            }
            "toggle" => {
                let Some(e) = self.main.entry() else { return };
                let name = e.name.clone();
                if !self.main.selected.remove(&name) {
                    self.main.selected.insert(name);
                }
                self.move_cur(1);
            }
            "selall" => {
                self.main.selected = self
                    .main
                    .view
                    .iter()
                    .map(|e| e.name.clone())
                    .collect::<HashSet<_>>();
                self.after_move();
            }
            "copy" | "cut" => {
                let ts = self.real_targets();
                if ts.is_empty() {
                    self.notify("nothing to take (inside an archive use e to extract)");
                } else {
                    self.clip = Some((
                        if action == "copy" { "copy" } else { "cut" },
                        ts.iter().map(|e| e.path.clone().unwrap()).collect(),
                    ));
                    self.notify(&format!(
                        "{} {} item(s)",
                        if action == "copy" { "copied" } else { "cut" },
                        ts.len()
                    ));
                }
            }
            "paste" => self.act_paste(),
            "rename" => self.form_rename(),
            "newdir" => self.form_new(true),
            "newfile" => self.form_new(false),
            "trash" => self.act_trash(),
            "delete" => self.form_delete(),
            "extract" => self.form_extract(false),
            "extract_here" => self.form_extract(true),
            "compress" => self.form_compress(false),
            "compress_here" => self.form_compress(true),
            "copy_path" => {
                let p = match self.main.entry() {
                    Some(e) if e.path.is_some() => e.path.clone().unwrap(),
                    _ => self.loc.path.clone(),
                };
                if fsops::which("wl-copy") {
                    let _ = Command::new("wl-copy")
                        .arg(p.display().to_string())
                        .output();
                }
                self.notify(&p.display().to_string());
                self.msg_until = Instant::now() + Duration::from_secs(4);
            }
            "terminal" => {
                let term = std::env::var("TERMINAL").unwrap_or_else(|_| "foot".into());
                let d = self.loc.path.display().to_string();
                let args = if term.contains("foot") {
                    vec![term.clone(), "-D".into(), d]
                } else {
                    vec![term.clone(), "--working-directory".into(), d]
                };
                self.spawn(&args);
            }
            "props" => self.form_props(),
            "frecent" => {
                let dirs = frecent_dirs(10);
                if dirs.is_empty() {
                    self.notify("no folder history found yet");
                    return;
                }
                let items: Vec<Option<(String, String, bool)>> = dirs
                    .into_iter()
                    .map(|d| Some((homey(&d), format!("dir:{}", d.display()), true)))
                    .collect();
                self.open_menu(items, 4, 2);
            }
            "refresh" => self.reload(),
            "clear" => {
                if !self.main.selected.is_empty() {
                    self.main.selected.clear();
                    self.after_move();
                } else if !self.filter.is_empty() {
                    self.filter.clear();
                    self.refresh_view(None);
                }
            }
            "help" => self.overlay = Some(overlay::Overlay::Help),
            "undo" => self.act_undo(),
            "quit_cd" => {
                self.quit = Some(Some(self.loc.path.display().to_string()));
            }
            "quit" => {
                self.quit = Some(None);
            }
            "act_hint_top" => {}
            _ => {}
        }
    }

    fn act_paste(&mut self) {
        let Some(mode) = self.clip.take() else {
            self.notify("clipboard is empty");
            return;
        };
        if self.loc.arc.is_some() {
            self.notify("cannot paste inside an archive");
            self.clip = Some(mode);
            return;
        }
        let (m, paths) = (mode.0, mode.1);
        let dst = self.loc.path.clone();
        if m != "cut" {
            self.clip = Some((m, paths.clone()));
        }
        let label = format!(
            "{} {} item(s)",
            if m == "cut" { "moving" } else { "copying" },
            paths.len()
        );
        let done_text = format!(
            "{} {} item(s)",
            if m == "cut" { "moved" } else { "copied" },
            paths.len()
        );
        self.spawn_job(Job {
            id: 0,
            label,
            totals: paths.len() as u64,
            kind: JobKind::CopyMove(paths, dst, m == "cut"),
            done_text,
            select: None,
            after: None,
        });
    }

    fn act_trash(&mut self) {
        let ts = self.real_targets();
        if ts.is_empty() {
            return;
        }
        let paths: Vec<PathBuf> = ts.iter().map(|e| e.path.clone().unwrap()).collect();
        let n = paths.len();
        self.spawn_job(Job {
            id: 0,
            label: format!("moving {n} item(s) to trash"),
            totals: n as u64,
            kind: JobKind::Trash(paths),
            done_text: format!("moved {} item(s) to trash", ts.len()),
            select: None,
            after: None,
        });
    }

    fn act_undo(&mut self) {
        let Some(op) = self.state.pop_undo() else {
            self.notify("nothing to undo");
            return;
        };
        let res = match op.kind.as_str() {
            "trash" => fsops::restore_from_trash(Path::new(&op.b)),
            "move" => fsops::undo_move_back(Path::new(&op.a), Path::new(&op.b)),
            _ => Err("unknown undo kind".into()),
        };
        match res {
            Ok(()) => {
                self.notify(&format!("restored {}", op.a));
                self.reload();
            }
            Err(e) => {
                self.err(&e);
                self.reload();
            }
        }
    }

    fn act_hint(&mut self) {
        let n = self.main.view.len();
        if n == 0 {
            return;
        }
        let h = self
            .geo
            .as_ref()
            .map(|g| g.main.height)
            .unwrap_or(n.min(24) as u16) as usize;
        let rows = n.min(h);
        self.main.hints = Some(
            (0..rows)
                .zip(list::make_tags(rows))
                .collect::<BTreeMap<_, _>>(),
        );
        self.hint = Some(String::new());
        self.dirty = true;
    }

    fn act_graph(&mut self) {
        if self.graph.is_some() {
            return;
        }
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        let (maxn, hidden) = (self.cfg.graph_max_nodes, self.show_hidden);
        let (pal, loc) = (self.pal.clone(), self.loc.clone());
        let g = Graph::new(loc, &mut self.state, pal, hidden, maxn);
        self.graph = Some(graphview::GraphView::new(
            g,
            w,
            (h.saturating_sub(2)).max(4),
        ));
        self.dirty = true;
    }

    fn act_find(&mut self) {
        self.overlay = Some(overlay::Overlay::Form(overlay::Form {
            title: "Search subfolders (F)".into(),
            note: "recursively matches names under the current folder".into(),
            focus: 0,
            fields: vec![overlay::Field::Input {
                label: "Find".into(),
                text: String::new(),
            }],
            action: overlay::FormAction::Search,
        }));
        self.dirty = true;
    }

    // ---------- forms & menus ----------

    fn open_menu(&mut self, items: Vec<Option<(String, String, bool)>>, x: u16, y: u16) {
        self.overlay = Some(overlay::Overlay::Menu {
            items,
            hover: usize::MAX,
            x,
            y,
        });
        self.dirty = true;
    }

    fn form_rename(&mut self) {
        let Some(e) = self.main.entry().cloned() else {
            return;
        };
        if e.virtual_() {
            self.notify("cannot rename inside an archive");
            return;
        }
        self.overlay = Some(overlay::Overlay::Form(overlay::Form {
            title: "Rename".into(),
            note: String::new(),
            focus: 0,
            fields: vec![overlay::Field::Input {
                label: "New name".into(),
                text: e.name.clone(),
            }],
            action: overlay::FormAction::Rename(e.path.clone().unwrap()),
        }));
        self.dirty = true;
    }

    fn form_new(&mut self, dir: bool) {
        if self.loc.arc.is_some() {
            self.notify("archives are read-only here");
            return;
        }
        self.overlay = Some(overlay::Overlay::Form(overlay::Form {
            title: if dir {
                "New folder".into()
            } else {
                "New file".into()
            },
            note: String::new(),
            focus: 0,
            fields: vec![overlay::Field::Input {
                label: "Name".into(),
                text: if dir {
                    "New folder".into()
                } else {
                    "untitled.txt".into()
                },
            }],
            action: if dir {
                overlay::FormAction::NewDir
            } else {
                overlay::FormAction::NewFile
            },
        }));
        self.dirty = true;
    }

    fn form_delete(&mut self) {
        let ts = self.real_targets();
        if ts.is_empty() {
            return;
        }
        let names: Vec<String> = ts.iter().map(|e| e.name.clone()).collect();
        let shown: Vec<String> = names.iter().take(4).cloned().collect();
        let mut note = format!("This cannot be undone: {}", shown.join(", "));
        if names.len() > 4 {
            note.push('…');
        }
        self.overlay = Some(overlay::Overlay::Form(overlay::Form {
            title: "Delete permanently?".into(),
            note,
            focus: 0,
            fields: vec![],
            action: overlay::FormAction::Delete(
                ts.iter().map(|e| e.path.clone().unwrap()).collect(),
            ),
        }));
        self.dirty = true;
    }

    fn ask_password(&mut self, arc: PathBuf) {
        self.overlay = Some(overlay::Overlay::Form(overlay::Form {
            title: "Password required".into(),
            note: arc.display().to_string(),
            focus: 0,
            fields: vec![overlay::Field::Password {
                label: "Password".into(),
                text: String::new(),
            }],
            action: overlay::FormAction::Password(arc),
        }));
        self.dirty = true;
    }

    #[allow(clippy::type_complexity)]
    fn jobs_for(&self) -> (Vec<(PathBuf, Option<Vec<String>>)>, bool) {
        // (jobs, whole_archive) — python's _extract_jobs
        if self.loc.arc.is_some() {
            let ts = self.targets();
            let members = if self.main.selected.is_empty() {
                None
            } else {
                Some(ts.iter().map(|e| e.inner.clone()).collect())
            };
            let arc = self.loc.arc.clone().unwrap();
            let whole = members.is_none();
            return (vec![(arc, members)], whole);
        }
        let ts: Vec<Entry> = self
            .targets()
            .into_iter()
            .filter(|e| e.kind == crate::model::KIND_ARCHIVE)
            .collect();
        (
            ts.iter().map(|e| (e.path.clone().unwrap(), None)).collect(),
            true,
        )
    }

    fn form_extract(&mut self, here: bool) {
        let (jobs, whole) = self.jobs_for();
        if jobs.is_empty() {
            self.notify("select an archive first");
            return;
        }
        let names = jobs
            .iter()
            .map(|(a, _)| {
                a.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(", ");
        if here {
            let jobs = jobs.clone();
            self.run_extract(jobs, self.loc.path.clone(), whole, false);
            return;
        }
        self.overlay = Some(overlay::Overlay::Form(overlay::Form {
            title: "Extract".into(),
            note: format!(
                "{names}{}",
                if whole { "" } else { "  (selected items only)" }
            ),
            focus: 0,
            fields: vec![
                overlay::Field::Input {
                    label: "Extract to".into(),
                    text: self.loc.path.display().to_string(),
                },
                overlay::Field::Check {
                    label: "Make a subfolder per archive".into(),
                    on: whole,
                },
                overlay::Field::Check {
                    label: "Open destination after".into(),
                    on: false,
                },
            ],
            action: overlay::FormAction::Extract { jobs, whole },
        }));
        self.dirty = true;
    }

    fn run_extract(
        &mut self,
        jobs: Vec<(PathBuf, Option<Vec<String>>)>,
        dest: PathBuf,
        sub: bool,
        go: bool,
    ) {
        let pws = self.passwords.clone();
        self.retry_job = Some(Job {
            id: 0,
            label: format!("extracting {} archive(s)", jobs.len()),
            totals: jobs.len() as u64,
            kind: JobKind::Extract(jobs.clone(), dest.clone(), sub, go, pws.clone()),
            done_text: format!("extracted to {}", dest.display()),
            select: None,
            after: if go { Some(dest.clone()) } else { None },
        });
        self.spawn_job(Job {
            id: 0,
            label: format!("extracting {} archive(s)", jobs.len()),
            totals: jobs.len() as u64,
            kind: JobKind::Extract(jobs, dest.clone(), sub, go, pws),
            done_text: format!("extracted to {}", dest.display()),
            select: None,
            after: if go { Some(dest) } else { None },
        });
    }

    fn form_compress(&mut self, here: bool) {
        let ts = self.real_targets();
        if ts.is_empty() {
            self.notify("select files to compress first");
            return;
        }
        let base = self.loc.path.clone();
        let names: Vec<String> = ts.iter().map(|e| e.name.clone()).collect();
        let default = if here {
            if ts.len() == 1 && ts[0].is_dir {
                ts[0].name.clone()
            } else if ts.len() == 1 {
                strip_arc_ext(&ts[0].name)
            } else {
                base.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "archive".into())
            }
        } else if ts.len() > 1 {
            base.file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "archive".into())
        } else if ts[0].is_dir {
            ts[0].name.clone()
        } else {
            strip_arc_ext(&ts[0].name)
        };
        let fmts: Vec<String> = archive::FORMATS.iter().map(|s| s.to_string()).collect();
        let idx = fmts
            .iter()
            .position(|f| f == &self.cfg.archive_format)
            .unwrap_or(0);
        if here {
            let names = names.clone();
            self.run_compress(
                base.clone(),
                names,
                base.clone(),
                default,
                "zip".into(),
                None,
                false,
                None,
            );
            return;
        }
        let preview_names = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        self.overlay = Some(overlay::Overlay::Form(overlay::Form {
            title: "Compress".into(),
            note: format!("{} item(s): {}", names.len(), preview_names),
            focus: 0,
            fields: vec![
                overlay::Field::Input {
                    label: "Archive name".into(),
                    text: default,
                },
                overlay::Field::Select {
                    label: "Format".into(),
                    options: fmts,
                    idx,
                },
                overlay::Field::Input {
                    label: "Save to".into(),
                    text: base.display().to_string(),
                },
                overlay::Field::Password {
                    label: "Password (zip)".into(),
                    text: String::new(),
                },
                overlay::Field::Check {
                    label: "Trash the originals after".into(),
                    on: false,
                },
            ],
            action: overlay::FormAction::Compress { names, base },
        }));
        self.dirty = true;
    }

    #[allow(clippy::too_many_arguments)]
    fn run_compress(
        &mut self,
        base: PathBuf,
        names: Vec<String>,
        dest: PathBuf,
        name: String,
        fmt: String,
        pw: Option<String>,
        rm: bool,
        select: Option<String>,
    ) {
        let out = fsops::unique_path(&dest, &format!("{name}.{fmt}"));
        let select = select.or_else(|| out.file_name().map(|s| s.to_string_lossy().into_owned()));
        self.spawn_job(Job {
            id: 0,
            label: format!(
                "compressing to {}",
                out.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ),
            totals: 1,
            kind: JobKind::Compress(out, base, names, fmt, pw, rm),
            done_text: String::new(),
            select,
            after: None,
        });
    }

    fn form_props(&mut self) {
        let Some(e) = self.main.entry().cloned() else {
            return;
        };
        let when = crate::model::fmt_local_time(e.mtime);
        let note = format!(
            "{}\n\nkind: {}\nsize: {}\nmodified: {}\npermissions: {}\nlocation: {}",
            e.name,
            crate::model::TAGS[e.kind],
            human_size(e.size),
            when,
            crate::model::fmt_perms(e.mode),
            self.loc.display()
        );
        self.overlay = Some(overlay::Overlay::Form(overlay::Form {
            title: "Properties".into(),
            note,
            focus: 0,
            fields: vec![],
            action: overlay::FormAction::Dismiss,
        }));
        self.dirty = true;
    }

    /// submit: overlay Form values → actions
    fn submit_form(&mut self, form: overlay::Form) {
        let vals: Vec<String> = form.fields.iter().map(|f| f.value()).collect();
        match &form.action {
            overlay::FormAction::Dismiss => {}
            overlay::FormAction::Rename(path) => {
                let new = vals.first().cloned().unwrap_or_default();
                let new = new.trim().to_string();
                if !new.is_empty() {
                    match fsops::rename_path(path, &new) {
                        Ok(_) => {
                            self.notify(&format!("renamed to {new}"));
                            self.reload_sel(Some(new));
                        }
                        Err(e) => {
                            self.err(&e);
                            self.reload();
                        }
                    }
                } else {
                    self.reload();
                }
            }
            overlay::FormAction::NewDir => {
                let name = vals.first().cloned().unwrap_or_default();
                match fsops::make_dir(&self.loc.path, &name) {
                    Ok(d) => {
                        self.reload_sel(d.file_name().map(|s| s.to_string_lossy().into_owned()));
                    }
                    Err(e) => {
                        self.err(&e);
                        self.reload();
                    }
                }
            }
            overlay::FormAction::NewFile => {
                let name = vals.first().cloned().unwrap_or_default();
                match fsops::make_file(&self.loc.path, &name) {
                    Ok(d) => {
                        self.reload_sel(d.file_name().map(|s| s.to_string_lossy().into_owned()));
                    }
                    Err(e) => {
                        self.err(&e);
                        self.reload();
                    }
                }
            }
            overlay::FormAction::Delete(paths) => {
                let n = paths.len();
                self.spawn_job(Job {
                    id: 0,
                    label: "deleting".into(),
                    totals: n as u64,
                    kind: JobKind::Delete(paths.clone()),
                    done_text: format!("deleted {n} item(s)"),
                    select: None,
                    after: None,
                });
            }
            overlay::FormAction::Password(arc) => {
                let pw = vals.first().cloned().unwrap_or_default();
                if pw.is_empty() {
                    return;
                }
                self.passwords.insert(arc.display().to_string(), pw);
                self.dirty = true;
                if let Some(job) = self.retry_job.take() {
                    self.spawn_job(job); // python retried the extract with the fresh password
                } else {
                    self.reload();
                }
                self.overlay = None;
                self.dirty = true;
                return;
            }
            overlay::FormAction::Search => {
                let q = vals.first().cloned().unwrap_or_default().trim().to_string();
                if !q.is_empty() {
                    let loc = self.loc.clone();
                    self.spawn_job(Job {
                        id: 0,
                        label: "searching…".to_string(),
                        totals: 1,
                        kind: JobKind::Find(loc, q),
                        done_text: "found".into(),
                        select: None,
                        after: None,
                    });
                }
            }
            overlay::FormAction::Extract { jobs, whole } => {
                let dest = PathBuf::from(vals.first().cloned().unwrap_or_default());
                let dest = if dest.is_absolute() {
                    dest
                } else {
                    self.loc.path.join(dest)
                };
                let dest = expand_home(&dest);
                let sub = vals.get(1).map(|v| !v.is_empty()).unwrap_or(*whole);
                let go = vals.get(2).map(|v| !v.is_empty()).unwrap_or(false);
                self.run_extract(jobs.clone(), dest, sub, go);
            }
            overlay::FormAction::Compress { names, base } => {
                let name = vals.first().cloned().unwrap_or_default().trim().to_string();
                let fmt = vals.get(1).cloned().unwrap_or_else(|| "zip".into());
                let dest = PathBuf::from(vals.get(2).cloned().unwrap_or_default());
                let dest = if dest.is_absolute() {
                    dest
                } else {
                    self.loc.path.join(dest)
                };
                let dest = expand_home(&dest);
                let pw = vals.get(3).cloned().unwrap_or_default();
                let rm = vals.get(4).map(|v| !v.is_empty()).unwrap_or(false);
                self.run_compress(
                    base.clone(),
                    names.clone(),
                    dest,
                    name,
                    fmt,
                    if pw.is_empty() { None } else { Some(pw) },
                    rm,
                    None,
                );
            }
        }
        self.overlay = None;
        self.dirty = true;
    }

    // ---------- background jobs ----------

    fn spawn_job(&mut self, mut job: Job) {
        job.id = self.next_job;
        self.next_job += 1;
        self.running_job = Some((job.id, job.label.clone(), job.totals));
        self.job_done = 0;
        self.cur_meta = Some((
            job.done_text.clone(),
            job.select.clone(),
            job.after.clone().map(Loc::real),
        ));
        self.notify(&format!("⏳ {}…", job.label));
        self.msg_until = Instant::now() + Duration::from_secs(3600);
        let tx = self.tx.clone();
        let id = job.id;
        std::thread::spawn(move || {
            let res = match &job.kind {
                JobKind::CopyMove(srcs, dst, is_move) => {
                    let mut n = 0u64;
                    let mut cb = |_b: u64| {
                        n += 1;
                        let _ = tx.send(Msg::Progress { job: id, done: n });
                    };
                    let cm = if *is_move {
                        fsops::move_paths(srcs, dst, &mut cb)
                    } else {
                        fsops::copy_paths(srcs, dst, &mut cb)
                    };
                    if cm.errs.is_empty() {
                        BgRes::Moved(cm, *is_move)
                    } else {
                        BgRes::Failed(cm.errs.join("; "))
                    }
                }
                JobKind::Trash(paths) => {
                    let (errs, pairs) = fsops::journal_trash(paths);
                    if errs.is_empty() {
                        BgRes::Finished(pairs)
                    } else {
                        BgRes::Failed(errs.join("; "))
                    }
                }
                JobKind::Delete(paths) => {
                    let errs = fsops::delete_paths(paths);
                    if errs.is_empty() {
                        BgRes::Finished(vec![])
                    } else {
                        BgRes::Failed(errs.join("; "))
                    }
                }
                JobKind::Extract(jobs, dest, sub, _go, pws) => {
                    let mut res = BgRes::Finished(vec![]);
                    for (i, (arc, members)) in jobs.iter().enumerate() {
                        let d = if *sub {
                            fsops::unique_path(
                                dest,
                                &strip_arc_ext(
                                    &arc.file_name()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_default(),
                                ),
                            )
                        } else {
                            dest.clone()
                        };
                        let pw = pws.get(&arc.display().to_string()).map(String::as_str);
                        let mut cb = |_n: u64| {
                            let _ = tx.send(Msg::Progress {
                                job: id,
                                done: i as u64 + 1,
                            });
                        };
                        if let Err(er) = archive::extract(arc, &d, members.as_deref(), pw, &mut cb)
                        {
                            res = if er.need_password {
                                BgRes::NeedPassword(arc.clone())
                            } else {
                                BgRes::Failed(er.msg)
                            };
                            break;
                        }
                        let _ = tx.send(Msg::Progress {
                            job: id,
                            done: i as u64 + 1,
                        });
                    }
                    res
                }
                JobKind::Compress(out, base, names, fmt, pw, rm) => {
                    match archive::compress(out, base, names, fmt, pw.as_deref()) {
                        Err(er) => BgRes::Failed(er.msg),
                        Ok(()) if !*rm => BgRes::Finished(vec![]),
                        Ok(()) => {
                            let originals: Vec<PathBuf> =
                                names.iter().map(|n| base.join(n)).collect();
                            let (errs, pairs) = fsops::journal_trash(&originals);
                            if errs.is_empty() {
                                BgRes::Finished(pairs)
                            } else {
                                BgRes::Failed(errs.join("; "))
                            }
                        }
                    }
                }
                JobKind::Find(base, q) => {
                    let hits = deep_find(base, q, 400);
                    let _ = tx.send(Msg::Found { hits });
                    BgRes::Finished(vec![])
                }
            };
            tx.send(Msg::Done { job: id, res })
                .expect("ui died mid-job");
        });
    }

    /// drain worker messages and apply them
    fn drain(&mut self) {
        loop {
            let got = self.rx.try_recv();
            let Ok(m) = got else { return };
            self.apply(m);
        }
    }

    fn apply(&mut self, m: Msg) {
        match m {
            Msg::Preview { seq, pv } => {
                if seq == self.pv_seq {
                    self.pv.set(pv);
                    self.dirty = true;
                }
            }
            Msg::Progress { job, done } => {
                if self.running_job.as_ref().map(|(id, _, _)| *id) == Some(job) {
                    self.job_done = done;
                    self.dirty = true;
                }
            }
            Msg::Done { job, res } => {
                if self.running_job.as_ref().map(|(id, _, _)| *id) != Some(job) {
                    return;
                }
                self.running_job = None;
                let (done_text, select, after) = self.cur_meta.take().unwrap_or_default();
                let failed = matches!(&res, BgRes::Failed(_));
                match res {
                    BgRes::Failed(msg) => {
                        self.err(&msg);
                        self.dirty = true;
                    }
                    BgRes::NeedPassword(arc) => {
                        self.ask_password(arc);
                        return;
                    }
                    BgRes::Finished(pairs) => {
                        self.retry_job = None;
                        for (a, b) in pairs {
                            self.state.push_undo(UndoOp {
                                kind: "trash".into(),
                                a: a.display().to_string(),
                                b: b.display().to_string(),
                            });
                        }
                        if !done_text.is_empty() {
                            self.notify(&done_text);
                        }
                    }
                    BgRes::Moved(cm, is_move) => {
                        self.retry_job = None;
                        if is_move {
                            for (a, b) in cm.targets.iter() {
                                self.state.push_undo(UndoOp {
                                    kind: "move".into(),
                                    a: a.display().to_string(),
                                    b: b.display().to_string(),
                                });
                            }
                        }
                        self.notify(&done_text);
                    }
                }
                match after {
                    Some(dest) if !failed => self.goto(dest),
                    _ => self.reload_sel(select),
                }
            }
            Msg::Found { hits } => {
                self.found_menu(hits);
            }
            Msg::Loaded {
                key,
                entries,
                parents,
                err,
            } => self.loaded(key, entries, parents, err),
        }
    }

    fn loaded(
        &mut self,
        key: String,
        entries: Vec<Entry>,
        parents: Vec<Entry>,
        err: Option<(String, bool)>,
    ) {
        if key != Self::loc_key(&self.loc) {
            return;
        }
        if let Some((m, needpw)) = err {
            if needpw {
                if let Some(arc) = self.loc.arc.clone() {
                    self.ask_password(arc);
                }
                return;
            }
            self.err(&m);
            if self.loc.arc.is_none() && !self.loc.path.exists() {
                if let Some(par) = self.loc.parent() {
                    self.goto(par);
                }
            }
            return;
        }
        self.entries = entries;
        if self.loc.arc.is_none() {
            self.dir_mtime = mtime_of(&self.loc.path);
        }
        let cur = if let Some(arc) = self.loc.arc.as_ref() {
            if !self.loc.inner.is_empty() {
                self.loc
                    .inner
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_string()
            } else {
                arc.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            }
        } else {
            self.loc
                .path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        };
        let pents: Vec<Entry> = parents
            .into_iter()
            .filter(|e| self.show_hidden || !e.name.starts_with('.'))
            .collect();
        let pents = sort_entries(pents, "name", false);
        Self::set_list(&mut self.parent_list, pents, Some(cur));
        let keep = self.want.take().filter(|s| !s.is_empty());
        self.refresh_view(keep);
    }

    fn found_menu(&mut self, hits: Vec<Entry>) {
        if hits.is_empty() {
            self.notify("no matches");
            return;
        }
        let items: Vec<Option<(String, String, bool)>> = hits
            .iter()
            .map(|e| {
                let act = if e.is_dir || e.virtual_() {
                    format!(
                        "dir:{}",
                        e.path
                            .as_ref()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default()
                    )
                } else {
                    let par = e
                        .path
                        .as_ref()
                        .and_then(|p| p.parent())
                        .map(|p| p.display().to_string())
                        .unwrap_or_default();
                    let name = e.name.clone();
                    format!("sel:{par}\u{1}{name}")
                };
                Some((
                    format!("{}  {}", e.name, homey(e.path.as_ref().unwrap())),
                    act,
                    true,
                ))
            })
            .collect();
        self.open_menu(items, 8, 4);
    }

    // ---------- preview ----------

    fn spawn_preview(&mut self) {
        let Some(e) = self.main.entry().cloned() else {
            return;
        };
        self.pv_seq += 1;
        let seq = self.pv_seq;
        let pal = self.pal.clone();
        let nerd = self.cfg.icons == "nerd";
        let key = e
            .arc
            .as_ref()
            .or(e.path.as_ref())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let pw = self
            .passwords
            .get(&key)
            .filter(|s| !s.is_empty())
            .map(String::as_str)
            .unwrap_or_default()
            .to_string();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let pv = build_preview(
                Some(&e),
                &pal,
                nerd,
                if pw.is_empty() {
                    None
                } else {
                    Some(pw.as_str())
                },
            );
            let _ = tx.send(Msg::Preview { seq, pv });
        });
    }
}

fn deep_find(base: &Loc, q: &str, cap: usize) -> Vec<Entry> {
    if base.arc.is_some() {
        return vec![];
    }
    let needle = q.to_lowercase();
    let mut out = vec![];
    let mut stack = vec![(base.path.clone(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 4 || out.len() >= cap {
            break;
        }
        for e in fsops::list_real(&dir).unwrap_or_default() {
            if !out.is_empty() && out.len() >= cap {
                break;
            }
            if e.name.starts_with('.') {
                continue;
            }
            if fuzzy(&e.name, &needle) {
                out.push(e.clone());
            }
            if e.is_dir && e.path.is_some() {
                stack.push((e.path.clone().unwrap(), depth + 1));
            } else if e.kind == crate::model::KIND_ARCHIVE {
                // inside archives too: index stays cached so reuse the walk
                if let Some(arc) = e.path.clone() {
                    let inner = base.path.display().to_string();
                    let _ = (arc, inner);
                }
            }
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

fn expand_home(p: &Path) -> PathBuf {
    let s = p.display().to_string();
    if let Some(rest) = s.strip_prefix('~') {
        return crate::theme::home().join(rest.trim_start_matches('/'));
    }
    p.to_path_buf()
}

impl App {
    // ---------- run loop ----------

    pub fn run(
        &mut self,
        t: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> std::io::Result<()> {
        self.reload();
        loop {
            self.drain();
            self.cadence();
            if self.quit.is_some() {
                return Ok(());
            }
            if self.dirty {
                self.dirty = false;
                self.paint_frame(t)?;
            }
            let timeout = if self.graph.is_some() {
                Duration::from_millis(33)
            } else {
                Duration::from_millis(120)
            };
            if crossterm::event::poll(timeout)? {
                match crossterm::event::read()? {
                    Event::Key(k)
                        if matches!(
                            k.kind,
                            crossterm::event::KeyEventKind::Press
                                | crossterm::event::KeyEventKind::Repeat
                        ) =>
                    {
                        self.on_key(&k)
                    }
                    Event::Mouse(m) => self.on_mouse(m),
                    Event::Resize(_, _) => {
                        self.pv.forget();
                        self.dirty = true;
                    }
                    _ => {}
                }
            }
        }
    }

    /// One frame, then the picture. ratatui only resends cells that changed,
    /// and a sixel under unchanged cells would survive that, so when the
    /// picture goes away the pane is first blanked for real (a frame with the
    /// area cleared), then drawn again.
    fn paint_frame(
        &mut self,
        t: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> std::io::Result<()> {
        let mut six = None;
        t.draw(|f| six = self.draw(f))?;
        if let Some(gone) = self.pv.erase.take() {
            t.draw(|f| {
                self.draw(f);
                f.render_widget(Clear, gone);
            })?;
            t.draw(|f| six = self.draw(f))?;
        }
        if let Some(req) = six {
            sixel::place_at(req.row, req.col, &req.data);
            self.pv.mark_painted(&req);
        }
        Ok(())
    }

    fn draw(&mut self, f: &mut ratatui::Frame) -> Option<preview::SixReq> {
        self.clicks.clear();
        self.pending_six = None;
        let area = f.area();
        if let Some(gv) = &mut self.graph {
            let rows = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(4),
                Constraint::Length(1),
            ])
            .split(area);
            self.graph_pane = rows[1];
            self.pv.forget();
            gv.resize(rows[1].width, rows[1].height);
            let title = gv.title_line(&self.pal);
            f.render_widget(Paragraph::new(title).style(bgp(self.pal.panel2)), rows[0]);
            let lines: Vec<Line> = gv
                .frame
                .iter()
                .map(|row| {
                    let spans: Vec<Span> = row
                        .iter()
                        .map(|s| {
                            let mut st = cpx(s.fg);
                            if s.bold {
                                st = st.add_modifier(Modifier::BOLD);
                            }
                            if let Some(bgb) = s.bg {
                                st = st.bg(Color::Rgb(bgb.0, bgb.1, bgb.2));
                            }
                            Span::styled(s.text.clone(), st)
                        })
                        .collect();
                    Line::from(spans)
                })
                .collect();
            f.render_widget(Paragraph::new(lines), rows[1]);
            let info = gv.info_line(&self.pal);
            f.render_widget(Paragraph::new(info).style(bgp(self.pal.panel2)), rows[2]);
            return None;
        }
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);
        self.top_row(f, rows[0]);
        self.tools_row(f, rows[1]);
        self.cols_row(f, rows[2]);
        self.status_row(f, rows[3]);
        // overlay on top
        match &self.overlay {
            Some(overlay::Overlay::Form(form)) => overlay::draw_form(f, &self.pal, form),
            Some(overlay::Overlay::Help) => overlay::draw_help(f, &self.pal),
            Some(overlay::Overlay::Menu { .. }) => {}
            None => {}
        }
        let menu = match self.overlay.as_ref() {
            Some(overlay::Overlay::Menu { items, hover, x, y }) => {
                Some((items.clone(), *hover, *x, *y))
            }
            _ => None,
        };
        if let Some((items, hover, x, y)) = menu {
            self.menu_rect = overlay::draw_menu(f, &self.pal, &items, hover, x, y);
        }
        self.pending_six.take()
    }

    fn top_row(&mut self, f: &mut ratatui::Frame, row: Rect) {
        let input_w = 24.min(row.width.saturating_sub(30));
        let crumbs = self.loc.crumbs();
        let budget = row.width.saturating_sub(input_w) as usize;
        let mut start = 0usize;
        while start < crumbs.len().saturating_sub(1)
            && crumbs[start..]
                .iter()
                .map(|(n, _)| n.chars().count() + 5)
                .sum::<usize>()
                > budget
        {
            start += 1;
        }
        let mut spans: Vec<Span> = vec![Span::styled(" ".to_string(), cpx(self.pal.panel))];
        if start > 0 {
            spans.push(Span::styled("… ".to_string(), cpx(self.pal.faint)));
        }
        for (i, (name, _)) in crumbs.iter().enumerate().skip(start) {
            if i > start {
                spans.push(Span::styled(" › ".to_string(), cpx(self.pal.faint)));
            }
            let last = i == crumbs.len() - 1;
            let style = if last {
                Style::new()
                    .fg(Color::Rgb(
                        self.pal.accent.0,
                        self.pal.accent.1,
                        self.pal.accent.2,
                    ))
                    .add_modifier(Modifier::BOLD)
            } else {
                cpx(self.pal.text)
            };
            let w = spans
                .iter()
                .map(|s| s.content.as_ref().width())
                .sum::<usize>();
            let text = format!(" {name} ");
            spans.push(Span::styled(text.clone(), style));
            self.clicks.push(Click {
                x0: 1 + w as u16,
                x1: 1 + w as u16 + text.chars().count() as u16,
                y: row.y,
                act: format!("cd:{i}"),
            });
        }
        let trail_line = Line::from(spans);
        let cols = Layout::horizontal([Constraint::Min(20), Constraint::Length(input_w.max(1))])
            .split(row);
        f.render_widget(
            Paragraph::new(trail_line).style(bgp(self.pal.panel)),
            cols[0],
        );
        // the search box
        let txt: String = if self.filter.is_empty() && !self.input_focus {
            "/ filter".to_string()
        } else {
            self.filter.clone()
        };
        let cur = if self.input_focus { "▍" } else { "" };
        let bgc = if self.input_focus {
            mix(self.pal.panel, self.pal.sel, 0.6)
        } else {
            self.pal.panel
        };
        let boxl: Vec<Span> = vec![
            Span::styled(" ".to_string(), bgp(bgc)),
            Span::styled(
                if self.input_focus {
                    txt.clone()
                } else {
                    format!("{txt}{} ", " ".repeat(input_w.saturating_sub(3) as usize))
                },
                if self.input_focus {
                    cpx(self.pal.text)
                } else {
                    cpx(self.pal.faint)
                }
                .bg(Color::Rgb(bgc.0, bgc.1, bgc.2)),
            ),
            Span::styled(
                cur.to_string(),
                cpx(self.pal.text)
                    .bg(Color::Rgb(bgc.0, bgc.1, bgc.2))
                    .add_modifier(Modifier::BOLD),
            ),
        ];
        self.clicks.push(Click {
            x0: cols[1].x,
            x1: cols[1].x + cols[1].width,
            y: row.y,
            act: "search".into(),
        });
        f.render_widget(Paragraph::new(Line::from(boxl)).style(bgp(bgc)), cols[1]);
    }

    fn tools_row(&mut self, f: &mut ratatui::Frame, row: Rect) {
        let ts = self.targets();
        let real = !ts.is_empty() && self.loc.arc.is_none();
        let can_ex = self.loc.arc.is_some()
            || (!ts.is_empty() && ts.iter().all(|e| e.kind == crate::model::KIND_ARCHIVE));
        let items: Vec<(String, &str, &str, bool)> = vec![
            ("Graph".into(), "graph", "accent", true),
            ("Up".into(), "back", "normal", self.loc.parent().is_some()),
            (" · ".to_string(), "", "sep", false),
            ("Cut".into(), "cut", "normal", real),
            ("Copy".into(), "copy", "normal", real),
            (
                "Paste".into(),
                "paste",
                "normal",
                self.clip.is_some() && self.loc.arc.is_none(),
            ),
            (" · ".to_string(), "", "sep", false),
            (
                "+ Folder".into(),
                "newdir",
                "normal",
                self.loc.arc.is_none(),
            ),
            ("Rename".into(), "rename", "normal", real),
            ("Trash".into(), "trash", "normal", real),
            (" · ".to_string(), "", "sep", false),
            ("Extract".into(), "extract", "normal", can_ex),
            ("Compress".into(), "compress", "normal", real),
            (" · ".to_string(), "", "sep", false),
            (
                "Hidden".into(),
                "hidden",
                if self.show_hidden {
                    "current"
                } else {
                    "normal"
                },
                true,
            ),
            (
                format!("Sort: {}", SORTS[self.sort_idx]),
                "sort",
                "normal",
                true,
            ),
            ("Help".into(), "help", "normal", true),
        ];
        let mut spans: Vec<Span> = vec![Span::styled(" ".to_string(), cpx(self.pal.panel))];
        let mut x = 1u16;
        for (label, act, kind, on) in items {
            let w = label.chars().count() as u16;
            if x + w > row.width {
                break;
            }
            if kind == "sep" {
                spans.push(Span::styled(label.clone(), cpx(self.pal.faint)));
                x += w;
                continue;
            }
            let (fgc, bgc2) = match kind {
                "accent" | "current" => (self.pal.bg, self.pal.accent),
                _ if on => (self.pal.text, self.pal.panel),
                _ => (self.pal.faint, self.pal.panel),
            };
            let text = format!(" {label} ");
            spans.push(Span::styled(
                text.clone(),
                Style::new()
                    .fg(Color::Rgb(fgc.0, fgc.1, fgc.2))
                    .bg(Color::Rgb(bgc2.0, bgc2.1, bgc2.2)),
            ));
            if on {
                self.clicks.push(Click {
                    x0: x,
                    x1: x + w + 2,
                    y: row.y,
                    act: act.to_string(),
                });
            }
            x += w + 2;
        }
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(bgp(self.pal.panel)),
            row,
        );
    }

    fn cols_row(&mut self, f: &mut ratatui::Frame, area: Rect) {
        let g = rects(area, self.pct);
        self.geo = Some(Geo {
            cols: area,
            parent: g.0,
            main: g.1,
            prev: g.2,
            sz0: g.3,
            sz1: g.4,
        });
        let nerd = self.cfg.icons == "nerd";
        self.list_pane(f, g.0, false, nerd);
        self.list_pane(f, g.1, true, nerd);
        // sizers
        let scolor = cpx(mix(self.pal.panel, self.pal.faint, 0.4));
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                "│".repeat(g.0.height as usize),
                scolor,
            )])),
            Rect::new(g.3, area.y, 1, area.height),
        );
        f.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                "│".repeat(g.0.height as usize),
                scolor,
            )])),
            Rect::new(g.4, area.y, 1, area.height),
        );
        // preview
        if self.overlay.is_some() {
            self.pv.taint();
        }
        let paint = self.overlay.is_none() && self.sizer.is_none();
        self.pending_six = self.pv.draw(f, g.2, &self.pal, paint);
    }

    /// one file list plus its 1-column scrollbar (python: scrollbar-size-vertical: 1)
    fn list_pane(&mut self, f: &mut ratatui::Frame, area: Rect, main: bool, nerd: bool) {
        let pal = self.pal.clone();
        let lv = if main {
            &mut self.main
        } else {
            &mut self.parent_list
        };
        let bar = lv.overflows(area.height);
        lv.width = (area.width as usize).saturating_sub(bar as usize);
        lv.rows = area.height as usize;
        lv.ensure_visible();
        let body = Rect {
            width: area.width - bar as u16,
            ..area
        };
        let base = if main { pal.panel } else { pal.side() };
        f.render_widget(Block::new().style(bgp(base)), area);
        f.render_widget(Paragraph::new(lv.lines(&pal, nerd)), body);
        if bar {
            let mut st = ScrollbarState::new(lv.view.len())
                .viewport_content_length(lv.rows)
                .position(lv.top);
            f.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight)
                    .begin_symbol(None)
                    .end_symbol(None)
                    .track_symbol(Some(" "))
                    .thumb_symbol("┃")
                    .thumb_style(cpx(mix(base, pal.faint, 0.8)))
                    .track_style(Style::new().bg(Color::Rgb(base.0, base.1, base.2))),
                area,
                &mut st,
            );
        }
    }

    fn status_row(&mut self, f: &mut ratatui::Frame, row: Rect) {
        let mode = if self.hint.is_some() {
            "HINT"
        } else if self.pending.as_deref() == Some("pin") {
            "PIN"
        } else if self.pending.as_deref() == Some("warp") {
            "WARP"
        } else {
            "NAV"
        };
        let mut spans = vec![chip(
            &format!(" {mode} "),
            self.pal.bg,
            self.pal.accent,
            true,
        )];
        let mut info = format!("  {} items", self.main.view.len());
        let sel = self.main.selected.len();
        if sel > 0 {
            info += &format!(" · {sel} selected");
        }
        if !self.filter.is_empty() {
            info += &format!(" · filter \"{}\"", self.filter);
        }
        if self.show_hidden {
            info += " · hidden shown";
        }
        if let Some((m, paths)) = &self.clip {
            info += &format!(" · clipboard: {} {}", paths.len(), m);
        }
        let mut x = mode.chars().count() as u16 + 2;
        let style_b = cpx(self.pal.text);
        spans.push(Span::styled(format!(" {info} "), style_b));
        x += info.chars().count() as u16 + 2;
        // progress bar while an op runs
        let mut bar: Option<String> = None;
        if let Some((_, label, total)) = &self.running_job {
            let donep = (self.job_done * 100 / (*total).max(1)).min(100);
            let fill = donep as usize * 12 / 100;
            bar = Some(format!(
                "⏳ {label}… [{}{}] {}/{}",
                "█".repeat(fill),
                "─".repeat(12 - fill),
                self.job_done,
                total
            ));
        } else if !self.msg.is_empty() {
            spans.push(Span::styled(format!("  {}", self.msg), cpx(self.pal.pin)));
            x += self.msg.chars().count() as u16 + 2;
        }
        if let Some(bar_text) = bar {
            spans.push(Span::styled(format!("  {bar_text}"), cpx(self.pal.accent2)));
        }
        let right = "Tab graph · ? keys ".to_string();
        let rightw = right.chars().count() as u16;
        let pad = (row.width.saturating_sub(1)) - x.min(row.width.saturating_sub(1));
        let pad = pad.saturating_sub(rightw.max(1));
        spans.push(Span::styled(" ".repeat(pad as usize), cpx(self.pal.panel)));
        spans.push(Span::styled(right, cpx(self.pal.faint)));
        f.render_widget(
            Paragraph::new(Line::from(spans)).style(bgp(self.pal.panel)),
            row,
        );
    }

    // ---------- cadence ----------

    fn cadence(&mut self) {
        let now = Instant::now();
        if let Some(gv) = &mut self.graph {
            if self.last_tick.elapsed() >= Duration::from_millis(33) {
                self.last_tick = now;
                gv.tick();
                self.dirty = true;
            }
        }
        if self.pv_due.map(|d| now >= d).unwrap_or(false) && self.graph.is_none() {
            self.pv_due = None;
            self.spawn_preview();
        }
        if self.running_job.is_none() && !self.msg.is_empty() && now > self.msg_until {
            self.msg.clear();
            self.dirty = true;
        }
        if self.last_watch.elapsed() >= Duration::from_secs(1) {
            self.last_watch = now;
            self.watch();
        }
    }

    fn watch(&mut self) {
        let spath = self
            .scheme
            .clone()
            .unwrap_or_else(crate::theme::scheme_path);
        if let Ok(m) = std::fs::metadata(&spath) {
            let mt = m.modified().ok();
            if mt != self.pal_mtime {
                self.pal_mtime = mt;
                self.pal = Palette::load(&spath).0;
                self.pv.refresh();
                self.dirty = true;
            }
        }
        if self.loc.arc.is_none() {
            let mt = mtime_of(&self.loc.path);
            if mt != self.dir_mtime {
                self.dir_mtime = mt;
                self.reload();
            }
        }
    }

    // ---------- keys ----------

    fn on_key(&mut self, k: &KeyEvent) {
        if self.graph.is_some() {
            self.graph_key(k);
            return;
        }
        let Some(ks) = Some(canon(k.code, k.modifiers)) else {
            return;
        };
        if ks.is_empty() {
            return;
        }
        if let Some(ov) = self.overlay.take() {
            self.overlay_key(ov, &ks, k);
            return;
        }
        if self.input_focus {
            self.input_key(&ks, k);
            return;
        }
        if self.hint.is_some() {
            self.hint_typing(&ks);
            return;
        }
        if self.pending.is_some() {
            self.pending_typing(&ks);
            return;
        }
        if let Some(&act) = self.keys.get(&ks) {
            self.do_action(act);
        }
    }

    fn graph_key(&mut self, k: &KeyEvent) {
        let ks = canon(k.code, k.modifiers);
        let single = if k.modifiers.is_empty() {
            match k.code {
                KeyCode::Char(c) => Some(c),
                _ => None,
            }
        } else {
            None
        };
        let Some(mut gv) = self.graph.take() else {
            return;
        };
        let mut close: Option<Loc> = None;
        match single {
            Some('j') => {
                gv.cycle(1);
            }
            Some('k') => {
                gv.cycle(-1);
            }
            Some('l') => {
                if let Some(f) = gv.focus {
                    match gv.g.nodes[f].role {
                        crate::graph::Role::Centre => close = Some(gv.g.loc.clone()),
                        crate::graph::Role::Parent => {
                            gv.g.up();
                            gv.wake();
                        }
                        _ => {
                            gv.activate(f);
                        }
                    }
                }
            }
            Some('.') => {
                gv.toggle_hidden();
            }
            Some('n') => {
                if let Some(m) =
                    gv.g.nodes
                        .iter()
                        .position(|n| n.role == crate::graph::Role::More)
                {
                    gv.activate(m);
                }
            }
            Some('r') => {
                gv.reset_layout();
            }
            Some('+') | Some('=') => {
                gv.zoom_by(1.15);
            }
            Some('-') => {
                gv.zoom_by(1.0 / 1.15);
            }
            Some('0') => {
                gv.g.zoom = 1.0;
                gv.wake();
            }
            Some('q') => {
                self.graph = None;
                self.dirty = true;
                return;
            }
            _ => match ks.as_str() {
                "down" => {
                    gv.cycle(1);
                }
                "up" => {
                    gv.cycle(-1);
                }
                "right" | "space" => {
                    if let Some(f) = gv.focus {
                        match gv.g.nodes[f].role {
                            crate::graph::Role::Centre => close = Some(gv.g.loc.clone()),
                            crate::graph::Role::Parent => {
                                gv.g.up();
                                gv.wake();
                            }
                            _ => {
                                gv.activate(f);
                            }
                        }
                    }
                }
                "backspace" | "left" => {
                    gv.hover = None;
                    gv.focus = None;
                    gv.g.up();
                    gv.wake();
                }
                "enter" => match gv.focus {
                    Some(f) => match gv.g.nodes[f].role {
                        crate::graph::Role::Centre => close = Some(gv.g.loc.clone()),
                        _ => {
                            gv.activate(f);
                        }
                    },
                    None => close = Some(gv.g.loc.clone()),
                },
                "tab" | "escape" => close = Some(gv.g.loc.clone()),
                _ => {}
            },
        }
        if let Some(loc) = close {
            self.goto(loc);
            return;
        }
        self.graph = Some(gv);
        self.dirty = true;
    }

    fn overlay_key(&mut self, ov: overlay::Overlay, ks: &str, k: &KeyEvent) {
        match ov {
            overlay::Overlay::Help => {
                self.overlay = None;
                self.dirty = true;
            }
            overlay::Overlay::Menu {
                items, mut hover, ..
            } => {
                if ks == "escape" || ks == "q" {
                    self.dirty = true;
                    return;
                }
                let nl = items.len() as i32;
                let cur = if nl > 0 {
                    let mut i = if hover == usize::MAX {
                        -i32::from(ks == "up" || ks == "k")
                    } else {
                        hover as i32
                    };
                    for _ in 0..nl {
                        i = (i + if ks == "down" || ks == "j" { 1 } else { -1 }).rem_euclid(nl);
                        if items
                            .get(i as usize)
                            .map(|x| x.as_ref().map(|(_, _, e)| *e).unwrap_or(false))
                            .unwrap_or(false)
                        {
                            hover = i as usize;
                            break;
                        }
                    }
                    hover
                } else {
                    hover
                };
                if ks == "enter" || ks == "l" {
                    if let Some(it) = items.get(cur).and_then(Option::as_ref) {
                        let act = it.1.clone();
                        self.overlay = None;
                        self.run_menu_action(&act);
                        return;
                    }
                }
                self.overlay = Some(overlay::Overlay::Menu {
                    items,
                    hover: cur,
                    x: 0,
                    y: 0,
                });
                self.dirty = true;
            }
            overlay::Overlay::Form(mut form) => {
                if ks == "escape" {
                    self.dirty = true;
                    return;
                }
                if canon(k.code, k.modifiers) == "enter" {
                    self.submit_form(form);
                    return;
                }
                match k.code {
                    KeyCode::Backspace => {
                        if let Some(f) = form.fields.get_mut(form.focus) {
                            f.backspace();
                        }
                    }
                    KeyCode::Tab | KeyCode::Down => {
                        form.focus = (form.focus + 1).min(form.fields.len().saturating_sub(1));
                    }
                    KeyCode::Up => {
                        form.focus = form.focus.saturating_sub(1);
                    }
                    KeyCode::Left | KeyCode::Right => {
                        if let Some(f) = form.fields.get_mut(form.focus) {
                            let d = if k.code == KeyCode::Right { 1 } else { -1 };
                            f.cycle(d);
                        }
                    }
                    KeyCode::Char(c)
                        if !k
                            .modifiers
                            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                    {
                        if let Some(f) = form.fields.get_mut(form.focus) {
                            match f {
                                overlay::Field::Check { on, .. } => {
                                    if c == ' ' {
                                        *on = !*on;
                                    }
                                }
                                _ => f.key(c),
                            }
                        }
                    }
                    _ => {}
                }
                self.overlay = Some(overlay::Overlay::Form(form));
                self.dirty = true;
            }
        }
    }

    fn input_key(&mut self, ks: &str, k: &KeyEvent) {
        match k.code {
            KeyCode::Esc => {
                self.input_focus = false;
                self.dirty = true;
            }
            KeyCode::Enter => {
                self.input_focus = false;
                self.dirty = true;
            }
            KeyCode::Backspace => {
                self.filter.pop();
                self.main.cursor = 0;
                self.refresh_view(None);
            }
            KeyCode::Char(c)
                if !k
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                self.filter.push(c);
                self.main.cursor = 0;
                self.refresh_view(None);
            }
            _ => {
                let _ = ks;
            }
        }
    }

    fn hint_typing(&mut self, ks: &str) {
        if ks == "escape" || ks.chars().any(|c| !c.is_ascii_alphabetic()) {
            self.end_hint();
            return;
        }
        let Some(h) = self.hint.as_mut() else { return };
        h.push_str(&ks.to_lowercase());
        let hint = h.clone();
        let Some(hints) = self.main.hints.clone() else {
            self.end_hint();
            return;
        };
        let hit = hints.iter().find(|(_, t)| **t == hint);
        if let Some((row, _)) = hit {
            let row = *row;
            self.end_hint();
            self.main.cursor = row;
            self.main.reveal();
            self.after_move();
            let ent = self.main.entry().cloned();
            if let Some(e) = ent {
                if e.is_dir || e.kind == crate::model::KIND_ARCHIVE {
                    self.act_enter();
                }
            }
            return;
        }
        let frames = hints.values().any(|t| t.starts_with(&hint));
        if !frames {
            self.end_hint();
        } else {
            self.dirty = true;
        }
    }

    fn end_hint(&mut self) {
        self.hint = None;
        self.main.hints = None;
        self.dirty = true;
    }

    fn pending_typing(&mut self, ks: &str) {
        let what = self.pending.clone();
        self.pending = None;
        let mut it = ks.chars();
        if let (Some(c), None) = (it.next(), it.next()) {
            if c.is_ascii_alphabetic() {
                if what.as_deref() == Some("pin") {
                    self.state
                        .set_anchor(&c.to_string(), &self.loc.path.display().to_string());
                    self.notify(&format!("pinned {} as '{c}'", self.loc.path.display()));
                } else {
                    match self.state.anchors.get(&c.to_string()).cloned() {
                        Some(p) if Path::new(&p).is_dir() => self.goto(Loc::real(PathBuf::from(p))),
                        Some(_) => self.err(&format!("no folder pinned as '{c}'")),
                        None => self.notify(&format!("no folder pinned as '{c}'")),
                    }
                }
            }
        }
        self.dirty = true;
    }

    // ---------- mouse ----------

    fn on_mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        let ctrl = m.modifiers.contains(KeyModifiers::CONTROL);
        if self.graph.is_some() {
            self.mouse_graph(m, x, y);
            return;
        }
        if self.overlay.is_some() {
            self.mouse_overlay(m, x, y);
            return;
        }
        match m.kind {
            MouseEventKind::Down(btn) => self.mouse_down(btn, x, y, ctrl),
            MouseEventKind::Drag(_) => self.mouse_drag(x, y, ctrl),
            MouseEventKind::Up(btn) => self.mouse_up(btn, x, y, ctrl),
            MouseEventKind::ScrollUp => self.wheel(x, y, -WHEEL_STEP),
            MouseEventKind::ScrollDown => self.wheel(x, y, WHEEL_STEP),
            _ => {}
        }
    }

    /// textual's default: two rows per notch, on whichever pane the pointer
    /// is over. The cursor stays where it is.
    fn wheel(&mut self, x: u16, y: u16, d: i32) {
        let Some(g) = self.geo else { return };
        let over = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        if over(g.prev) {
            self.pv.scroll_by(d);
        } else if over(g.main) {
            self.main.scroll_by(d);
        } else if over(g.parent) {
            self.parent_list.scroll_by(d);
        } else {
            return;
        }
        self.dirty = true;
    }

    fn mouse_graph(&mut self, m: MouseEvent, gx: u16, gy: u16) {
        let pane = self.graph_pane;
        let dx = (gx.saturating_sub(pane.x)) as f32 * 2.0 + 1.0;
        let dy = (gy.saturating_sub(pane.y)) as f32 * 4.0 + 2.0;
        let Some(mut gv) = self.graph.take() else {
            return;
        };
        match m.kind {
            MouseEventKind::Down(btn) => {
                gv.mouse_down(dx, dy, btn == MouseButton::Right);
            }
            MouseEventKind::Drag(_) => {
                gv.mouse_drag(dx, dy);
            }
            MouseEventKind::Up(_) => {
                let (act, pinned) = gv.mouse_up(dx, dy);
                if let Some(name) = pinned {
                    self.notify(&format!("pinned {name}"));
                }
                if let Some(i) = act {
                    match gv.g.nodes.get(i).map(|n| n.role) {
                        Some(crate::graph::Role::Centre) => {
                            let loc = gv.g.loc.clone();
                            self.dirty = true;
                            self.goto(loc);
                            return;
                        }
                        Some(crate::graph::Role::Parent) => {
                            gv.g.up();
                            gv.wake();
                        }
                        _ => {
                            gv.activate(i);
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp => gv.zoom_by(1.12),
            MouseEventKind::ScrollDown => gv.zoom_by(1.0 / 1.12),
            _ => gv.set_hover_from_dots(dx, dy),
        }
        self.graph = Some(gv);
        self.dirty = true;
    }

    fn mouse_overlay(&mut self, m: MouseEvent, x: u16, y: u16) {
        // python menus: hover via move, click on an enabled row picks, esc/outside dismisses
        if let MouseEventKind::Moved = m.kind {
            let r = self.menu_rect;
            let inside = x >= r.x && x < r.x + r.width && y > r.y && y < r.y + r.height;
            if inside {
                let Some(overlay::Overlay::Menu { items, hover, .. }) = self.overlay.as_mut()
                else {
                    return;
                };
                let row = (y - r.y - 1) as usize;
                if row < items.len()
                    && items[row].as_ref().map(|it| it.2).unwrap_or(false)
                    && *hover != row
                {
                    *hover = row;
                    self.dirty = true;
                }
            }
            return;
        }
        if !matches!(m.kind, MouseEventKind::Up(_) | MouseEventKind::Down(_)) {
            return;
        }
        let down = matches!(m.kind, MouseEventKind::Down(_));
        let Some(overlay::Overlay::Menu { items, hover, .. }) = self.overlay.take() else {
            return;
        };
        let r = self.menu_rect;
        let inside = x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        let row = y.saturating_sub(r.y + 1) as usize;
        let pick = inside
            && down
            && items
                .get(row)
                .and_then(|it| it.as_ref())
                .map(|it| it.2)
                .unwrap_or(false);
        if pick {
            let (name, act) = items[row]
                .as_ref()
                .map(|it| (it.0.clone(), it.1.clone()))
                .unwrap();
            let _ = name;
            self.run_menu_action(&act);
            return;
        }
        self.overlay = Some(overlay::Overlay::Menu {
            items,
            hover,
            x: r.x,
            y: r.y,
        });
        self.dirty = true;
    }

    fn run_menu_action(&mut self, act: &str) {
        if let Some(rest) = act.strip_prefix("cd:") {
            let idx = rest.parse::<usize>().unwrap_or(usize::MAX);
            let locs: Vec<Loc> = self.loc.crumbs().into_iter().map(|(_, l)| l).collect();
            if let Some(l) = locs.get(idx) {
                self.goto(l.clone());
            }
            self.dirty = true;
            return;
        }
        if let Some(rest) = act.strip_prefix("dir:") {
            let p = PathBuf::from(rest);
            self.goto(Loc::real(p));
            return;
        }
        if let Some(rest) = act.strip_prefix("sel:") {
            let mut it = rest.splitn(2, '\u{1}');
            let (par, name) = (
                it.next().unwrap_or_default().to_string(),
                it.next().unwrap_or_default().to_string(),
            );
            self.goto_sel(Loc::real(PathBuf::from(&par)), Some(name));
            return;
        }
        self.do_action(act);
        self.dirty = true;
    }

    /// visible row → absolute view index
    fn abs_row(&self, y: u16, g: &Geo, main: bool) -> Option<usize> {
        let (area, lv) = if main {
            (g.main, &self.main)
        } else {
            (g.parent, &self.parent_list)
        };
        let r = (y - area.y) as usize;
        let abs = lv.top + r;
        if abs < lv.view.len() {
            Some(abs)
        } else {
            None
        }
    }

    fn mouse_down(&mut self, btn: MouseButton, x: u16, y: u16, ctrl: bool) {
        // bar clicks first
        for c in &self.clicks {
            if c.y == y && x >= c.x0 && x < c.x1 {
                let act = c.act.clone();
                if act == "search" {
                    self.input_focus = true;
                    self.dirty = true;
                } else {
                    self.run_menu_action(&act);
                }
                return;
            }
        }
        if y == 0 || y == 1 {
            return;
        }
        if btn == MouseButton::Right {
            self.click_right(x, y);
            return;
        }
        // sizers
        if let Some(g) = self.geo {
            if y > 1 && x == g.sz0 {
                self.sizer = Some((0, x, x, 0));
                return;
            }
            if y > 1 && x == g.sz1 {
                self.sizer = Some((1, x, x, 0));
                return;
            }
        }
        // main / parent list click
        let Some(g) = self.geo else { return };
        if y >= g.main.y
            && y < g.main.y + g.main.height
            && x >= g.main.x
            && x < g.main.x + g.main.width
        {
            if self.main.overflows(g.main.height) && x + 1 == g.main.x + g.main.width {
                return; // scrollbar column
            }
            self.drag_pending = Some((self.abs_row(y, &g, true).unwrap_or(usize::MAX), x, y));
            if self.abs_row(y, &g, true).is_none() {
                // below the last row: like python, a click on blank clears
                self.main.selected.clear();
                self.main.cursor = 0;
                self.after_move();
            }
            return;
        }
        if y >= g.parent.y
            && y < g.parent.y + g.parent.height
            && x >= g.parent.x
            && x < g.parent.x + g.parent.width
        {
            if let Some(row) = self.abs_row(y, &g, false) {
                let par = self.loc.parent();
                if let (Some(par), Some(e)) = (par, self.parent_list.view.get(row).cloned()) {
                    if e.is_dir && e.path.is_some() {
                        self.goto(par.child(&e));
                    }
                }
            }
        }
        let _ = ctrl;
    }

    fn mouse_drag(&mut self, x: u16, y: u16, ctrl: bool) {
        if let Some((which, down_x, div_x, moved)) = self.sizer {
            let newmoved = moved.max(if x != div_x { 1 } else { 0 });
            self.sizer = Some((which, down_x, div_x, newmoved));
            let w = self.geo.as_ref().map(|g| g.cols.width).unwrap_or(x);
            if x != div_x {
                let pct = if which == 0 {
                    x as i64 * 100 / w.max(1) as i64
                } else {
                    (w.saturating_sub(x)) as i64 * 100 / w.max(1) as i64
                };
                if which == 0 {
                    self.pct.0 = pct;
                } else {
                    self.pct.1 = pct;
                }
            }
            self.dirty = true;
            let _ = down_x;
            return;
        }
        if let Some((row, dx, dy)) = self.drag_pending.take() {
            if row == usize::MAX {
                self.drag_pending = Some((row, dx, dy));
            } else if (x.abs_diff(dx) as i32) > 4 || (y.abs_diff(dy) as i32) > 1 {
                self.drag_start(row);
            } else {
                self.drag_pending = Some((row, dx, dy));
            }
        }
        if self.drag.is_some() {
            self.drag_move(x, y);
            self.dirty = true;
            let _ = ctrl;
        }
    }

    fn mouse_up(&mut self, btn: MouseButton, x: u16, y: u16, ctrl: bool) {
        if btn != MouseButton::Left {
            return;
        }
        if let Some((_, _down_x, _div, moved)) = self.sizer.take() {
            if moved == 0 {
                self.save_panes();
            } // python: plain click on a sizer also saves
            return;
        }
        if let Some((row, _dx, _dy)) = self.drag_pending.take() {
            if row != usize::MAX {
                self.list_click(row, ctrl, false);
            }
            return;
        }
        if self.drag.take().is_some() {
            self.drag_drop(x, y, ctrl);
        }
    }

    fn drag_start(&mut self, row: usize) {
        let Some(e) = self.main.view.get(row) else {
            return;
        };
        let names = if self.main.selected.contains(&e.name) {
            self.main.selected.clone()
        } else {
            [e.name.clone()].into_iter().collect::<HashSet<_>>()
        };
        let drag: Vec<Entry> = self
            .main
            .view
            .iter()
            .filter(|x| names.contains(&x.name) && !x.virtual_())
            .cloned()
            .collect();
        self.drag = Some((drag, false));
        self.notify(&format!(
            "dragging {} item(s): drop on a folder (hold ctrl to copy)",
            { self.drag.as_ref().map(|(d, _)| d.len()).unwrap_or(0) }
        ));
        self.msg_until = Instant::now() + Duration::from_secs(3600);
        self.dirty = true;
    }

    fn drag_move(&mut self, x: u16, y: u16) {
        let _ = (x, y);
        self.main.drop_row = None;
        self.parent_list.drop_row = None;
        let Some(g) = self.geo else { return };
        let Some((drag, _)) = &self.drag else { return };
        let hitm = self
            .abs_row(y, &g, true)
            .and_then(|row| self.main.view.get(row).cloned())
            .filter(|e| e.is_dir && !e.virtual_() && !drag.iter().any(|d| d.path == e.path));
        let hitp = self
            .abs_row(y, &g, false)
            .and_then(|row| self.parent_list.view.get(row).cloned())
            .filter(|e| e.is_dir && !e.virtual_());
        let hit = match (hitm, hitp) {
            (Some(e), _) => self.abs_row(y, &g, true).map(|r| (0u16, r, e)),
            (None, Some(e)) => self.abs_row(y, &g, false).map(|r| (1u16, r, e)),
            _ => None,
        };
        if let Some((which, row, _)) = hit {
            if which == 0 {
                self.main.drop_row = Some(row);
            } else {
                self.parent_list.drop_row = Some(row);
            }
        }
    }

    fn drag_drop(&mut self, _x: u16, _y: u16, ctrl: bool) {
        self.msg.clear();
        self.dirty = true;
        let tgt = {
            let row = self
                .main
                .drop_row
                .map(|r| (r, self.main.view.get(r).cloned()));
            let prow = self
                .parent_list
                .drop_row
                .map(|r| (r, self.parent_list.view.get(r).cloned()));
            if let Some((_, Some(e))) = row {
                Some(e)
            } else if let Some((_, Some(e))) = prow {
                Some(e)
            } else {
                None
            }
        };
        self.main.drop_row = None;
        self.parent_list.drop_row = None;
        let Some((drag, _)) = self.drag.take() else {
            return;
        };
        let Some(tgt) = tgt else { return };
        if drag.iter().any(|e| e.path == tgt.path) {
            return;
        }
        let paths: Vec<PathBuf> = drag.iter().map(|e| e.path.clone().unwrap()).collect();
        let dest = tgt.path.clone().unwrap();
        let n = paths.len();
        self.spawn_job(Job {
            id: 0,
            label: format!(
                "{} {n} item(s) to {}",
                if ctrl { "copying" } else { "moving" },
                tgt.name
            ),
            totals: n as u64,
            kind: JobKind::CopyMove(paths, dest, !ctrl),
            done_text: format!(
                "{} {n} item(s) to {}",
                if ctrl { "copied" } else { "moved" },
                tgt.name
            ),
            select: None,
            after: None,
        });
    }

    /// click on a list row — ctrl toggles, shift range-selects, plain selects,
    /// double-click opens
    fn list_click(&mut self, row: usize, ctrl: bool, shift: bool) {
        let Some(e) = self.main.view.get(row).cloned() else {
            return;
        };
        let mut after = false;
        if ctrl {
            if !self.main.selected.remove(&e.name) {
                self.main.selected.insert(e.name.clone());
            }
            self.main.cursor = row;
            self.anchor = Some(row);
        } else if shift {
            let a = self.anchor.unwrap_or(self.main.cursor);
            let (lo, hi) = (a.min(row), a.max(row));
            self.main.selected = self
                .main
                .view
                .iter()
                .skip(lo)
                .take(hi - lo + 1)
                .map(|x| x.name.clone())
                .collect();
            self.main.cursor = row;
        } else {
            let now = Instant::now();
            if self
                .last_click
                .map(|(r, t)| r == row && now.duration_since(t) < Duration::from_millis(450))
                .unwrap_or(false)
            {
                self.last_click = None;
                self.main.cursor = row;
                self.act_enter();
                return;
            }
            self.last_click = Some((row, now));
            self.main.selected.clear();
            self.main.cursor = row;
            self.anchor = Some(row);
            after = true;
        }
        if after {
            self.after_move();
        }
        self.dirty = true;
    }

    /// right-click: pick the row if it was hit, else the blank-pane menu
    fn click_right(&mut self, x: u16, y: u16) {
        let Some(g) = self.geo else { return };
        if !(y >= g.main.y
            && y < g.main.y + g.main.height
            && x >= g.main.x
            && x < g.main.x + g.main.width)
        {
            return;
        }
        self.context_menu_row(self.abs_row(y, &g, true), x, y);
    }

    fn context_menu_row(&mut self, row: Option<usize>, x: u16, y: u16) {
        let arc = self.loc.arc.is_some();
        let mut selected = self.main.selected.clone();
        let mut cur = self.main.cursor;
        let mut dirty_sel = false;
        if let Some(row) = row {
            if let Some(e) = self.main.view.get(row) {
                if !selected.contains(&e.name) {
                    selected.clear();
                    selected.insert(e.name.clone());
                    cur = row;
                    dirty_sel = true;
                }
            }
        }
        if dirty_sel {
            self.main.selected = selected;
            self.main.cursor = cur;
            self.after_move();
        }
        if row.is_none() {
            self.main.selected.clear();
            self.after_move();
            self.blank_menu(x, y);
            return;
        }
        let ts = self.targets();
        let can_ex = arc
            || (!ts.is_empty()
                && ts
                    .iter()
                    .all(|t| t.kind == crate::model::KIND_ARCHIVE && !t.virtual_()));
        let mut items: Vec<Option<(String, String, bool)>> = vec![
            Some(("Open".into(), "enter".into(), true)),
            None,
            Some(("Cut".into(), "cut".into(), !arc)),
            Some(("Copy".into(), "copy".into(), !arc)),
            Some(("Paste".into(), "paste".into(), self.clip.is_some() && !arc)),
            None,
            Some(("Rename…".into(), "rename".into(), !arc)),
            Some(("Move to Trash".into(), "trash".into(), !arc)),
            None,
        ];
        if can_ex {
            items.push(Some(("Extract Here".into(), "extract_here".into(), true)));
            items.push(Some(("Extract To…".into(), "extract".into(), true)));
        }
        items.push(Some((
            "Compress Here (.zip)".into(),
            "compress_here".into(),
            !arc,
        )));
        items.push(Some(("Compress To…".into(), "compress".into(), !arc)));
        items.push(None);
        items.push(Some(("Copy Path".into(), "copy_path".into(), true)));
        items.push(Some(("Open Terminal Here".into(), "terminal".into(), true)));
        items.push(Some(("Properties".into(), "props".into(), true)));
        self.open_menu(items, x, y);
    }

    fn save_panes(&mut self) {
        let mut cols = BTreeMap::new();
        cols.insert("parent".to_string(), self.pct.0);
        if self.pct.1 > 0 {
            cols.insert("preview".to_string(), self.pct.1);
        }
        self.state.set_cols(cols);
        self.notify("column widths saved");
    }

    /// the right-click menu on an empty main pane
    fn blank_menu(&mut self, x: u16, y: u16) {
        let arc = self.loc.arc.is_some();
        let next = SORTS[(self.sort_idx + 1) % SORTS.len()];
        let items: Vec<Option<(String, String, bool)>> = vec![
            Some(("New Folder…".into(), "newdir".into(), !arc)),
            Some(("New File…".into(), "newfile".into(), !arc)),
            Some(("Paste".into(), "paste".into(), self.clip.is_some() && !arc)),
            None,
            Some((
                if self.show_hidden {
                    "Hide Hidden Files".into()
                } else {
                    "Show Hidden Files".into()
                },
                "hidden".into(),
                true,
            )),
            Some((format!("Sort by {next}"), "sort".into(), true)),
            None,
            Some(("Open Terminal Here".into(), "terminal".into(), true)),
            Some(("Open Graph".into(), "graph".into(), true)),
        ];
        self.open_menu(items, x, y);
    }
}

/// pane layout from the saved percentages, with python's clamps
fn rects(area: Rect, pct: (i64, i64)) -> (Rect, Rect, Rect, u16, u16) {
    let total = area.width;
    if total < 30 {
        return (area, area, area, area.x, area.x);
    }
    let pw = (pct.0.clamp(8, 34) as u16 * total / 100).clamp(2, total - 8);
    let pvw = (pct.1.clamp(14, 55) as u16 * total / 100).clamp(2, total.saturating_sub(pw + 3));
    let mainw = total.saturating_sub(pw + pvw).saturating_sub(2).max(4);
    let sx0 = area.x + pw;
    let mx = sx0 + 1;
    let sx1 = mx + mainw;
    let px = sx1 + 1;
    (
        Rect {
            x: area.x,
            y: area.y,
            width: pw,
            height: area.height,
        },
        Rect {
            x: mx,
            y: area.y,
            width: mainw,
            height: area.height,
        },
        Rect {
            x: px,
            y: area.y,
            width: pvw.min(total.saturating_sub(px - area.x)),
            height: area.height,
        },
        sx0,
        sx1,
    )
}
