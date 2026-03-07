use ratatui::{
    Frame,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::ThemePickerState;
use crate::theme::{Theme, ThemeName};

use super::super::dashboard::centered_rect;

pub fn draw_theme_picker(
    frame: &mut Frame,
    state: &ThemePickerState,
    current_theme: &ThemeName,
    theme: &Theme,
) {
    let area = centered_rect(40, 40, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()))
        .title(" Theme ");

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

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}
