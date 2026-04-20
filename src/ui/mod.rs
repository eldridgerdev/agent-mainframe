mod dashboard;
mod dialogs;
mod header;
mod list;
mod pane;
mod picker;
mod status;

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    widgets::{Block, Clear},
};

use crate::app::App;
use crate::theme::Theme;

pub(crate) use pane::render_ansi_lines;
pub(crate) use pane::render_vt100_screen;
pub(crate) use pane::viewing_main_width;

pub fn draw(frame: &mut Frame, app: &mut App) {
    dashboard::draw(frame, app);
}

pub(crate) fn draw_modal_overlay(frame: &mut Frame, area: Rect, theme: &Theme) {
    let viewport = frame.area();
    frame.render_widget(Clear, viewport);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.effective_bg())),
        viewport,
    );

    let shadow = Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(1),
        area.height.saturating_sub(1),
    );
    if shadow.width > 0 && shadow.height > 0 {
        frame.render_widget(
            Block::default().style(Style::default().bg(theme.background.to_color())),
            shadow,
        );
    }

    frame.render_widget(Clear, area);
}
