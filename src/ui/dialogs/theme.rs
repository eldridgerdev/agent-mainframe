use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use crate::app::ThemePickerState;
use crate::theme::{Theme, ThemeName};

use super::super::dashboard::centered_rect;

pub fn draw_theme_picker(
    frame: &mut Frame,
    state: &ThemePickerState,
    current_theme: &ThemeName,
    theme: &Theme,
    transparent: bool,
) {
    let area = centered_rect(40, 40, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()))
        .title(" Theme ");

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let visible_height = chunks[0].height as usize;
    let scroll_offset =
        theme_picker_scroll_offset(state.selected, state.themes.len(), visible_height);

    let lines: Vec<Line> = state
        .themes
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(i, theme_name)| {
            let is_current = theme_name == current_theme;
            let marker = if is_current { " *" } else { "" };
            let label = format!(" {}{}", theme_name.display_name(), marker,);
            let style = if i == state.selected {
                Style::default()
                    .fg(theme.shortcut_text.to_color())
                    .bg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD)
            } else if is_current {
                Style::default().fg(theme.primary.to_color())
            } else {
                Style::default().fg(theme.text.to_color())
            };
            Line::from(Span::styled(label, style))
        })
        .collect();

    let list = Paragraph::new(lines);
    frame.render_widget(list, chunks[0]);

    if state.themes.len() > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(state.themes.len())
            .position(scroll_offset)
            .viewport_content_length(visible_height);
        let scrollbar_area = Rect {
            x: chunks[0].x + chunks[0].width.saturating_sub(1),
            y: chunks[0].y,
            width: 1,
            height: chunks[0].height,
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }

    let transparent_label = if transparent { "on" } else { "off" };
    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" j/k", Style::default().fg(theme.warning.to_color())),
        Span::raw(" select  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" apply  "),
        Span::styled("t", Style::default().fg(theme.warning.to_color())),
        Span::raw(format!(" transparent: {}  ", transparent_label)),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" close"),
    ]));
    frame.render_widget(hints, chunks[1]);
}

fn theme_picker_scroll_offset(selected: usize, total: usize, visible: usize) -> usize {
    if visible == 0 {
        return 0;
    }

    selected
        .saturating_add(1)
        .saturating_sub(visible)
        .min(total.saturating_sub(visible))
}

#[cfg(test)]
mod tests {
    use super::theme_picker_scroll_offset;

    #[test]
    fn picker_does_not_scroll_while_selection_is_visible() {
        assert_eq!(theme_picker_scroll_offset(0, 28, 8), 0);
        assert_eq!(theme_picker_scroll_offset(7, 28, 8), 0);
    }

    #[test]
    fn picker_scrolls_to_keep_selection_visible() {
        assert_eq!(theme_picker_scroll_offset(8, 28, 8), 1);
        assert_eq!(theme_picker_scroll_offset(20, 28, 8), 13);
        assert_eq!(theme_picker_scroll_offset(27, 28, 8), 20);
    }

    #[test]
    fn picker_handles_empty_viewport() {
        assert_eq!(theme_picker_scroll_offset(10, 28, 0), 0);
    }
}
