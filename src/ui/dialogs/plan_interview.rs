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
use super::editor_view::editor_lines;

pub fn draw_plan_interview_dialog(
    frame: &mut Frame,
    state: &PlanInterviewState,
    message: Option<&str>,
    theme: &Theme,
    throbber_state: &throbber_widgets_tui::ThrobberState,
) {
    let area = centered_rect(80, 72, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(format!(" Plan Mode · {} ", state.feature_name))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
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
        PlanInterviewPhase::AiLoading => {
            draw_ai_loading(frame, chunks[2], state, theme, throbber_state)
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
            Span::raw(" finish  "),
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
        PlanInterviewPhase::AiLoading => (
            state.questions.len() + 1,
            format!("AI round {}", state.ai_rounds_completed + 1),
        ),
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
        PlanInterviewPhase::AiLoading => ("Generating follow-up questions".to_string(), false),
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
