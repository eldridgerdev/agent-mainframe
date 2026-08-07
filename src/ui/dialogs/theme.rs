use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use crate::app::{ThemePickerEntry, ThemePickerState};
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

    let title = match &state.group {
        Some(group) => format!(" Theme › {} ", group.label),
        None => " Theme ".to_string(),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()))
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let visible_height = chunks[0].height as usize;
    let (total, selected) = match &state.group {
        Some(group) => (group.themes.len(), group.selected),
        None => (state.entries.len(), state.selected),
    };
    let scroll_offset = theme_picker_scroll_offset(selected, total, visible_height);

    let lines: Vec<Line> = if let Some(group) = &state.group {
        group
            .themes
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(visible_height)
            .map(|(i, theme_name)| {
                let label = strip_group_prefix(theme_name.display_name(), group.label);
                render_row(
                    i == group.selected,
                    theme_name == current_theme,
                    &label,
                    theme,
                )
            })
            .collect()
    } else {
        state
            .entries
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(visible_height)
            .map(|(i, entry)| {
                let is_selected = i == state.selected;
                match entry {
                    ThemePickerEntry::Theme(theme_name) => render_row(
                        is_selected,
                        theme_name == current_theme,
                        theme_name.display_name(),
                        theme,
                    ),
                    ThemePickerEntry::Group { label, themes } => {
                        let is_current = themes.contains(current_theme);
                        let group_label = format!("{} \u{25b8} ({})", label, themes.len());
                        render_row(is_selected, is_current, &group_label, theme)
                    }
                }
            })
            .collect()
    };

    let list = Paragraph::new(lines);
    frame.render_widget(list, chunks[0]);

    if total > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(total)
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
    let mut hint_spans = vec![
        Span::styled(" j/k", Style::default().fg(theme.warning.to_color())),
        Span::raw(" select  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(if state.group.is_some() {
            " apply  "
        } else {
            " apply/open  "
        }),
        Span::styled("t", Style::default().fg(theme.warning.to_color())),
        Span::raw(format!(" transparent: {}  ", transparent_label)),
    ];
    if state.group.is_some() {
        hint_spans.push(Span::styled(
            "Esc",
            Style::default().fg(theme.warning.to_color()),
        ));
        hint_spans.push(Span::raw(" back"));
    } else {
        hint_spans.push(Span::styled(
            "Esc",
            Style::default().fg(theme.warning.to_color()),
        ));
        hint_spans.push(Span::raw(" close"));
    }
    let hints = Paragraph::new(Line::from(hint_spans));
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

fn render_row<'a>(is_selected: bool, is_current: bool, label: &str, theme: &Theme) -> Line<'a> {
    let marker = if is_current { " *" } else { "" };
    let text = format!(" {}{}", label, marker);
    let style = if is_selected {
        Style::default()
            .fg(theme.shortcut_text.to_color())
            .bg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else if is_current {
        Style::default().fg(theme.primary.to_color())
    } else {
        Style::default().fg(theme.text.to_color())
    };
    Line::from(Span::styled(text, style))
}

/// Strips a group's own name from a theme's display name so the second
/// screen doesn't repeat it on every row (e.g. "Gruvbox Material Dark Hard"
/// under the "Gruvbox Material" group becomes "Dark Hard").
fn strip_group_prefix(display_name: &str, group_label: &str) -> String {
    display_name
        .strip_prefix(group_label)
        .map(|rest| rest.trim_start().to_string())
        .filter(|rest| !rest.is_empty())
        .unwrap_or_else(|| display_name.to_string())
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
