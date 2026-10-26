mod dashboard;
mod dialogs;
pub(crate) use dialogs::pr_workspace::cancel_button_area as pr_workspace_cancel_button_area;
pub(crate) use dialogs::pr_workspace::pane_areas as pr_workspace_pane_areas;
pub mod header;
pub(crate) mod list;
pub mod pane;
mod picker;
mod status;
mod toast;

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    widgets::{Block, Clear},
};

use crate::app::App;
use crate::theme::Theme;

pub(crate) use pane::SCROLLBAR_WIDTH;
pub(crate) use pane::normalize_captured_pane;
pub(crate) use pane::render_ansi_lines;
pub(crate) use pane::render_vt100_screen;
pub(crate) use pane::viewing_main_width;
pub(crate) use toast::draw_toasts;

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

/// One footer hint for [`fit_hints`]. A lower `priority` survives a narrower
/// width; priority 0 is never dropped.
#[derive(Clone, Copy)]
pub(crate) struct Hint {
    pub key: &'static str,
    pub label: &'static str,
    pub priority: u8,
}

/// Compose a single-line hint row that fits `width` columns by dropping the
/// highest-priority-number hints first (ties: the later one), so a narrow main
/// pane never clips a hint mid-word. Order of the survivors is preserved.
pub(crate) fn fit_hints(
    hints: &[Hint],
    width: usize,
    key_style: Style,
    label_style: Style,
) -> ratatui::text::Line<'static> {
    use ratatui::text::{Line, Span};
    use unicode_width::UnicodeWidthStr;

    let cost = |h: &Hint| 2 + UnicodeWidthStr::width(h.key) + 1 + UnicodeWidthStr::width(h.label);
    let mut keep = vec![true; hints.len()];
    // " " lead, items separated by two spaces.
    let total = |keep: &[bool]| -> usize {
        1 + hints
            .iter()
            .zip(keep)
            .filter(|(_, k)| **k)
            .map(|(h, _)| cost(h))
            .sum::<usize>()
    };
    while total(&keep) > width {
        let drop = hints
            .iter()
            .enumerate()
            .filter(|(i, h)| keep[*i] && h.priority > 0)
            .max_by_key(|(i, h)| (h.priority, *i))
            .map(|(i, _)| i);
        match drop {
            Some(i) => keep[i] = false,
            None => break,
        }
    }
    let mut spans = vec![Span::raw(" ")];
    let mut first = true;
    for (h, _) in hints.iter().zip(&keep).filter(|(_, k)| **k) {
        if !first {
            spans.push(Span::styled("  ", label_style));
        }
        first = false;
        spans.push(Span::styled(h.key, key_style));
        spans.push(Span::styled(format!(" {}", h.label), label_style));
    }
    Line::from(spans)
}

/// Center a content-sized dialog, clamping it to the current terminal.
pub(crate) fn dialog_rect(viewport: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(viewport.width);
    let height = height.min(viewport.height);
    Rect::new(
        viewport.x + (viewport.width - width) / 2,
        viewport.y + (viewport.height - height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod usability_tests;
