//! Rendering for the AI Review pane — AMF's own review of a PR's diff,
//! independent of PR Triage. See `crate::app::ai_review`'s module doc for why
//! this is a separate workflow/pane rather than bolted onto triage.

use chrono::{DateTime, Local};
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
};

use crate::{
    app::ai_review::{AiReviewFinding, AiReviewRun, AiReviewRunOutcome, AiReviewStage},
    app::{
        AiHarnessPickState, AiModelPickState, AiReviewPostConfirmState, AiReviewRunState,
        AiReviewState, ModelPickRow,
    },
    theme::Theme,
};

use super::pr_review::{chip, diff_hunk_lines, divider, pane_block, section_label, truncate_left};

/// Full-screen running view for the background diff-fetch + review pass.
pub fn draw_ai_review_running(
    frame: &mut Frame,
    state: &AiReviewRunState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = frame.area();
    let block = pane_block(theme).title(format!(
        " AI review of PR #{} with {} (experimental) ",
        state.origin.pr.number,
        state
            .origin
            .harness
            .as_ref()
            .map(|agent| agent.display_name())
            .unwrap_or("agent")
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);

    let status_line = match state.progress.stage {
        AiReviewStage::PreparingDiff => Line::from(vec![
            spinner,
            Span::styled(
                " Fetching PR diff...",
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        AiReviewStage::Reviewing { token_estimate } => Line::from(vec![
            spinner,
            Span::styled(
                format!(" Reviewing diff (~{token_estimate} tokens)..."),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    };

    let elapsed = format_elapsed(state.progress.started_at.elapsed());
    let mut lines = vec![Line::from(""), status_line, Line::from("")];
    if let Some(activity) = &state.progress.activity {
        lines.push(Line::from(vec![
            Span::styled(
                "Current activity: ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled(activity, Style::default().fg(theme.text.to_color())),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            "Waiting for the harness's first progress event...",
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }
    let mut elapsed_line = vec![
        Span::styled(
            "Elapsed: ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(elapsed, Style::default().fg(theme.text.to_color())),
    ];
    if let Some((input, output)) = state.progress.usage {
        elapsed_line.push(Span::styled(
            format!("  ·  {input} input / {output} output tokens"),
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }
    lines.push(Line::from(elapsed_line));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "esc to return to the AI Review pane (the run keeps going in the background)",
        Style::default().fg(theme.text_muted.to_color()),
    )));

    let body = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(body, inner);
}

fn format_elapsed(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes == 0 {
        format!("{seconds}s")
    } else {
        format!("{minutes}m {seconds:02}s")
    }
}

pub(super) fn ai_review_run_badge_text(
    run: &AiReviewRun,
    include_error_detail: bool,
) -> (String, bool) {
    let age = format_relative_time(run.ran_at);
    match &run.outcome {
        AiReviewRunOutcome::Findings(0) => (format!("no findings ({age})"), false),
        AiReviewRunOutcome::Findings(n) => (
            format!("{n} finding{} ({age})", if *n == 1 { "" } else { "s" }),
            false,
        ),
        AiReviewRunOutcome::Error(e) if include_error_detail => {
            (format!("failed ({age}): {}", truncate_right(e, 60)), true)
        }
        AiReviewRunOutcome::Error(_) => (format!("failed ({age})"), true),
    }
}

/// Coarse relative age (`"now"`, `"5m"`, `"3h"`, `"2d"`, or a bare date past a
/// week) for a header badge.
fn format_relative_time(at: DateTime<Local>) -> String {
    let delta = Local::now().signed_duration_since(at);
    if delta.num_minutes() < 1 {
        "now".to_string()
    } else if delta.num_hours() < 1 {
        format!("{}m", delta.num_minutes())
    } else if delta.num_days() < 1 {
        format!("{}h", delta.num_hours())
    } else if delta.num_days() < 7 {
        format!("{}d", delta.num_days())
    } else {
        at.format("%b %-d").to_string()
    }
}

fn truncate_right(s: &str, max: usize) -> String {
    let len = s.chars().count();
    if len <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let head: String = s.chars().take(max - 1).collect();
    format!("{head}…")
}

fn finding_location(f: &AiReviewFinding) -> String {
    match (&f.path, f.side, f.line) {
        (Some(path), Some(crate::diff::DiffSide::Old), Some(line)) => {
            format!("{path}:{line} (base)")
        }
        (Some(path), Some(crate::diff::DiffSide::New), Some(line)) => {
            format!("{path}:{line}")
        }
        (Some(path), _, _) => path.clone(),
        (None, _, _) => "General".to_string(),
    }
}

fn finding_list_line(
    index: usize,
    f: &AiReviewFinding,
    has_combined_cost: bool,
    theme: &Theme,
    width: usize,
) -> Line<'static> {
    let marker = if f.published {
        "✓"
    } else if f.skipped {
        "-"
    } else {
        " "
    };
    let marker_color = if f.published {
        theme.success.to_color()
    } else if f.skipped {
        theme.text_muted.to_color()
    } else {
        theme.warning.to_color()
    };
    let marker_span = format!("[{marker}] ");
    let index_span = format!("{}. ", index + 1);
    // `⧉` flags a finding that was fixed in PR Triage as part of a combined
    // batch (its shared cost shows in the detail pane).
    let combined_span = if has_combined_cost { "⧉ " } else { "" };
    let prefix_width =
        marker_span.chars().count() + index_span.chars().count() + combined_span.chars().count();
    let location = truncate_left(&finding_location(f), width.saturating_sub(prefix_width));
    let snippet = f.body.lines().next().unwrap_or("").to_string();

    Line::from(vec![
        Span::styled(marker_span, Style::default().fg(marker_color)),
        Span::styled(index_span, Style::default().fg(theme.text_muted.to_color())),
        Span::styled(combined_span, Style::default().fg(theme.info.to_color())),
        Span::styled(
            location,
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {snippet}"),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ])
}

fn draw_finding_list(
    frame: &mut Frame,
    area: Rect,
    state: &AiReviewState,
    theme: &Theme,
    finding_fix_costs: &[Option<String>],
) {
    let block = pane_block(theme)
        .border_style(Style::default().fg(theme.primary.to_color()))
        .title(" Findings ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.findings.is_empty() {
        frame.render_widget(
            Paragraph::new("No findings yet — press A to generate a review.")
                .style(Style::default().fg(theme.text_muted.to_color()))
                .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }

    let width = inner.width.max(1) as usize;
    let lines: Vec<Line<'static>> = state
        .findings
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let has_combined = finding_fix_costs.get(i).is_some_and(Option::is_some);
            let line = finding_list_line(i, f, has_combined, theme, width);
            if i == state.selected {
                Line::from(
                    line.spans
                        .into_iter()
                        .map(|s| {
                            let style = s.style.bg(theme.effective_selection_bg());
                            s.style(style)
                        })
                        .collect::<Vec<_>>(),
                )
            } else {
                line
            }
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

/// Render the detail pane for the selected finding; returns the content line
/// count so the caller can clamp detail scrolling.
fn draw_finding_detail(
    frame: &mut Frame,
    area: Rect,
    finding: Option<&AiReviewFinding>,
    fix_cost_line: Option<&str>,
    scroll: usize,
    theme: &Theme,
) -> usize {
    let block = pane_block(theme)
        .border_style(Style::default().fg(theme.text_muted.to_color()))
        .title(" Detail ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(f) = finding else {
        return 0;
    };
    let width = inner.width.max(1) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    let mut header_spans = vec![Span::styled(
        finding_location(f),
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )];
    if f.published {
        header_spans.push(chip("posted", theme.success.to_color()));
    } else if f.skipped {
        header_spans.push(chip("skipped", theme.text_muted.to_color()));
    }
    lines.push(Line::from(header_spans));
    lines.push(Line::from(chip("ai", theme.info.to_color())));
    // Shown only when this finding was posted and then fixed in PR Triage as
    // part of a combined batch — the shared cost of that one agent run.
    if let Some(fix_cost_line) = fix_cost_line {
        lines.push(Line::from(Span::styled(
            fix_cost_line.to_string(),
            Style::default()
                .fg(theme.info.to_color())
                .add_modifier(Modifier::BOLD),
        )));
    }

    if let Some(hunk) = &f.diff_hunk {
        lines.push(divider(width, theme));
        lines.push(section_label("Diff hunk", theme));
        lines.extend(diff_hunk_lines(hunk, f.path.as_deref(), theme));
    } else if f.path.is_some() {
        lines.push(divider(width, theme));
        lines.push(Line::from(Span::styled(
            "no matching diff hunk — open the file for context",
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }

    lines.push(divider(width, theme));
    if f.body.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            "(no body)",
            Style::default().fg(theme.text_muted.to_color()),
        )));
    } else {
        lines.extend(crate::markdown::render_markdown(&f.body, theme, width, None).lines);
    }

    let count = lines.len();
    let body = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    frame.render_widget(body, inner);
    count
}

/// Main AI Review pane: findings list (left) + detail (right), plus any
/// overlaid picker/editor/post-confirm dialog.
pub fn draw_ai_review(
    frame: &mut Frame,
    state: &mut AiReviewState,
    theme: &Theme,
    ai_review_running: bool,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    finding_fix_costs: &[Option<String>],
) {
    let area = frame.area();
    // A sub-header line naming the harness/model that produced the current
    // findings and what the run cost. Absent until a run completes for this
    // head SHA (or for a legacy cache row with no attribution).
    let attribution_line: Option<Line<'static>> = state.attribution.as_ref().map(|attribution| {
        let mut label = format!("  {}", attribution.plain_label());
        if !attribution.has_usage() {
            // The run finished but the harness reported no token counts, so
            // there is no cost to show — say so rather than leave it looking
            // truncated.
            label.push_str(" · usage not reported");
        }
        Line::from(Span::styled(
            label,
            Style::default().fg(theme.text_muted.to_color()),
        ))
    });
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                                     // header
            Constraint::Length(u16::from(attribution_line.is_some())), // attribution
            Constraint::Min(1),                                        // body
            Constraint::Length(1),                                     // footer
        ])
        .split(area);

    let mut header_spans = vec![
        Span::styled(
            format!(" AI Review · PR #{} (experimental) ", state.pr.number),
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{} finding{}",
                state.findings.len(),
                if state.findings.len() == 1 { "" } else { "s" }
            ),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ];
    if ai_review_running {
        header_spans.push(Span::raw("  "));
        header_spans.push(
            throbber_widgets_tui::Throbber::default()
                .style(Style::default().fg(theme.warning.to_color()))
                .to_symbol_span(throbber_state),
        );
        header_spans.push(Span::styled(
            " running…",
            Style::default().fg(theme.warning.to_color()),
        ));
    } else if let Some(run) = &state.last_run {
        let (text, is_error) = ai_review_run_badge_text(run, true);
        header_spans.push(Span::styled(
            format!("  {text}"),
            Style::default().fg(if is_error {
                theme.danger.to_color()
            } else {
                theme.status_detail.to_color()
            }),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(header_spans)), outer[0]);

    if let Some(attribution_line) = attribution_line {
        frame.render_widget(Paragraph::new(attribution_line), outer[1]);
    }

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(outer[2]);
    draw_finding_list(frame, body[0], state, theme, finding_fix_costs);
    let detail_lines = draw_finding_detail(
        frame,
        body[1],
        state.findings.get(state.selected),
        finding_fix_costs
            .get(state.selected)
            .and_then(Option::as_deref),
        state.detail_scroll,
        theme,
    );
    state.detail_content_lines = detail_lines;

    let ai_action = if ai_review_running {
        "A view progress"
    } else {
        "A regenerate"
    };
    let keys = Paragraph::new(Line::from(Span::styled(
        format!(" j/k move   s skip/unskip   e edit   {ai_action}   W post   esc/q close"),
        Style::default().fg(theme.text_muted.to_color()),
    )));
    frame.render_widget(keys, outer[3]);

    if let Some(pick) = &state.harness_pick {
        draw_ai_harness_pick(frame, pick, theme);
    }
    if let Some(pick) = &state.model_pick {
        draw_ai_model_pick(frame, pick, theme);
    }
    if let Some(editor) = &state.finding_editor {
        draw_finding_editor(frame, editor, theme);
    }
    if let Some(post) = &state.post_confirm {
        let eligible = state
            .findings
            .iter()
            .filter(|f| !f.skipped && !f.published)
            .count();
        draw_ai_review_post_dialog(frame, post, eligible, theme);
    }
}

fn draw_finding_editor(frame: &mut Frame, editor: &crate::editor::TextEditor, theme: &Theme) {
    let area = super::super::dashboard::centered_rect(70, 55, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);
    let block = Block::default()
        .title(" Edit finding ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    let lines = super::editor_view::editor_lines(editor, theme, "(finding body)");
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), chunks[0]);
    frame.render_widget(
        Paragraph::new("[esc] done editing").style(Style::default().fg(theme.primary.to_color())),
        chunks[1],
    );
}

fn draw_ai_harness_pick(frame: &mut Frame, pick: &AiHarnessPickState, theme: &Theme) {
    let area = super::super::dashboard::centered_rect(54, 44, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Harness for AI review ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(if pick.error.is_some() { 2 } else { 0 }),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new("  Generate this PR's AI review with:")
            .style(Style::default().fg(theme.text_muted.to_color())),
        chunks[0],
    );
    let lines = pick.agents.iter().enumerate().map(|(index, agent)| {
        let selected = index == pick.selected;
        let style = if selected {
            Style::default()
                .fg(theme.text.to_color())
                .bg(theme.effective_selection_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text.to_color())
        };
        Line::from(vec![
            Span::styled(
                if selected { "  > " } else { "    " },
                Style::default().fg(theme.warning.to_color()),
            ),
            Span::styled(agent.display_name().to_string(), style),
        ])
    });
    frame.render_widget(Paragraph::new(lines.collect::<Vec<_>>()), chunks[1]);
    if let Some(error) = &pick.error {
        frame.render_widget(
            Paragraph::new(format!("  {error}"))
                .style(Style::default().fg(theme.danger.to_color()))
                .wrap(Wrap { trim: true }),
            chunks[2],
        );
    }
    frame.render_widget(
        Paragraph::new("  [j/k] choose   [⏎] run review   [esc] cancel")
            .style(Style::default().fg(theme.primary.to_color())),
        chunks[3],
    );
}

/// Label shown for one [`ModelPickRow`] in the model picker's list.
fn model_pick_row_label(row: &ModelPickRow) -> String {
    match row {
        ModelPickRow::Default => "Default (harness's own model)".to_string(),
        ModelPickRow::Preset(name) => name.to_string(),
        ModelPickRow::Custom => "Custom…".to_string(),
    }
}

fn draw_ai_model_pick(frame: &mut Frame, pick: &AiModelPickState, theme: &Theme) {
    let area = super::super::dashboard::centered_rect(54, 46, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Model for AI review ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let custom_selected = matches!(pick.rows.get(pick.selected), Some(ModelPickRow::Custom));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(if pick.editing_custom { 2 } else { 0 }),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new("  Generate this PR's AI review using:")
            .style(Style::default().fg(theme.text_muted.to_color())),
        chunks[0],
    );
    let lines = pick.rows.iter().enumerate().map(|(index, row)| {
        let selected = index == pick.selected;
        let style = if selected {
            Style::default()
                .fg(theme.text.to_color())
                .bg(theme.effective_selection_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text.to_color())
        };
        let mut label = model_pick_row_label(row);
        if matches!(row, ModelPickRow::Custom) && !pick.custom_input.is_empty() {
            label = format!("{label} ({})", pick.custom_input);
        }
        Line::from(vec![
            Span::styled(
                if selected { "  > " } else { "    " },
                Style::default().fg(theme.warning.to_color()),
            ),
            Span::styled(label, style),
        ])
    });
    frame.render_widget(Paragraph::new(lines.collect::<Vec<_>>()), chunks[1]);
    if pick.editing_custom {
        frame.render_widget(
            Paragraph::new(format!("  model: {}▏", pick.custom_input))
                .style(Style::default().fg(theme.text.to_color())),
            chunks[2],
        );
    }
    let hints = if pick.editing_custom {
        "  [⏎] use this model   [esc] back to list"
    } else if custom_selected {
        "  [j/k] choose   [⏎] type a model   [esc] harness"
    } else {
        "  [j/k] choose   [⏎] confirm   [esc] harness"
    };
    frame.render_widget(
        Paragraph::new(hints).style(Style::default().fg(theme.primary.to_color())),
        chunks[3],
    );
}

fn draw_ai_review_post_dialog(
    frame: &mut Frame,
    post: &AiReviewPostConfirmState,
    eligible_count: usize,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(70, 55, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Post AI review to GitHub ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut constraints = vec![Constraint::Length(2)];
    if post.error.is_some() {
        constraints.push(Constraint::Length(2));
    }
    constraints.push(Constraint::Min(1));
    constraints.push(Constraint::Length(1));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);
    let mut row = 0;

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!(
                    "{eligible_count} finding{} · {} inline comment{}, rest folded into the summary",
                    if eligible_count == 1 { "" } else { "s" },
                    post.inline.len(),
                    if post.inline.len() == 1 { "" } else { "s" },
                ),
                Style::default().fg(theme.text_muted.to_color()),
            )),
            Line::from(Span::styled(
                "Summary (edit freely):",
                Style::default().fg(theme.text.to_color()),
            )),
        ]),
        chunks[row],
    );
    row += 1;

    if let Some(error) = &post.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("Post failed: {error}"),
                Style::default().fg(theme.danger.to_color()),
            )))
            .wrap(Wrap { trim: false }),
            chunks[row],
        );
        row += 1;
    }

    let body_lines = super::editor_view::editor_lines(&post.editor, theme, "(summary body)");
    frame.render_widget(
        Paragraph::new(body_lines).wrap(Wrap { trim: false }),
        chunks[row],
    );
    row += 1;

    let hints = if post.editing {
        "[esc] done editing"
    } else {
        "[⏎] post to GitHub   [e] edit summary   [esc] cancel"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[row],
    );
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use ratatui::{Terminal, backend::TestBackend};

    use super::{draw_ai_review, draw_ai_review_running, finding_location, format_elapsed};
    use crate::{
        app::{AiReviewRunState, AiReviewState},
        project::AgentKind,
        theme::Theme,
    };

    #[test]
    fn elapsed_time_stays_compact_for_short_and_long_reviews() {
        assert_eq!(format_elapsed(std::time::Duration::from_secs(9)), "9s");
        assert_eq!(
            format_elapsed(std::time::Duration::from_secs(125)),
            "2m 05s"
        );
    }

    #[test]
    fn finding_location_renders_only_validated_side_aware_coordinates() {
        let finding = |side, line| crate::app::ai_review::AiReviewFinding {
            path: Some("src/lib.rs".to_string()),
            line,
            side,
            body: "finding".to_string(),
            diff_hunk: None,
            skipped: false,
            published: false,
        };

        assert_eq!(
            finding_location(&finding(Some(crate::diff::DiffSide::New), Some(12))),
            "src/lib.rs:12"
        );
        assert_eq!(
            finding_location(&finding(Some(crate::diff::DiffSide::Old), Some(9))),
            "src/lib.rs:9 (base)"
        );
        assert_eq!(
            finding_location(&finding(None, None)),
            "src/lib.rs",
            "an unmapped finding must not display its rejected line number"
        );
    }

    #[test]
    fn running_pane_renders_live_activity_elapsed_time_and_usage() {
        let mut state = AiReviewRunState {
            origin: AiReviewState {
                workdir: PathBuf::from("/tmp/review"),
                pr: crate::github::PrRef {
                    number: 473,
                    head_sha: "abc123".to_string(),
                    url: "https://github.com/o/r/pull/473".to_string(),
                    owner: "o".to_string(),
                    repo: "r".to_string(),
                    head_ref: "feature".to_string(),
                },
                findings: Vec::new(),
                summary: None,
                attribution: None,
                selected: 0,
                detail_scroll: 0,
                detail_content_lines: 0,
                last_run: None,
                harness: Some(AgentKind::Codex),
                harness_pick: None,
                harness_pick_origin: None,
                model: None,
                model_picked: true,
                model_pick: None,
                finding_editor: None,
                post_confirm: None,
            },
            progress: crate::app::AiReviewRunProgress {
                stage: crate::app::ai_review::AiReviewStage::Reviewing {
                    token_estimate: 95_000,
                },
                started_at: std::time::Instant::now() - std::time::Duration::from_secs(125),
                activity: Some("Inspecting the repository".to_string()),
                usage: Some((94_000, 1_200)),
            },
        };
        let backend = TestBackend::new(100, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut throbber = throbber_widgets_tui::ThrobberState::default();
        throbber.calc_next();
        terminal
            .draw(|frame| draw_ai_review_running(frame, &state, &throbber, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("Reviewing diff (~95000 tokens)"));
        assert!(rendered.contains("Current activity: Inspecting the repository"));
        assert!(rendered.contains("Elapsed: 2m 05s"));
        assert!(rendered.contains("94000 input / 1200 output tokens"));

        terminal
            .draw(|frame| {
                draw_ai_review(
                    frame,
                    &mut state.origin,
                    &Theme::default(),
                    true,
                    &throbber,
                    &[],
                )
            })
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("A view progress"));
    }

    fn pane_state_with_attribution(
        attribution: Option<crate::app::ai_review::AiReviewAttribution>,
    ) -> AiReviewState {
        AiReviewState {
            workdir: PathBuf::from("/tmp/review"),
            pr: crate::github::PrRef {
                number: 12,
                head_sha: "abc123".to_string(),
                url: "https://github.com/o/r/pull/12".to_string(),
                owner: "o".to_string(),
                repo: "r".to_string(),
                head_ref: "feature".to_string(),
            },
            findings: Vec::new(),
            summary: None,
            attribution,
            selected: 0,
            detail_scroll: 0,
            detail_content_lines: 0,
            last_run: None,
            harness: None,
            harness_pick: None,
            harness_pick_origin: None,
            model: None,
            model_picked: false,
            model_pick: None,
            finding_editor: None,
            post_confirm: None,
        }
    }

    fn render_pane(state: &mut AiReviewState) -> String {
        let backend = TestBackend::new(120, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let throbber = throbber_widgets_tui::ThrobberState::default();
        terminal
            .draw(|frame| draw_ai_review(frame, state, &Theme::default(), false, &throbber, &[]))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn pane_shows_the_model_token_cost_attribution_line_after_a_run() {
        let mut state =
            pane_state_with_attribution(Some(crate::app::ai_review::AiReviewAttribution {
                harness: Some("claude".to_string()),
                model: Some("sonnet".to_string()),
                input_tokens: Some(12_300),
                output_tokens: Some(4_500),
                estimated_cost: Some("$0.10".to_string()),
            }));
        let rendered = render_pane(&mut state);
        assert!(
            rendered.contains("harness claude · model sonnet · ~12.3k in / ~4.5k out · est. $0.10"),
            "{rendered}"
        );
    }

    #[test]
    fn pane_attribution_line_degrades_to_model_only_without_usage() {
        let mut state =
            pane_state_with_attribution(Some(crate::app::ai_review::AiReviewAttribution {
                harness: Some("codex".to_string()),
                model: None,
                input_tokens: None,
                output_tokens: None,
                estimated_cost: None,
            }));
        let rendered = render_pane(&mut state);
        assert!(rendered.contains("harness codex · model harness default · usage not reported"));
        assert!(!rendered.contains("est. $"));
    }

    #[test]
    fn pane_has_no_attribution_line_before_the_first_run() {
        let mut state = pane_state_with_attribution(None);
        let rendered = render_pane(&mut state);
        assert!(!rendered.contains("harness "));
    }
}
