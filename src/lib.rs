pub mod archive;
pub mod graph;
pub mod hist;
pub mod model;
pub mod ops;
pub mod particles;
pub mod preview;
pub mod sixel;
pub mod state;
pub mod theme;
pub mod ui;

/// palette colour → ratatui colour (used all over the ui)
pub fn col(c: theme::Col) -> ratatui::style::Color {
    ratatui::style::Color::Rgb(c.0, c.1, c.2)
}
