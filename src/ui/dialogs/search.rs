use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use crate::app::SearchState;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_search_dialog(frame: &mut Frame, state: &SearchState, theme: &Theme) {
    let area = centered_rect(70, 60, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Search ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

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
        Span::styled(&state.query, Style::default().fg(theme.text.to_color())),
        Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color())),
    ]));
    frame.render_widget(query_line, chunks[0]);

    if state.matches.is_empty() {
        if !state.query.is_empty() {
            let no_results = Paragraph::new(Line::from(Span::styled(
                " No matches found",
                Style::default().fg(theme.text_muted.to_color()),
            )));
            frame.render_widget(no_results, chunks[1]);
        } else {
            let placeholder = Paragraph::new(Line::from(Span::styled(
                " Type to search projects, features, and sessions...",
                Style::default().fg(theme.text_muted.to_color()),
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
                        .fg(theme.primary.to_color())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.text.to_color())
                };
                let line = Line::from(vec![
                    Span::styled(
                        format!(" {} ", marker),
                        Style::default().fg(theme.primary.to_color()),
                    ),
                    Span::styled(&m.label, style),
                    Span::styled(
                        format!("  {}", m.context),
                        Style::default().fg(theme.text_muted.to_color()),
                    ),
                ]);
                if is_selected {
                    ListItem::new(line).style(Style::default().bg(theme.effective_selection_bg()))
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
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::raw(" navigate  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" jump  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel"),
        Span::raw("  "),
        Span::styled(count_text, Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hints, chunks[2]);
}
