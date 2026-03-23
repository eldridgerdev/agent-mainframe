use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
};

use crate::app::{DiffReviewState, DiffViewerLayout, HookPromptState, RunningHookState};
use crate::highlight;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;
use super::diff::{PatchPanelOptions, draw_patch_panel};

fn diff_review_uses_new_file_presentation(state: &DiffReviewState) -> bool {
    state.diff_file.as_ref().is_some_and(|file| {
        matches!(
            file.status,
            crate::diff::DiffFileStatus::Added | crate::diff::DiffFileStatus::Untracked
        )
    })
}

fn diff_review_language_status(
    state: &DiffReviewState,
) -> Option<(
    highlight::HighlightLanguage,
    highlight::HighlightInstallState,
)> {
    highlight::language_install_state_for_path(std::path::Path::new(&state.relative_path))
}

pub fn draw_diff_review_dialog(
    frame: &mut Frame,
    state: &DiffReviewState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = centered_rect(92, 82, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Diff Review ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let explanation_loading = state.explanation_child.is_some();
    let explanation_open = state.explanation.is_some() || explanation_loading;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // file
            Constraint::Length(1), // tool
            Constraint::Min(7),    // diff
            if explanation_open {
                Constraint::Length(6)
            } else {
                Constraint::Length(2)
            },
            Constraint::Length(2), // hints
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

    let new_file_presentation = diff_review_uses_new_file_presentation(state);
    let patch_layout = if new_file_presentation {
        DiffViewerLayout::Unified
    } else {
        state.layout.clone()
    };

    if let Some(file) = &state.diff_file {
        draw_patch_panel(
            frame,
            chunks[2],
            Some(file),
            PatchPanelOptions {
                layout: patch_layout.clone(),
                title: if new_file_presentation {
                    format!("New File: {}", state.relative_path)
                } else {
                    format!("Patch: {}", state.relative_path)
                },
                border_color: theme.primary.to_color(),
                scroll: state.patch_scroll,
                include_prologue: new_file_presentation,
                new_file_presentation,
            },
            theme,
        );
    } else {
        let mut diff_lines = vec![];

        if let Some(error) = &state.diff_error {
            diff_lines.push(Line::from(Span::styled(
                format!(" Diff preview unavailable: {error}"),
                Style::default().fg(theme.warning.to_color()),
            )));
            diff_lines.push(Line::from(""));
        }

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

    if explanation_loading {
        let throbber = throbber_widgets_tui::Throbber::default()
            .throbber_style(
                Style::default()
                    .fg(theme.info.to_color())
                    .add_modifier(Modifier::BOLD),
            )
            .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
            .use_type(throbber_widgets_tui::WhichUse::Spin);
        let span = throbber.to_symbol_span(throbber_state);
        let explanation_widget = Paragraph::new(Line::from(vec![
            Span::styled(" ", Style::default()),
            span,
            Span::styled(
                " Generating explanation...",
                Style::default().fg(theme.info.to_color()),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.text_muted.to_color()))
                .title(Span::styled(
                    " explanation ",
                    Style::default().fg(theme.text_muted.to_color()),
                )),
        );
        frame.render_widget(explanation_widget, chunks[3]);
    } else if let Some(explanation) = &state.explanation {
        let explanation_lines = explanation
            .lines()
            .map(|line| {
                Line::from(Span::styled(
                    format!(" {line}"),
                    Style::default().fg(theme.text.to_color()),
                ))
            })
            .collect::<Vec<_>>();
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
    } else {
        let explanation_hint = Paragraph::new(Line::from(vec![
            Span::styled(" e", Style::default().fg(theme.info.to_color())),
            Span::styled(
                " explain these changes",
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .border_style(Style::default().fg(theme.text_muted.to_color())),
        );
        frame.render_widget(explanation_hint, chunks[3]);
    }

    let hints = Paragraph::new(diff_review_hint_lines(
        explanation_loading,
        new_file_presentation,
        state.layout.clone(),
        diff_review_language_status(state),
        theme,
    ))
    .wrap(Wrap { trim: false });
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
            .constraints([Constraint::Length(2), Constraint::Length(1)])
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

fn diff_review_hint_lines(
    explanation_loading: bool,
    new_file_presentation: bool,
    layout: DiffViewerLayout,
    syntax_status: Option<(
        highlight::HighlightLanguage,
        highlight::HighlightInstallState,
    )>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    if explanation_loading {
        return vec![Line::from(vec![
            Span::styled(" e", Style::default().fg(theme.info.to_color())),
            Span::raw(" generating explanation..."),
        ])];
    }

    let mut primary = vec![
        Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" approve  "),
    ];
    if let Some((language, status)) = syntax_status {
        primary.push(Span::styled(
            "i",
            Style::default().fg(theme.warning.to_color()),
        ));
        let label = match status {
            highlight::HighlightInstallState::Installed => {
                format!(" syntax:{} installed  ", language.display_name())
            }
            highlight::HighlightInstallState::Available => {
                format!(" install {} parser  ", language.display_name())
            }
            highlight::HighlightInstallState::Broken => {
                format!(" repair {} parser  ", language.display_name())
            }
        };
        let color = match status {
            highlight::HighlightInstallState::Installed => theme.info.to_color(),
            highlight::HighlightInstallState::Available => theme.warning.to_color(),
            highlight::HighlightInstallState::Broken => theme.danger.to_color(),
        };
        primary.push(Span::styled(label, Style::default().fg(color)));
    }
    primary.extend(vec![
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" cancel"),
    ]);

    let mut secondary = vec![
        Span::styled("j/k", Style::default().fg(theme.primary.to_color())),
        Span::raw(" scroll  "),
        Span::styled("e", Style::default().fg(theme.info.to_color())),
        Span::raw(" explain  "),
    ];
    if new_file_presentation {
        secondary.push(Span::styled(
            "new file uses unified view  ",
            Style::default().fg(theme.text_muted.to_color()),
        ));
    } else {
        secondary.push(Span::styled(
            "v",
            Style::default().fg(theme.primary.to_color()),
        ));
        secondary.push(Span::raw(if layout == DiffViewerLayout::SideBySide {
            " stacked  "
        } else {
            " side-by-side  "
        }));
    }
    secondary.extend(vec![
        Span::styled("r", Style::default().fg(theme.danger.to_color())),
        Span::raw(" feedback"),
    ]);

    vec![Line::from(primary), Line::from(secondary)]
}

pub fn draw_running_hook_dialog(
    frame: &mut Frame,
    state: &RunningHookState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = centered_rect(90, 70, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

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
    crate::ui::draw_modal_overlay(frame, area, theme);

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

pub fn draw_latest_prompt_dialog(frame: &mut Frame, prompt: Option<&str>, theme: &Theme) {
    let area = centered_rect(80, 70, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

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

    let prompt_text = prompt.unwrap_or("(No prompt saved yet)");
    let paragraph = Paragraph::new(prompt_text)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(theme.text.to_color()));
    frame.render_widget(paragraph, chunks[0]);

    let mut hint_spans = Vec::new();
    if prompt.is_some() {
        hint_spans.extend(vec![
            Span::styled("Tab", Style::default().fg(theme.warning.to_color())),
            Span::styled("/Enter", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " inject  ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]);
    }
    hint_spans.extend(vec![
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled("/q", Style::default().fg(theme.warning.to_color())),
        Span::styled(" close", Style::default().fg(theme.text_muted.to_color())),
    ]);
    let hint = Paragraph::new(Line::from(hint_spans));
    frame.render_widget(hint, chunks[1]);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line<'static>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect::<String>()
    }

    #[test]
    fn diff_review_hints_include_syntax_install_for_new_files() {
        let theme = Theme::default();
        let lines = diff_review_hint_lines(
            false,
            true,
            DiffViewerLayout::Unified,
            Some((
                highlight::HighlightLanguage::Tsx,
                highlight::HighlightInstallState::Available,
            )),
            &theme,
        );

        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).contains("install tsx parser"));
        assert!(line_text(&lines[0]).contains("Esc cancel"));
        assert!(line_text(&lines[1]).contains("new file uses unified view"));
    }
}
