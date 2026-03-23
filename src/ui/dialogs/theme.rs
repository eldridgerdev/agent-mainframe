use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
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

    let lines: Vec<Line> = state
        .themes
        .iter()
        .enumerate()
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
