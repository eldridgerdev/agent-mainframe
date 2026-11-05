use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::ui::dashboard::centered_rect;
use crate::{app::DiffPickerState, theme::Theme, ui::draw_modal_overlay};

pub fn draw_diff_picker(frame: &mut Frame, state: &DiffPickerState, theme: &Theme) {
    let area = centered_rect(72, 70, frame.area());
    draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Choose Diff Scope ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(inner);

    // One line under the intro: the commit count, or why there are none.
    // The raw git error is in the debug log, not here.
    let status_line = if let Some(error) = &state.error {
        Span::styled(
            format!(" {error}"),
            Style::default().fg(theme.warning.to_color()),
        )
    } else if state.no_base_branch {
        Span::styled(
            " No base branch found: showing uncommitted changes against HEAD.",
            Style::default().fg(theme.warning.to_color()),
        )
    } else {
        Span::styled(
            format!(" {} commit(s) on this feature branch", state.commits.len()),
            Style::default().fg(theme.text_muted.to_color()),
        )
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(" View the whole branch and worktree, or isolate one commit."),
            Line::from(status_line),
        ]),
        rows[0],
    );

    let mut items = vec![ListItem::new(Line::from(vec![
        Span::styled(
            "All current changes",
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            if state.no_base_branch {
                "  staged + unstaged + untracked vs HEAD"
            } else {
                "  commits + staged + unstaged + untracked"
            },
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]))];
    items.extend(state.commits.iter().map(|commit| {
        ListItem::new(Line::from(vec![
            Span::styled(
                format!("{}  ", commit.short_hash),
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                commit.subject.clone(),
                Style::default().fg(theme.text.to_color()),
            ),
        ]))
    }));

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme.effective_selection_bg())
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut list_state = ListState::default();
    list_state.select(Some(state.selected.min(state.commits.len())));
    frame.render_stateful_widget(list, rows[1], &mut list_state);

    if state.no_base_branch || state.base_unresolved {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " Press b in the viewer to compare against another ref.",
                Style::default().fg(theme.text_muted.to_color()),
            )))
            .wrap(Wrap { trim: false }),
            rows[2],
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" j/k", Style::default().fg(theme.warning.to_color())),
            Span::raw(" move  "),
            Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" view  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" cancel"),
        ])),
        rows[3],
    );
}
