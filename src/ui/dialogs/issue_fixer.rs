use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::issue_fixer::IssueDuplicateWarningState;
use crate::app::issue_fixer::{IssueSetupRow, IssueSetupState};
use crate::app::{IssueBrowserState, IssueBrowserStatus};
use crate::theme::Theme;

pub fn draw_issue_browser(frame: &mut Frame, state: &IssueBrowserState, theme: &Theme) {
    let block = Block::default()
        .title(format!(
            " GitHub issues · {} ",
            state.repository.canonical()
        ))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(frame.area());
    frame.render_widget(block, frame.area());

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(inner);

    let status = match &state.status {
        IssueBrowserStatus::Loading => " Loading issues…".to_string(),
        IssueBrowserStatus::Ready if state.entries.is_empty() => {
            " No open issues on this page.".to_string()
        }
        IssueBrowserStatus::Ready => format!(
            " {} issue(s) · page {}{}",
            state.entries.len(),
            state.page,
            if state.has_next_page {
                " · more available"
            } else {
                ""
            }
        ),
        IssueBrowserStatus::Error(error) => format!(" Error: {error}"),
    };
    let status_style = if matches!(state.status, IssueBrowserStatus::Error(_)) {
        Style::default().fg(theme.danger.to_color())
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(status, status_style))),
        layout[0],
    );

    if state.entries.is_empty() {
        let body = match &state.status {
            IssueBrowserStatus::Loading => "Loading from GitHub…",
            IssueBrowserStatus::Error(_) => "Press r to retry.",
            IssueBrowserStatus::Ready => "No open issues were returned.",
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {body}"),
                Style::default().fg(theme.text.to_color()),
            )))
            .wrap(Wrap { trim: false }),
            layout[1],
        );
    } else {
        let items = state
            .entries
            .iter()
            .map(|issue| {
                let labels = issue
                    .labels
                    .iter()
                    .map(|label| format!(" [{}]", label.name))
                    .collect::<String>();
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("#{} ", issue.number),
                        Style::default().fg(theme.primary.to_color()),
                    ),
                    Span::styled(
                        format!("{}{}", issue.title, labels),
                        Style::default().fg(theme.text.to_color()),
                    ),
                ]))
            })
            .collect::<Vec<_>>();
        let list = List::new(items).highlight_symbol("> ").highlight_style(
            Style::default()
                .bg(theme.effective_selection_bg())
                .add_modifier(Modifier::BOLD),
        );
        let mut list_state = ListState::default();
        list_state.select(Some(state.selected.min(state.entries.len() - 1)));
        frame.render_stateful_widget(list, layout[1], &mut list_state);
    }

    if let IssueBrowserStatus::Error(error) = &state.status {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {error}"),
                Style::default().fg(theme.danger.to_color()),
            )))
            .wrap(Wrap { trim: false }),
            layout[2],
        );
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(
                " h/← previous   l/→ next   r refresh   j/k select   esc close   page {}",
                state.page
            ),
            Style::default().fg(theme.text_muted.to_color()),
        ))),
        layout[3],
    );
}

pub fn draw_issue_setup(frame: &mut Frame, state: &IssueSetupState, theme: &Theme) {
    let area = super::super::dashboard::centered_rect(78, 86, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);
    let block = Block::default()
        .title(format!(
            " Fix issue #{} · {} · {} ",
            state.browser.entries[state.browser.selected].number,
            state.project_name,
            state.browser.repository.canonical()
        ))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(11),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Edit the feature settings and generated prompt before creating the worktree.",
            Style::default().fg(theme.text_muted.to_color()),
        ))),
        chunks[0],
    );

    let values = [
        state.feature_name.clone(),
        state.settings.branch.clone(),
        state.settings.preset_label(),
        state.settings.agent().display_name().to_string(),
        state.settings.mode.display_name().to_string(),
        if state.settings.review { "On" } else { "Off" }.to_string(),
        if state.settings.enable_chrome {
            "On"
        } else {
            "Off"
        }
        .to_string(),
        if state.use_worktree {
            "New worktree"
        } else {
            "Project workdir"
        }
        .to_string(),
        if !state.plan_mode {
            "Off".to_string()
        } else if state.quick_plan {
            "Quick plan".to_string()
        } else {
            "Plan interview".to_string()
        },
        if state.prompt_editing {
            "Editing…".to_string()
        } else {
            format!("{} chars (press e)", state.prompt.text().chars().count())
        },
    ];
    let rows = IssueSetupRow::ALL
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let selected = *row == state.row;
            let marker = if selected { ">" } else { " " };
            let value_style = if selected {
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };
            Line::from(vec![
                Span::styled(
                    format!(" {marker} {:<14} ", row.label()),
                    Style::default().fg(theme.text_muted.to_color()),
                ),
                Span::styled(values[index].as_str(), value_style),
            ])
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(rows), chunks[1]);

    if let Some(error) = &state.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {error}"),
                Style::default().fg(theme.danger.to_color()),
            ))),
            chunks[2],
        );
    }
    let prompt_lines =
        super::editor_view::editor_lines(&state.prompt, theme, "Generated issue prompt");
    frame.render_widget(
        Paragraph::new(prompt_lines).wrap(Wrap { trim: false }),
        chunks[3],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            if state.prompt_editing {
                "Tab/Esc finish prompt edit"
            } else {
                "j/k move  h/l change  e edit prompt  Enter create  Esc cancel"
            },
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[4],
    );
}

pub fn draw_issue_duplicate_warning(
    frame: &mut Frame,
    state: &IssueDuplicateWarningState,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(62, 38, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);
    let issue = state
        .setup
        .browser
        .entries
        .get(state.setup.browser.selected)
        .map(|issue| issue.number)
        .unwrap_or_default();
    let block = Block::default()
        .title(format!(" Issue #{issue} already has feature work "))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.warning.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
    frame.render_widget(
        Paragraph::new("The same canonical repository and issue number are already linked to:")
            .wrap(Wrap { trim: true }),
        chunks[0],
    );
    let rows = state
        .matches
        .iter()
        .map(|entry| {
            Line::from(Span::styled(
                format!("  {} / {}", entry.project_name, entry.feature_name),
                Style::default().fg(theme.text.to_color()),
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(rows), chunks[1]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Enter/y create another   Esc/n return to setup",
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[2],
    );
}
