mod dashboard;
mod dialogs;
pub mod header;
mod list;
pub mod pane;
mod picker;
mod status;
mod toast;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    widgets::{Block, Clear},
};

use crate::app::App;
use crate::context_display::ContextIndicator;
use crate::context_tracking::ContextBand;
use crate::theme::Theme;

pub(crate) use pane::SCROLLBAR_WIDTH;
pub(crate) use pane::normalize_captured_pane;
pub(crate) use pane::render_ansi_lines;
pub(crate) use pane::render_vt100_screen;
pub(crate) use pane::viewing_main_width;
pub(crate) use toast::draw_toasts;

pub(crate) fn context_indicator_style(indicator: &ContextIndicator, theme: &Theme) -> Style {
    let color = match indicator.band {
        ContextBand::Normal => theme.success.to_color(),
        ContextBand::Warning => theme.warning.to_color(),
        ContextBand::Critical => theme.danger.to_color(),
    };
    let modifier = if indicator.band == ContextBand::Normal {
        Modifier::empty()
    } else {
        Modifier::BOLD
    };
    Style::default().fg(color).add_modifier(modifier)
}

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
