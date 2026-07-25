use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{PlanInterviewPhase, PlanInterviewState};
use crate::plan_interview::{PlanQuestionKind, QuestionSource};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;
use super::editor_view::{count_wrapped_editor_lines, editor_lines, sync_editor_scroll};

pub fn draw_plan_interview_dialog(
    frame: &mut Frame,
    state: &mut PlanInterviewState,
    message: Option<&str>,
    theme: &Theme,
    throbber_state: &throbber_widgets_tui::ThrobberState,
) {
    let review_gate = matches!(
        state.phase,
        PlanInterviewPhase::Review | PlanInterviewPhase::Editing
    );
    let area = if review_gate {
        centered_rect(86, 86, frame.area())
    } else {
        centered_rect(80, 72, frame.area())
    };
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = match state.phase {
        PlanInterviewPhase::Review => format!(" Plan Review · {} ", state.feature_name),
        PlanInterviewPhase::Editing => format!(" Edit Plan · {} ", state.feature_name),
        _ => format!(" Plan Mode · {} ", state.feature_name),
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(if review_gate {
            theme.effective_header_bg()
        } else {
            theme.effective_bg()
        }))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.abort_confirmation {
        let confirm = Paragraph::new(vec![
            Line::from(Span::styled(
                "Leave this interview?",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                hint("y", theme),
                Span::raw(" launch without a plan  "),
                hint("n", theme),
                Span::raw(" cancel feature creation  "),
                hint("Esc", theme),
                Span::raw(" resume interview"),
            ]),
        ])
        .block(
            Block::default()
                .title(" Confirm abort ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.warning.to_color())),
        )
        .style(Style::default().bg(theme.effective_bg()))
        .wrap(Wrap { trim: false });
        frame.render_widget(confirm, inner);
        return;
    }

    if state.phase == PlanInterviewPhase::Review {
        draw_plan_review(frame, inner, state, message, theme);
        return;
    }
    if state.phase == PlanInterviewPhase::Editing {
        draw_plan_edit(frame, inner, state, message, theme);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(inner);

    frame.render_widget(progress_header(state, theme), chunks[0]);
    frame.render_widget(question_prompt(state, theme), chunks[1]);

    match state.phase {
        PlanInterviewPhase::Brief => draw_editor(
            frame,
            chunks[2],
            state,
            "Describe the goal, intended users, and the outcome you want.",
            theme,
        ),
        PlanInterviewPhase::StaticQuestions => match state.current_question().map(|q| &q.kind) {
            Some(PlanQuestionKind::FreeText) => draw_editor(
                frame,
                chunks[2],
                state,
                "Type an answer, or skip if this question is optional.",
                theme,
            ),
            Some(PlanQuestionKind::Select(options)) => {
                draw_options(frame, chunks[2], options, state.selected_option, theme)
            }
            None => {}
        },
        PlanInterviewPhase::AiConsent => {
            frame.render_widget(
                Paragraph::new(vec![
                    Line::from(Span::styled(
                        "Adaptive follow-up questions are optional.",
                        Style::default()
                            .fg(theme.text.to_color())
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(
                        "AMF will send your brief, answers, and bounded repository context to an available agent harness.",
                    ),
                    Line::from(""),
                    Line::from(Span::styled(
                        format!(
                            "No agent tokens are used unless you opt in. Opting in may run up to {} AI rounds.",
                            crate::plan_interview::MAX_AI_ROUNDS
                        ),
                        Style::default().fg(theme.warning.to_color()),
                    )),
                ])
                .wrap(Wrap { trim: false }),
                chunks[2],
            );
        }
        PlanInterviewPhase::AiLoading => {
            draw_ai_loading(frame, chunks[2], state, theme, throbber_state)
        }
        PlanInterviewPhase::SynthesisLoading => {
            draw_synthesis_loading(frame, chunks[2], state, theme, throbber_state)
        }
        PlanInterviewPhase::Review | PlanInterviewPhase::Editing => unreachable!(),
        PlanInterviewPhase::Done => {
            frame.render_widget(
                Paragraph::new(
                    "Questions complete. The collected answers are ready for the plan handoff.",
                )
                .style(Style::default().fg(theme.success.to_color()))
                .wrap(Wrap { trim: false }),
                chunks[2],
            );
        }
    }

    let footer = if let Some(message) = message {
        let color = if message.starts_with("Error:") {
            theme.danger.to_color()
        } else {
            theme.text_muted.to_color()
        };
        Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(color),
        ))
    } else if state.phase == PlanInterviewPhase::AiConsent {
        Line::from(vec![
            hint("a", theme),
            Span::raw(" generate (uses tokens)  "),
            hint("Enter", theme),
            Span::raw(" finish without AI  "),
            hint("Ctrl+F", theme),
            Span::raw(" synthesize now  "),
            hint("Ctrl+B", theme),
            Span::raw(" back  "),
            hint("Esc", theme),
            Span::raw(" cancel"),
        ])
    } else if matches!(
        state.phase,
        PlanInterviewPhase::AiLoading | PlanInterviewPhase::SynthesisLoading
    ) {
        Line::from(vec![hint("Esc", theme), Span::raw(" cancel")])
    } else {
        Line::from(vec![
            hint("Enter", theme),
            Span::raw(" next  "),
            hint("Alt+Enter", theme),
            Span::raw(" newline  "),
            hint("Ctrl+B", theme),
            Span::raw(" back  "),
            hint("Ctrl+S", theme),
            Span::raw(" skip  "),
            hint("Ctrl+F", theme),
            Span::raw(" synthesize now (uses tokens)  "),
            hint("Esc", theme),
            Span::raw(" cancel"),
        ])
    };
    frame.render_widget(Paragraph::new(footer).wrap(Wrap { trim: false }), chunks[3]);
}

fn progress_header(state: &PlanInterviewState, theme: &Theme) -> Paragraph<'static> {
    let total = state.questions.len() + 1;
    let (position, stage) = match state.phase {
        PlanInterviewPhase::Brief => (1, "Feature brief".to_string()),
        PlanInterviewPhase::StaticQuestions => {
            let source = state
                .current_question()
                .map(|question| match question.source {
                    QuestionSource::Builtin => "Built-in".to_string(),
                    QuestionSource::GlobalTemplate => "Global template".to_string(),
                    QuestionSource::Template => "Project template".to_string(),
                    QuestionSource::Ai { round } => format!("AI round {round}"),
                })
                .unwrap_or_default();
            (state.question_index + 2, source)
        }
        PlanInterviewPhase::AiConsent => (total, "Optional AI".to_string()),
        PlanInterviewPhase::AiLoading => (
            state.questions.len() + 1,
            format!("AI round {}", state.ai_rounds_completed + 1),
        ),
        PlanInterviewPhase::SynthesisLoading => (total, "Plan synthesis".to_string()),
        PlanInterviewPhase::Review => (total, "Plan review".to_string()),
        PlanInterviewPhase::Editing => (total, "Edit plan".to_string()),
        PlanInterviewPhase::Done => (total, "Complete".to_string()),
    };
    Paragraph::new(Line::from(vec![
        Span::styled(
            format!(" Step {position}/{total} "),
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(stage, Style::default().fg(theme.text_muted.to_color())),
    ]))
}

fn question_prompt(state: &PlanInterviewState, theme: &Theme) -> Paragraph<'static> {
    let (text, optional) = match state.phase {
        PlanInterviewPhase::Brief => ("Describe the feature".to_string(), false),
        PlanInterviewPhase::StaticQuestions => state
            .current_question()
            .map(|question| (question.text.clone(), question.optional))
            .unwrap_or_default(),
        PlanInterviewPhase::AiConsent => {
            ("Generate adaptive follow-up questions?".to_string(), false)
        }
        PlanInterviewPhase::AiLoading => ("Generating follow-up questions".to_string(), false),
        PlanInterviewPhase::SynthesisLoading => {
            ("Synthesizing implementation plan".to_string(), false)
        }
        PlanInterviewPhase::Review => ("Review implementation plan".to_string(), false),
        PlanInterviewPhase::Editing => ("Edit raw markdown".to_string(), false),
        PlanInterviewPhase::Done => ("Interview complete".to_string(), false),
    };
    let suffix = if optional { " (optional)" } else { "" };
    Paragraph::new(Line::from(vec![
        Span::styled(
            text,
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(suffix, Style::default().fg(theme.text_muted.to_color())),
    ]))
    .wrap(Wrap { trim: false })
}

/// Loading frame shown while an AI-adaptive round runs off the UI thread
/// (`App::poll_plan_interview_ai_bg`). Shows the engine, elapsed time, and a
/// cheap token estimate for the prompt — mirrors the PR-review family's
/// running frames (`draw_ai_pr_review_running`).
fn draw_ai_loading(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &PlanInterviewState,
    theme: &Theme,
    throbber_state: &throbber_widgets_tui::ThrobberState,
) {
    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);

    let engine = state
        .ai_harness
        .as_ref()
        .and_then(|resolved| resolved.as_ref())
        .map(|harness| harness.display_name())
        .unwrap_or("agent");
    let elapsed = state
        .ai_round_started_at
        .map(|started_at| started_at.elapsed().as_secs())
        .unwrap_or(0);
    let round = state.ai_rounds_completed + 1;

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(vec![
                spinner,
                Span::styled(
                    format!(
                        " Generating follow-up questions ({engine}) · round {round} · {elapsed}s · ~{} tokens...",
                        state.ai_round_token_estimate
                    ),
                    Style::default()
                        .fg(theme.text.to_color())
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ])
        .wrap(Wrap { trim: false }),
        area,
    );
}

/// Loading frame for the final structured-plan synthesis pass.
fn draw_synthesis_loading(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &PlanInterviewState,
    theme: &Theme,
    throbber_state: &throbber_widgets_tui::ThrobberState,
) {
    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);

    let engine = state
        .ai_harness
        .as_ref()
        .and_then(|resolved| resolved.as_ref())
        .map(|harness| harness.display_name())
        .unwrap_or("agent");
    let elapsed = state
        .synthesis_started_at
        .map(|started_at| started_at.elapsed().as_secs())
        .unwrap_or(0);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(vec![
                spinner,
                Span::styled(
                    format!(
                        " Synthesizing implementation plan ({engine}) · {elapsed}s · ~{} tokens...",
                        state.synthesis_token_estimate
                    ),
                    Style::default()
                        .fg(theme.text.to_color())
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
        ])
        .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_plan_review(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &mut PlanInterviewState,
    message: Option<&str>,
    theme: &Theme,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(area);
    let source_path = state
        .pending_launch
        .as_ref()
        .map(|prepared| prepared.workdir.join(".claude/plan.md"))
        .unwrap_or_else(|| std::path::PathBuf::from(".claude/plan.md"));
    let content = state.synthesized_plan.as_deref().unwrap_or_default();
    super::markdown::draw_markdown_document(
        frame,
        chunks[0],
        content,
        &source_path,
        &mut state.review_scroll_offset,
        &mut state.review_rendered_width,
        &mut state.review_rendered_lines,
        theme,
    );

    let context = if let Some(message) = message {
        let color = if message.starts_with("Error:") {
            theme.danger.to_color()
        } else {
            theme.text_muted.to_color()
        };
        Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(color),
        ))
    } else {
        Line::from(Span::styled(
            source_path.display().to_string(),
            Style::default()
                .fg(theme.secondary.to_color())
                .add_modifier(Modifier::ITALIC),
        ))
    };
    let hints = Line::from(vec![
        hint("j/k", theme),
        Span::raw(" scroll  "),
        hint("PgUp/PgDn", theme),
        Span::raw(" page  "),
        hint("e", theme),
        Span::raw(" edit  "),
        hint("r", theme),
        Span::raw(" regenerate  "),
        hint("Enter", theme),
        Span::raw(" accept  "),
        hint("Esc", theme),
        Span::raw(" abort"),
    ]);
    frame.render_widget(
        Paragraph::new(vec![context, hints])
            .style(Style::default().bg(theme.effective_header_bg()))
            .wrap(Wrap { trim: false }),
        chunks[1],
    );
}

fn draw_plan_edit(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &mut PlanInterviewState,
    message: Option<&str>,
    theme: &Theme,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(2)])
        .split(area);
    let lines = editor_lines(
        &state.editor,
        theme,
        "Write the implementation plan in markdown.",
    );
    let wrap_width = chunks[0].width.saturating_sub(2).max(1) as usize;
    let total_visual_lines = count_wrapped_editor_lines(&lines, wrap_width);
    sync_editor_scroll(
        &state.editor,
        &mut state.edit_scroll_offset,
        &mut state.edit_sync_to_cursor,
        chunks[0].height.saturating_sub(2) as usize,
        wrap_width,
        total_visual_lines,
    );
    let editor = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Raw markdown ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border.to_color())),
        )
        .style(Style::default().bg(theme.effective_header_bg()))
        .wrap(Wrap { trim: false })
        .scroll((state.edit_scroll_offset.min(u16::MAX as usize) as u16, 0));
    frame.render_widget(editor, chunks[0]);

    let footer = if let Some(message) = message {
        let color = if message.starts_with("Error:") {
            theme.danger.to_color()
        } else {
            theme.text_muted.to_color()
        };
        Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(color),
        ))
    } else {
        Line::from(vec![
            hint("Enter", theme),
            Span::raw(" newline  "),
            hint("Ctrl+S", theme),
            Span::raw(" save + preview  "),
            hint("Esc", theme),
            Span::raw(" discard edits"),
        ])
    };
    frame.render_widget(
        Paragraph::new(footer)
            .style(Style::default().bg(theme.effective_header_bg()))
            .wrap(Wrap { trim: false }),
        chunks[1],
    );
}

fn draw_editor(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &PlanInterviewState,
    placeholder: &str,
    theme: &Theme,
) {
    let input = Paragraph::new(editor_lines(&state.editor, theme, placeholder))
        .block(
            Block::default()
                .title(" Answer ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme.border.to_color())),
        )
        .wrap(Wrap { trim: false });
    frame.render_widget(input, area);
}

fn draw_options(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    options: &[String],
    selected: usize,
    theme: &Theme,
) {
    let items = options
        .iter()
        .map(|option| ListItem::new(option.clone()))
        .collect::<Vec<_>>();
    let list = List::new(items)
        .block(Block::default().title(" Options ").borders(Borders::ALL))
        .highlight_symbol("› ")
        .highlight_style(
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        );
    let mut list_state =
        ListState::default().with_selected((!options.is_empty()).then_some(selected));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn hint(key: &'static str, theme: &Theme) -> Span<'static> {
    Span::styled(key, Style::default().fg(theme.warning.to_color()))
}
