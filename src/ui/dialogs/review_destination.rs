//! Overlays for the final review's destination choice: the destination picker
//! (`t` in the review viewer), the compact companion-feature setup, and the
//! integration overlay (dashboard `t` on a companion review feature).

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::app::{
    ReviewDestinationPickState, TriageFeatureSetupState, TriageIntegrateState, TriageIntegration,
    TriageSetupRow,
};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

/// The destination picker: one modal list of where a finished review's fixes
/// are dispatched.
pub fn draw_review_destination_pick(
    frame: &mut Frame,
    state: &ReviewDestinationPickState,
    theme: &Theme,
) {
    let area = centered_rect(56, 60, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()))
        .title(" Dispatch review fixes to… ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        Line::from(Span::styled(
            " Where the finished review's \"address the feedback\" prompt goes:",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(""),
    ];
    for (i, row) in state.rows.iter().enumerate() {
        let style = if i == state.selected {
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text.to_color())
        };
        lines.push(Line::from(Span::styled(
            format!("  {}", row.label()),
            style,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(" choose  ", Style::default().fg(theme.text.to_color())),
        Span::styled("j/k", Style::default().fg(theme.warning.to_color())),
        Span::styled(" move  ", Style::default().fg(theme.text.to_color())),
        Span::styled("q/Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" keep current", Style::default().fg(theme.text.to_color())),
    ]));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// The compact companion-feature setup overlay (the `New feature…` row).
pub fn draw_review_feature_setup(
    frame: &mut Frame,
    setup: &TriageFeatureSetupState,
    theme: &Theme,
) {
    let area = centered_rect(64, 60, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" New companion review feature ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // explanation
            Constraint::Min(1),    // settings rows
            Constraint::Length(1), // error
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(vec![Line::from(Span::styled(
            "Fixes run in their own worktree, branched from the feature under review, with the \
             settings below. Landing them back on that branch is an explicit step (t on the \
             dashboard).",
            Style::default().fg(theme.text_muted.to_color()),
        ))])
        .wrap(Wrap { trim: true }),
        chunks[0],
    );

    let value_for = |row: TriageSetupRow| -> String {
        match row {
            TriageSetupRow::Preset => setup.preset_label(),
            TriageSetupRow::Harness => setup.agent().display_name().to_string(),
            TriageSetupRow::Mode => {
                format!(
                    "{} — {}",
                    setup.mode.display_name(),
                    setup.mode.description()
                )
            }
            TriageSetupRow::Review => if setup.review { "on" } else { "off" }.to_string(),
            TriageSetupRow::Chrome => if setup.enable_chrome { "on" } else { "off" }.to_string(),
            TriageSetupRow::Branch => setup.branch.clone(),
        }
    };

    let lines: Vec<Line> = TriageSetupRow::ALL
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let is_selected = i == setup.row;
            let marker = if is_selected { ">" } else { " " };
            let value_style = if is_selected {
                Style::default()
                    .fg(theme.text.to_color())
                    .bg(theme.effective_selection_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };
            let mut value = value_for(*row);
            if is_selected && *row == TriageSetupRow::Branch {
                value.push('▏');
            }
            Line::from(vec![
                Span::styled(
                    format!("  {marker} "),
                    Style::default().fg(theme.warning.to_color()),
                ),
                Span::styled(
                    format!("{:<13}", row.label()),
                    Style::default().fg(theme.text_muted.to_color()),
                ),
                Span::styled(value, value_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), chunks[1]);

    if let Some(error) = &setup.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {error}"),
                Style::default()
                    .fg(theme.danger.to_color())
                    .add_modifier(Modifier::BOLD),
            ))),
            chunks[2],
        );
    }

    let hints = if setup.focused_row() == TriageSetupRow::Branch {
        "[⏎] create   [↑/↓] move   [type] edit branch   [esc] cancel"
    } else {
        "[⏎] create   [j/k] move   [h/l] change   [esc] cancel"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[3],
    );
}

/// The integration overlay: what the companion has committed since it branched,
/// and the two non-destructive ways to land it on the source branch.
pub fn draw_review_integrate(frame: &mut Frame, integrate: &TriageIntegrateState, theme: &Theme) {
    let area = centered_rect(70, 66, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Land companion review commits on the source branch ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // branches
            Constraint::Min(1),    // commit preview
            Constraint::Length(3), // option rows
            Constraint::Length(2), // status / error
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(vec![Line::from(vec![
            Span::raw("  "),
            Span::styled(
                integrate.triage_branch.clone(),
                Style::default()
                    .fg(theme.secondary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" → ", Style::default().fg(theme.text_muted.to_color())),
            Span::styled(
                integrate.pr_branch.clone(),
                Style::default()
                    .fg(theme.secondary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ])])
        .wrap(Wrap { trim: true }),
        chunks[0],
    );

    let mut commit_lines: Vec<Line> = Vec::new();
    if integrate.commits.is_empty() {
        commit_lines.push(Line::from(Span::styled(
            "  No commits on the companion branch yet — nothing to land.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
    } else {
        commit_lines.push(Line::from(Span::styled(
            format!("  {} commit(s) to land:", integrate.commits.len()),
            Style::default().fg(theme.text_muted.to_color()),
        )));
        for commit in &integrate.commits {
            commit_lines.push(Line::from(Span::styled(
                format!("    {commit}"),
                Style::default().fg(theme.text.to_color()),
            )));
        }
    }
    if integrate.triage_dirty {
        commit_lines.push(Line::from(Span::styled(
            "  ⚠ the companion worktree has uncommitted changes — they will not be included",
            Style::default().fg(theme.warning.to_color()),
        )));
    }
    frame.render_widget(
        Paragraph::new(commit_lines).wrap(Wrap { trim: false }),
        chunks[1],
    );

    let option_lines: Vec<Line> = TriageIntegration::ALL
        .iter()
        .enumerate()
        .map(|(i, option)| {
            let disabled =
                *option == TriageIntegration::CherryPick && integrate.source_dirty.is_some();
            let is_selected = i == integrate.selected;
            let marker = if is_selected { ">" } else { " " };
            let style = if disabled {
                Style::default().fg(theme.text_muted.to_color())
            } else if is_selected {
                Style::default()
                    .fg(theme.text.to_color())
                    .bg(theme.effective_selection_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };
            let mut spans = vec![
                Span::styled(
                    format!("  {marker} "),
                    Style::default().fg(theme.warning.to_color()),
                ),
                Span::styled(option.label(), style),
            ];
            if disabled {
                spans.push(Span::styled(
                    "  [unavailable]",
                    Style::default().fg(theme.danger.to_color()),
                ));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(option_lines), chunks[2]);

    let mut status: Vec<Line> = Vec::new();
    if let Some(done) = &integrate.done {
        status.push(Line::from(Span::styled(
            format!("  ✓ {done}"),
            Style::default()
                .fg(theme.success.to_color())
                .add_modifier(Modifier::BOLD),
        )));
    }
    if let Some(error) = &integrate.error {
        status.push(Line::from(Span::styled(
            format!("  {error}"),
            Style::default()
                .fg(theme.danger.to_color())
                .add_modifier(Modifier::BOLD),
        )));
    }
    if let Some(reason) = &integrate.source_dirty {
        status.push(Line::from(Span::styled(
            format!("  Cherry-pick unavailable: {reason}"),
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }
    frame.render_widget(Paragraph::new(status).wrap(Wrap { trim: true }), chunks[3]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[⏎] run   [j/k] choose   [esc] close",
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[4],
    );
}
