use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use crate::app::SearchState;

use super::super::dashboard::centered_rect;

pub fn draw_search_dialog(frame: &mut Frame, state: &SearchState) {
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Search ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    let query_line = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(&state.query, Style::default().fg(Color::White)),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(query_line, chunks[0]);

    if state.matches.is_empty() {
        if !state.query.is_empty() {
            let no_results = Paragraph::new(Line::from(Span::styled(
                " No matches found",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(no_results, chunks[1]);
        } else {
            let placeholder = Paragraph::new(Line::from(Span::styled(
                " Type to search projects, features, and sessions...",
                Style::default().fg(Color::DarkGray),
            )));
            frame.render_widget(placeholder, chunks[1]);
        }
    } else {
        let visible_matches: Vec<ListItem> = state
            .matches
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let is_selected = i == state.selected_match;
                let marker = if is_selected { ">" } else { " " };
                let style = if is_selected {
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                let line = Line::from(vec![
                    Span::styled(format!(" {} ", marker), Style::default().fg(Color::Cyan)),
                    Span::styled(&m.label, style),
                    Span::styled(
                        format!("  {}", m.context),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]);
                if is_selected {
                    ListItem::new(line).style(Style::default().bg(Color::DarkGray))
                } else {
                    ListItem::new(line)
                }
            })
            .collect();

        let list = List::new(visible_matches);
        frame.render_widget(list, chunks[1]);
    }

    let count_text = if state.matches.is_empty() {
        String::new()
    } else {
        format!(" {} / {}", state.selected_match + 1, state.matches.len())
    };
    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            " j/k or \u{2191}/\u{2193}",
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(" navigate  "),
        Span::styled("Enter", Style::default().fg(Color::Yellow)),
        Span::raw(" jump  "),
        Span::styled("Esc", Style::default().fg(Color::Yellow)),
        Span::raw(" cancel"),
        Span::raw("  "),
        Span::styled(count_text, Style::default().fg(Color::DarkGray)),
    ]));
    frame.render_widget(hints, chunks[2]);
}
