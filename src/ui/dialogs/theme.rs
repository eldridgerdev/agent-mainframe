use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::app::ThemePickerState;
use crate::theme::ThemeName;

use super::super::dashboard::centered_rect;

pub fn draw_theme_picker(frame: &mut Frame, state: &ThemePickerState, current_theme: &ThemeName) {
    let area = centered_rect(40, 40, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Theme ");

    let lines: Vec<Line> = state
        .themes
        .iter()
        .enumerate()
        .map(|(i, theme)| {
            let is_current = theme == current_theme;
            let marker = if is_current { " *" } else { "" };
            let label = format!(" {}{}", theme.display_name(), marker,);
            let style = if i == state.selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if is_current {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::White)
            };
            Line::from(Span::styled(label, style))
        })
        .collect();

    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, area);
}
