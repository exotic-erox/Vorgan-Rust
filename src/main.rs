//! vorgan — the terminal file manager (rust port).

use std::io::Write;
use std::path::PathBuf;

fn main() {
    // both --flag value and --flag=value work; the shell launchers use =
    let mut start: Option<PathBuf> = None;
    let (mut cwd_file, mut scheme) = (None, None);
    let mut args = std::env::args().skip(1).collect::<Vec<_>>().into_iter();
    while let Some(a) = args.next() {
        if let Some((flag, v)) = a.split_once('=') {
            match flag {
                "--cwd-file" => cwd_file = Some(PathBuf::from(v)),
                "--scheme" => scheme = Some(PathBuf::from(v)),
                _ => {
                    println!("bad flag: {a}");
                    std::process::exit(2);
                }
            }
        } else if a == "--cwd-file" {
            cwd_file = args.next().map(PathBuf::from);
        } else if a == "--scheme" {
            scheme = Some(PathBuf::from(args.next().unwrap_or_default()));
        } else if a.starts_with('-') {
            println!("bad flag: {a}");
            std::process::exit(2);
        } else {
            start = Some(PathBuf::from(a));
        }
    }
    let start =
        start.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let cfg = vorgan::state::Config::load();
    let state = vorgan::state::State::load();
    let spath = scheme.clone().unwrap_or_else(vorgan::theme::scheme_path);
    let (pal, _) = vorgan::theme::Palette::load(&spath);

    let mut out = std::io::stdout();
    crossterm::terminal::enable_raw_mode().expect("raw mode");
    let _ = crossterm::execute!(
        out,
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    );
    let backend = ratatui::backend::CrosstermBackend::new(std::io::stdout());
    let mut term = ratatui::Terminal::new(backend).expect("terminal");
    let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Hide);

    let mut app = vorgan::ui::App::new(cfg, state, pal, start.clone(), scheme);
    let res = app.run(&mut term);

    let _ = crossterm::execute!(
        std::io::stdout(),
        crossterm::cursor::Show,
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen
    );
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = term.show_cursor();

    if res.is_ok() {
        let fin: Option<String> = app.quit.iter().flat_map(|q| q.iter()).next().cloned();
        if let (Some(path), Some(target)) = (cwd_file, fin) {
            if let Ok(mut f) = std::fs::File::create(path) {
                let _ = f.write_all(target.as_bytes());
            }
        }
    }
}
