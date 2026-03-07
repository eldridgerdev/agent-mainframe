use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::app::BrowsePathState;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_browse_path_dialog(frame: &mut Frame, state: &BrowsePathState, theme: &Theme) {
    let area = centered_rect(80, 70, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Browse for Directory ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(inner);

    let cwd_line = Paragraph::new(Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            state.explorer.cwd().to_string_lossy().to_string(),
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(cwd_line, chunks[0]);

    if state.creating_folder {
        let input_area = chunks[1];
        let input_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(input_area);

        let prompt = Paragraph::new(Line::from(vec![
            Span::styled(
                " Create folder: ",
                Style::default().fg(theme.warning.to_color()),
            ),
            Span::styled(
                &state.new_folder_name,
                Style::default().fg(theme.text.to_color()),
            ),
            Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color())),
        ]));
        frame.render_widget(prompt, input_chunks[0]);
    } else {
        frame.render_widget(&state.explorer.widget(), chunks[1]);
    }

    let hints = if state.creating_folder {
        Paragraph::new(vec![
            Line::from(Span::styled(
                "\u{2500}".repeat(inner.width as usize),
                Style::default().fg(theme.text_muted.to_color()),
            )),
            Line::from(vec![
                Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
                Span::raw(" create  "),
                Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
                Span::raw(" cancel"),
            ]),
        ])
    } else {
        Paragraph::new(vec![
            Line::from(Span::styled(
                "\u{2500}".repeat(inner.width as usize),
                Style::default().fg(theme.text_muted.to_color()),
            )),
            Line::from(vec![
                Span::styled(" Space", Style::default().fg(theme.warning.to_color())),
                Span::raw(" select  "),
                Span::styled("Enter/l", Style::default().fg(theme.warning.to_color())),
                Span::raw(" open  "),
                Span::styled("c", Style::default().fg(theme.warning.to_color())),
                Span::raw(" create folder  "),
                Span::styled("h/BS", Style::default().fg(theme.warning.to_color())),
                Span::raw(" parent  "),
                Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
                Span::raw(" cancel"),
            ]),
        ])
    };
    frame.render_widget(hints, chunks[2]);
}
