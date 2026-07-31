use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{PlanInterviewPhase, PlanInterviewState, PriorAnswerState};
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
    // The whole review gate shares one frame size so moving between the plan,
    // its editor, and an agent review does not resize the dialog underfoot.
    let review_gate = matches!(
        state.phase,
        PlanInterviewPhase::Review
            | PlanInterviewPhase::Editing
            | PlanInterviewPhase::Critique
            | PlanInterviewPhase::CritiqueLoading
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
        PlanInterviewPhase::Critique | PlanInterviewPhase::CritiqueLoading => {
            format!(" Agent Review · {} ", state.feature_name)
        }
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
        // An on-demand interview has no launch riding on it, so the only
        // choices are to leave the feature as it is or keep answering.
        let choices = if state.pending_launch.is_some() {
            vec![
                hint("y", theme),
                Span::raw(" launch without a plan  "),
                hint("n", theme),
                Span::raw(" cancel feature creation  "),
                hint("Esc", theme),
                Span::raw(" resume interview"),
            ]
        } else {
            vec![
                hint("y", theme),
                Span::raw(" leave the plan unchanged  "),
                hint("Esc", theme),
                Span::raw(" resume interview"),
            ]
        };
        let confirm = Paragraph::new(vec![
            Line::from(Span::styled(
                "Leave this interview?",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(choices),
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
    if state.phase == PlanInterviewPhase::Critique {
        draw_plan_critique(frame, inner, state, message, theme);
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
        PlanInterviewPhase::ResumePrompt => draw_resume_prompt(frame, chunks[2], state, theme),
        PlanInterviewPhase::KickoffHandoff => {
            draw_kickoff_handoff(frame, chunks[2], state, theme)
        }
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
        PlanInterviewPhase::CritiqueLoading => {
            draw_critique_loading(frame, chunks[2], state, theme, throbber_state)
        }
        PlanInterviewPhase::Review | PlanInterviewPhase::Editing | PlanInterviewPhase::Critique => {
            unreachable!()
        }
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
    } else if state.phase == PlanInterviewPhase::ResumePrompt {
        Line::from(vec![
            hint("r", theme),
            Span::raw(" resume saved answers  "),
            hint("d", theme),
            Span::raw(" discard and start over  "),
            hint("Esc", theme),
            Span::raw(" cancel (keeps the draft)"),
        ])
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
    } else if state.phase == PlanInterviewPhase::KickoffHandoff {
        Line::from(vec![
            hint("y", theme),
            Span::raw(" open the session with a kickoff prompt  "),
            hint("n", theme),
            Span::raw(" leave it running"),
        ])
    } else if state.phase == PlanInterviewPhase::CritiqueLoading {
        Line::from(vec![hint("Esc", theme), Span::raw(" back to plan")])
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
        PlanInterviewPhase::ResumePrompt => (1, "Saved draft".to_string()),
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
        PlanInterviewPhase::CritiqueLoading | PlanInterviewPhase::Critique => {
            (total, "Agent review".to_string())
        }
        PlanInterviewPhase::Review => (total, "Plan review".to_string()),
        PlanInterviewPhase::Editing => (total, "Edit plan".to_string()),
        PlanInterviewPhase::KickoffHandoff => (total, "Plan accepted".to_string()),
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

/// The re-run note under the question: which of the three states the step's
/// answer is in relative to the previously accepted interview, and how to put
/// it back. Absent unless a prior transcript was pre-filled, and deliberately
/// here rather than in the footer — the footer's hint row is already full at
/// ordinary dialog widths.
fn prior_answer_note(state: &PlanInterviewState, theme: &Theme) -> Option<Line<'static>> {
    let is_brief = state.phase == PlanInterviewPhase::Brief;
    let (text, color) = match state.prior_answer_state()? {
        PriorAnswerState::Kept if is_brief => (
            "Previous brief pre-filled — Enter keeps it",
            theme.secondary.to_color(),
        ),
        PriorAnswerState::Kept => (
            "Previous answer pre-filled — Enter keeps it",
            theme.secondary.to_color(),
        ),
        PriorAnswerState::Changed => (
            "Changed from the previous interview — Ctrl+R restores it",
            theme.warning.to_color(),
        ),
        PriorAnswerState::Cleared => (
            "Previous answer cleared — Ctrl+R restores it",
            theme.warning.to_color(),
        ),
    };
    Some(Line::from(Span::styled(
        text,
        Style::default().fg(color).add_modifier(Modifier::ITALIC),
    )))
}

fn question_prompt(state: &PlanInterviewState, theme: &Theme) -> Paragraph<'static> {
    let (text, optional) = match state.phase {
        PlanInterviewPhase::ResumePrompt => ("Resume the saved interview?".to_string(), false),
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
        PlanInterviewPhase::CritiqueLoading => ("Reviewing the draft plan".to_string(), false),
        PlanInterviewPhase::Critique => ("Agent review of the plan".to_string(), false),
        PlanInterviewPhase::Review => ("Review implementation plan".to_string(), false),
        PlanInterviewPhase::Editing => ("Edit raw markdown".to_string(), false),
        PlanInterviewPhase::KickoffHandoff => {
            ("Tell the running session about the plan?".to_string(), false)
        }
        PlanInterviewPhase::Done => ("Interview complete".to_string(), false),
    };
    let suffix = if optional { " (optional)" } else { "" };
    let mut lines = vec![Line::from(vec![
        Span::styled(
            text,
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(suffix, Style::default().fg(theme.text_muted.to_color())),
    ])];
    lines.extend(prior_answer_note(state, theme));
    Paragraph::new(lines).wrap(Wrap { trim: false })
}

/// The resume-or-discard choice for a saved draft.
///
/// Summarizes what resuming would actually restore — how much was answered,
/// whether adaptive rounds were already spent, whether a plan was already
/// generated — so the choice is not made blind. `updated_at` is the DB's own
/// timestamp, which is what makes "is this draft still relevant?" answerable.
fn draw_resume_prompt(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &PlanInterviewState,
    theme: &Theme,
) {
    let Some(draft) = state.resume_draft.as_ref() else {
        return;
    };

    let answered = draft
        .answers
        .iter()
        .filter(|answer| !answer.as_deref().unwrap_or_default().trim().is_empty())
        .count();
    let brief_preview: String = draft
        .brief
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_default()
        .chars()
        .take(160)
        .collect();

    let mut lines = vec![
        Line::from(Span::styled(
            format!(
                "An unfinished interview for '{}' was saved.",
                draft.feature_name
            ),
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("Last saved: {}", draft.updated_at),
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            format!("{answered} of {} questions answered", draft.questions.len()),
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ];
    if draft.ai_rounds_completed > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "{} AI round(s) already spent — resuming does not pay for them again",
                draft.ai_rounds_completed
            ),
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }
    if draft.plan.is_some() {
        lines.push(Line::from(Span::styled(
            "A generated plan was saved — resuming reopens it at the review gate",
            Style::default().fg(theme.success.to_color()),
        )));
    }
    if state.has_prior_answers() {
        // Discarding a stale draft on a re-run is not "start from nothing": the
        // answers behind the plan already accepted for this feature remain the
        // baseline, so say so before the choice is made.
        lines.push(Line::from(Span::styled(
            "Discarding starts from the answers of this feature's accepted plan",
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }
    if !brief_preview.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            brief_preview,
            Style::default()
                .fg(theme.secondary.to_color())
                .add_modifier(Modifier::ITALIC),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// The handoff offer shown after an on-demand plan is accepted for a feature
/// whose agent session is already running.
///
/// States plainly that the plan is already written, because that is what makes
/// declining a real option rather than a mistake: the only thing on offer is
/// interrupting a session that may be mid-task.
fn draw_kickoff_handoff(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &PlanInterviewState,
    theme: &Theme,
) {
    let Some(target) = state.kickoff_handoff.as_ref() else {
        return;
    };

    let lines = vec![
        Line::from(Span::styled(
            format!("Plan written to {}", target.plan_path.display()),
            Style::default().fg(theme.success.to_color()),
        )),
        Line::from(""),
        Line::from(Span::styled(
            format!("'{}' is still running.", target.session_label),
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            "It will not notice the new plan on its own — an agent reads its \
             instruction file once, at startup.",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Opening it seeds the composer with a kickoff prompt pointing at the \
             plan. Nothing is sent until you press Enter there.",
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ];

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// Loading frame shown while an AI-adaptive round runs off the UI thread
/// (`App::poll_plan_interview_ai_bg`).
fn draw_ai_loading(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &PlanInterviewState,
    theme: &Theme,
    throbber_state: &throbber_widgets_tui::ThrobberState,
) {
    let round = state.ai_rounds_completed + 1;
    draw_headless_loading(
        frame,
        area,
        theme,
        throbber_state,
        format!(
            "Generating follow-up questions ({}) · round {round}",
            interview_engine(state)
        ),
        state.ai_round_started_at,
        state.ai_round_token_estimate,
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
    draw_headless_loading(
        frame,
        area,
        theme,
        throbber_state,
        format!(
            "Synthesizing implementation plan ({})",
            interview_engine(state)
        ),
        state.synthesis_started_at,
        state.synthesis_token_estimate,
    );
}

/// Loading frame for the optional advisory review of the draft plan.
fn draw_critique_loading(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    state: &PlanInterviewState,
    theme: &Theme,
    throbber_state: &throbber_widgets_tui::ThrobberState,
) {
    draw_headless_loading(
        frame,
        area,
        theme,
        throbber_state,
        format!("Reviewing the draft plan ({})", interview_engine(state)),
        state.critique_started_at,
        state.critique_token_estimate,
    );
}

/// Shared spinner frame for the interview's headless passes, showing elapsed
/// time and a cheap prompt-size token estimate — mirrors the PR-review
/// family's running frames (`draw_ai_pr_review_running`).
fn draw_headless_loading(
    frame: &mut Frame,
    area: ratatui::layout::Rect,
    theme: &Theme,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    head: String,
    started_at: Option<std::time::Instant>,
    token_estimate: usize,
) {
    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);
    let elapsed = started_at
        .map(|started_at| started_at.elapsed().as_secs())
        .unwrap_or(0);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(""),
            Line::from(vec![
                spinner,
                Span::styled(
                    format!(" {head} · {elapsed}s · ~{token_estimate} tokens..."),
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

/// Name of the harness powering this interview's headless calls, or a neutral
/// placeholder before one has been resolved.
fn interview_engine(state: &PlanInterviewState) -> &str {
    state
        .ai_harness
        .as_ref()
        .and_then(|resolved| resolved.as_ref())
        .map(|harness| harness.display_name())
        .unwrap_or("agent")
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
    let source_path = state.workdir.join(".claude/plan.md");
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
        hint("e", theme),
        Span::raw(" edit  "),
        hint("a", theme),
        // A review already held for this plan is re-opened, not re-run, so the
        // hint says which of the two `a` does before it costs anything.
        Span::raw(if state.critique.is_some() {
            " show review  "
        } else {
            " agent review  "
        }),
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

/// The agent's advisory review of the draft plan. Rendered through the same
/// markdown viewer as the plan, and pointedly read-only: the plan itself is
/// only touched if the user asks for a revision from here.
fn draw_plan_critique(
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
    let content = state.critique.as_deref().unwrap_or_default();
    super::markdown::draw_markdown_document(
        frame,
        chunks[0],
        content,
        std::path::Path::new("agent review"),
        &mut state.critique_scroll_offset,
        &mut state.critique_rendered_width,
        &mut state.critique_rendered_lines,
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
            format!(
                "Advisory only ({}) — the plan is unchanged.",
                interview_engine(state)
            ),
            Style::default()
                .fg(theme.secondary.to_color())
                .add_modifier(Modifier::ITALIC),
        ))
    };
    let hints = Line::from(vec![
        hint("j/k", theme),
        Span::raw(" scroll  "),
        hint("r", theme),
        Span::raw(" revise plan with this feedback  "),
        hint("Esc", theme),
        Span::raw(" back to plan"),
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
