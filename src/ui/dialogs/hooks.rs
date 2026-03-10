use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::app::{DiffReviewState, HookPromptState, RunningHookState};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_diff_review_dialog(frame: &mut Frame, state: &DiffReviewState, theme: &Theme) {
    let area = centered_rect(88, 74, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Diff Review ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // file
            Constraint::Length(1), // tool
            Constraint::Min(7),    // diff
            Constraint::Min(6),    // explanation
            Constraint::Length(1), // hints
        ])
        .split(inner);

    let file_line = Paragraph::new(Line::from(vec![
        Span::styled(" File: ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled(
            &state.relative_path,
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(file_line, chunks[0]);

    let tool_label = if state.tool == "edit" {
        "EDIT"
    } else {
        "WRITE"
    };
    let tool_line = Paragraph::new(Line::from(vec![
        Span::styled(" Tool: ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled(tool_label, Style::default().fg(theme.warning.to_color())),
    ]));
    frame.render_widget(tool_line, chunks[1]);

    if state.side_by_side {
        let diff_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(chunks[2]);

        let old_lines = if state.old_snippet.is_empty() {
            vec![Line::from(Span::styled(
                " (no removed content)",
                Style::default().fg(theme.text_muted.to_color()),
            ))]
        } else {
            state
                .old_snippet
                .lines()
                .take(8)
                .map(|line| {
                    let truncated = if line.len() > 44 { &line[..44] } else { line };
                    Line::from(Span::styled(
                        format!("- {truncated}"),
                        Style::default().fg(theme.danger.to_color()),
                    ))
                })
                .collect::<Vec<_>>()
        };
        let new_lines = if state.new_snippet.is_empty() {
            vec![Line::from(Span::styled(
                " (no added content)",
                Style::default().fg(theme.text_muted.to_color()),
            ))]
        } else {
            state
                .new_snippet
                .lines()
                .take(8)
                .map(|line| {
                    let truncated = if line.len() > 44 { &line[..44] } else { line };
                    Line::from(Span::styled(
                        format!("+ {truncated}"),
                        Style::default().fg(theme.success.to_color()),
                    ))
                })
                .collect::<Vec<_>>()
        };

        let old_widget = Paragraph::new(old_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled(
                        " removed ",
                        Style::default().fg(theme.danger.to_color()),
                    ))
                    .border_style(Style::default().fg(theme.danger.to_color())),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(old_widget, diff_chunks[0]);

        let new_widget = Paragraph::new(new_lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(Span::styled(
                        " added ",
                        Style::default().fg(theme.success.to_color()),
                    ))
                    .border_style(Style::default().fg(theme.success.to_color())),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(new_widget, diff_chunks[1]);
    } else {
        let mut diff_lines = vec![];

        if !state.old_snippet.is_empty() {
            diff_lines.push(Line::from(Span::styled(
                " Removed:",
                Style::default()
                    .fg(theme.danger.to_color())
                    .add_modifier(Modifier::BOLD),
            )));
            for line in state.old_snippet.lines().take(3) {
                let truncated = if line.len() > 70 { &line[..70] } else { line };
                diff_lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(truncated, Style::default().fg(theme.danger.to_color())),
                ]));
            }
            if state.old_snippet.lines().count() > 3 {
                diff_lines.push(Line::from(Span::styled(
                    "  ...",
                    Style::default().fg(theme.text_muted.to_color()),
                )));
            }
        }

        diff_lines.push(Line::from(""));
        diff_lines.push(Line::from(Span::styled(
            " Added:",
            Style::default()
                .fg(theme.success.to_color())
                .add_modifier(Modifier::BOLD),
        )));

        let new_preview: String = state
            .new_snippet
            .lines()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        let truncated = if new_preview.len() > 60 {
            format!("{}...", &new_preview[..57])
        } else {
            new_preview
        };
        diff_lines.push(Line::from(vec![
            Span::styled(" + ", Style::default().fg(theme.success.to_color())),
            Span::styled(truncated, Style::default().fg(theme.success.to_color())),
        ]));

        let diff_widget = Paragraph::new(diff_lines).wrap(Wrap { trim: false });
        frame.render_widget(diff_widget, chunks[2]);
    }

    let explanation_lines = if let Some(explanation) = &state.explanation {
        explanation
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    format!(" {line}"),
                    Style::default().fg(theme.text.to_color()),
                ))
            })
            .collect::<Vec<_>>()
    } else {
        vec![Line::from(Span::styled(
            " Press e to explain these changes.",
            Style::default().fg(theme.text_muted.to_color()),
        ))]
    };
    let explanation_widget = Paragraph::new(explanation_lines)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.text_muted.to_color()))
                .title(Span::styled(
                    " explanation ",
                    Style::default().fg(theme.text_muted.to_color()),
                )),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(explanation_widget, chunks[3]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" approve  "),
        Span::styled("e", Style::default().fg(theme.info.to_color())),
        Span::raw(" explain  "),
        Span::styled("v", Style::default().fg(theme.primary.to_color())),
        Span::raw(if state.side_by_side {
            " stacked  "
        } else {
            " side-by-side  "
        }),
        Span::styled("r", Style::default().fg(theme.danger.to_color())),
        Span::raw(" feedback  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[4]);

    if state.editing_feedback {
        let feedback_area = centered_rect(64, 18, frame.area());
        frame.render_widget(Clear, feedback_area);

        let feedback_block = Block::default()
            .title(" Reject With Feedback ")
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.effective_bg()))
            .border_style(Style::default().fg(theme.danger.to_color()));
        let feedback_inner = feedback_block.inner(feedback_area);
        frame.render_widget(feedback_block, feedback_area);

        let feedback_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(1),
            ])
            .split(feedback_inner);

        let feedback_line = Paragraph::new(Line::from(vec![
            Span::styled(" Feedback: ", Style::default().fg(theme.success.to_color())),
            Span::styled(&state.reason, Style::default().fg(theme.text.to_color())),
            Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color())),
        ]))
        .wrap(Wrap { trim: false });
        frame.render_widget(feedback_line, feedback_chunks[0]);

        let feedback_hints = Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(theme.danger.to_color())),
            Span::raw(" submit reject  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" back"),
        ]));
        frame.render_widget(feedback_hints, feedback_chunks[1]);
    }
}

pub fn draw_running_hook_dialog(
    frame: &mut Frame,
    state: &RunningHookState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = centered_rect(90, 70, frame.area());
    frame.render_widget(Clear, area);

    let is_running = state.child.is_some();
    let border_color = if is_running {
        theme.primary.to_color()
    } else if state.success.unwrap_or(false) {
        theme.success.to_color()
    } else {
        theme.danger.to_color()
    };

    let block = Block::default()
        .title(" Running Hook ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(inner);

    let status_text = if is_running {
        let throbber = throbber_widgets_tui::Throbber::default()
            .throbber_style(
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            )
            .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
            .use_type(throbber_widgets_tui::WhichUse::Spin);
        let span = throbber.to_symbol_span(throbber_state);
        Line::from(vec![
            Span::styled(" ", Style::default()),
            span,
            Span::styled(
                " Running hook...",
                Style::default().fg(theme.primary.to_color()),
            ),
        ])
    } else if state.success.unwrap_or(false) {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✓ ", Style::default().fg(theme.success.to_color())),
            Span::styled(
                "Hook completed successfully",
                Style::default().fg(theme.success.to_color()),
            ),
        ])
    } else {
        Line::from(vec![
            Span::styled(" ", Style::default()),
            Span::styled("✗ ", Style::default().fg(theme.danger.to_color())),
            Span::styled("Hook failed", Style::default().fg(theme.danger.to_color())),
        ])
    };
    frame.render_widget(Paragraph::new(status_text), chunks[0]);

    let script_line = Paragraph::new(Line::from(vec![
        Span::styled(
            " Script: ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            state.script.clone(),
            Style::default().fg(theme.text.to_color()),
        ),
    ]))
    .wrap(Wrap { trim: false });
    frame.render_widget(script_line, chunks[1]);

    // Show the last N lines of captured stdout/stderr output.
    // Subtract 1 for the Borders::TOP header row.
    let output_height = chunks[2].height.saturating_sub(1) as usize;
    let all_lines: Vec<&str> = state.output.lines().collect();
    let start = all_lines.len().saturating_sub(output_height);
    let output_lines: Vec<Line> = if all_lines.is_empty() {
        vec![Line::from(Span::styled(
            " (no output yet)",
            Style::default().fg(theme.text_muted.to_color()),
        ))]
    } else {
        all_lines[start..]
            .iter()
            .map(|l| {
                Line::from(Span::styled(
                    format!(" {}", l),
                    Style::default().fg(theme.text.to_color()),
                ))
            })
            .collect()
    };
    let output_para = Paragraph::new(output_lines)
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.text_muted.to_color()))
                .title(Span::styled(
                    " output ",
                    Style::default().fg(theme.text_muted.to_color()),
                )),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(output_para, chunks[2]);

    let hints = if is_running {
        Paragraph::new(Line::from(vec![
            Span::styled(" h", Style::default().fg(theme.warning.to_color())),
            Span::styled(" hide  ", Style::default().fg(theme.text_muted.to_color())),
        ]))
    } else {
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
            Span::raw(" continue  "),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::raw(" skip"),
        ]))
    };
    frame.render_widget(hints, chunks[3]);
}

pub fn draw_hook_prompt_dialog(frame: &mut Frame, state: &HookPromptState, theme: &Theme) {
    let area = centered_rect(60, 50, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(format!(" {} ", state.title))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let items: Vec<ListItem> = state
        .options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            if i == state.selected {
                ListItem::new(Line::from(vec![
                    Span::styled(
                        " > ",
                        Style::default()
                            .fg(theme.primary.to_color())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        opt.as_str(),
                        Style::default()
                            .fg(theme.primary.to_color())
                            .add_modifier(Modifier::BOLD),
                    ),
                ]))
            } else {
                ListItem::new(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(opt.as_str(), Style::default().fg(theme.text.to_color())),
                ]))
            }
        })
        .collect();

    let list = List::new(items);
    frame.render_widget(list, chunks[0]);

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(" j/k", Style::default().fg(theme.warning.to_color())),
        Span::raw(" move  "),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" confirm  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel"),
    ]));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_latest_prompt_dialog(frame: &mut Frame, prompt: &str, theme: &Theme) {
    let area = centered_rect(80, 70, frame.area());
    frame.render_widget(Clear, area);

    let block = Block::default()
        .title(" Latest Prompt ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let paragraph = Paragraph::new(prompt)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(theme.text.to_color()));
    frame.render_widget(paragraph, chunks[0]);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled("/q", Style::default().fg(theme.warning.to_color())),
        Span::styled(" close", Style::default().fg(theme.text_muted.to_color())),
    ]));
    frame.render_widget(hint, chunks[1]);
}
