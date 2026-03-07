use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::{RenameFeatureState, RenameSessionState};

use super::super::dashboard::centered_rect;

pub fn draw_rename_session_dialog(frame: &mut Frame, state: &RenameSessionState) {
    let area = centered_rect(50, 25, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Rename Session ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    let name_field = Paragraph::new(Line::from(vec![
        Span::styled(" Name: ", Style::default().fg(Color::Cyan)),
        Span::styled(&state.input, Style::default().fg(Color::White)),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(name_field, chunks[0]);
}

pub fn draw_rename_feature_dialog(frame: &mut Frame, state: &RenameFeatureState) {
    let area = centered_rect(50, 25, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Rename Feature ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(0)])
        .split(inner);

    let name_field = Paragraph::new(Line::from(vec![
        Span::styled(" Nickname: ", Style::default().fg(Color::Cyan)),
        Span::styled(&state.input, Style::default().fg(Color::White)),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]));
    frame.render_widget(name_field, chunks[0]);
}
