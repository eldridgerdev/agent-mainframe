use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::app::SettingsState;

use super::super::dashboard::centered_rect;

pub fn draw_settings_dialog(frame: &mut Frame, state: &SettingsState) {
    let area = centered_rect(50, 50, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Settings ");

    let nerd_font_display = if state.nerd_font { "On" } else { "Off" };
    let transparent_display = if state.transparent_background {
        "On"
    } else {
        "Off"
    };
    let opencode_theme_display = state.opencode_theme.as_deref().unwrap_or("default");

    let settings = [
        ("Nerd Font", nerd_font_display, true),
        ("Transparent Background", transparent_display, true),
        ("OpenCode Theme", opencode_theme_display, false),
    ];

    let lines: Vec<Line> = settings
        .iter()
        .enumerate()
        .map(|(i, (name, value, editable))| {
            let toggle_hint = if *editable { "" } else { " (read-only)" };
            let label = format!(" {} : {}{}", name, value, toggle_hint);
            let style = if i == state.selected_setting {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if *editable {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            Line::from(Span::styled(label, style))
        })
        .collect();

    let hint = Line::from(vec![
        Span::raw(" "),
        Span::styled(
            "j/k",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" navigate, "),
        Span::styled(
            "Enter",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" toggle, "),
        Span::styled(
            "Esc/q",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" close"),
    ]);

    let mut all_lines = lines;
    all_lines.push(Line::default());
    all_lines.push(hint);

    let paragraph = Paragraph::new(all_lines)
        .block(block)
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}
