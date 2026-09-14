use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};
use std::path::Path;
use unicode_width::UnicodeWidthStr;

use crate::{
    app::{
        DiffPickerState, DiffScope, DiffViewerFocus, DiffViewerLayout, DiffViewerState,
        ReviewDecision, SummaryItem,
    },
    diff::{DiffFile, DiffFileStatus, DiffHunk, DiffLine, DiffLineKind, DiffLineLocation},
    editor::VimMode,
    highlight,
    theme::Theme,
};

use super::super::dashboard::centered_rect;

#[derive(Debug, Clone, PartialEq, Eq)]
struct StyledChunk {
    text: String,
    style: Style,
}

struct FileHighlights {
    old: Option<highlight::HighlightedText>,
    new: Option<highlight::HighlightedText>,
}

pub fn draw_diff_picker(frame: &mut Frame, state: &DiffPickerState, theme: &Theme) {
    let area = centered_rect(72, 70, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

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

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(" View the whole branch and worktree, or isolate one commit."),
            Line::from(Span::styled(
                format!(" {} commit(s) on this feature branch", state.commits.len()),
                Style::default().fg(theme.text_muted.to_color()),
            )),
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
            "  commits + staged + unstaged + untracked",
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

    if let Some(error) = &state.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" Could not list feature commits: {error}"),
                Style::default().fg(theme.danger.to_color()),
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

pub fn draw_diff_viewer(frame: &mut Frame, state: &mut DiffViewerState, theme: &Theme) {
    let area = centered_rect(96, 90, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let border_color = if state.error.is_some() {
        theme.danger.to_color()
    } else {
        theme.primary.to_color()
    };

    let block = Block::default()
        .title(if state.review {
            " Final Review "
        } else if matches!(&state.scope, DiffScope::Commit(_)) {
            " Commit Diff "
        } else {
            " Current Changes "
        })
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // The review footer grows to host the multi-line feedback editor while it
    // is open; otherwise it is a two-row key hint.
    let footer_height = if state.review
        && (state.feedback_editing
            || state.editing_general
            || state.editing_line_comment
            || state.editing_file_comment
            || state.editing_suggestion)
    {
        inner.height.saturating_sub(10).clamp(4, 12)
    } else if state.review {
        // Grow to fit the hints themselves — the verdict row is dense enough to
        // wrap past two rows on an ordinary terminal — plus the line-comment
        // peek box when the cursor is parked on a line that carries a comment.
        review_hint_height(state, theme, inner) + cursor_comment_preview_rows(state)
    } else {
        2
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(8),
            Constraint::Length(footer_height),
        ])
        .split(inner);

    draw_header(frame, chunks[0], state, theme);
    draw_body(frame, chunks[1], state, theme);
    draw_footer(frame, chunks[2], state, theme);

    if state.editing_base_ref {
        draw_base_ref_prompt(frame, state, theme);
    }
    if state.editing_search {
        draw_search_prompt(frame, state, theme);
    }
    if state.changeset_overview_open {
        draw_changeset_overview_modal(frame, state, theme);
    }
    if state.interdiff_open {
        draw_interdiff_modal(frame, state, theme);
    }
    if state.summary_open {
        draw_review_summary_modal(frame, state, theme);
    }
    if state.review_history.is_some() {
        draw_review_history_modal(frame, state, theme);
    }
    // Drawn last so it sits on top of anything else the reviewer left open.
    if state.help_open {
        draw_review_help_modal(frame, state, theme);
    }
    // The destination picker and the companion-feature setup it can open sit on
    // top of everything (they capture every key while shown).
    if let Some(pick) = &state.destination_pick {
        super::draw_review_destination_pick(frame, pick, theme);
    }
    if let Some(setup) = &state.review_feature_setup {
        super::draw_review_feature_setup(frame, setup, theme);
    }
}

/// The review key surface, grouped by what the reviewer is trying to do. This
/// is the `?` overlay's content: the two footer rows can only ever show the
/// keys that apply right now, so the full set needs somewhere to live.
///
/// An empty key column is a continuation line for the entry above it.
const REVIEW_HELP_SECTIONS: &[(&str, &[(&str, &str)])] = &[
    (
        "Verdicts",
        &[
            ("a", "Approve the current file"),
            ("r", "Needs revision — opens the feedback editor"),
            ("s", "Skip the current file (no verdict)"),
            ("u", "Jump to the next file with no verdict"),
            ("U", "Undo the last verdict and go back to that file"),
        ],
    ),
    (
        "Comments",
        &[
            ("c", "Toggle the line cursor (see \"Line cursor\" below)"),
            ("m", "File comment — an observation that does not reject"),
            ("M", "Resolve / reopen this file's comment"),
            ("f", "General feedback for the whole review"),
            ("{ / }", "Previous / next comment, across every file"),
            ("Ctrl+E", "Cycle severity while an editor is open"),
            ("", "(blocker / suggestion / nit / question / praise)"),
            (
                "Ctrl+T",
                "Toggle Vim for every editor in this review session",
            ),
            ("", "(starts off; enabling Vim enters Normal mode)"),
            ("Tab", "Submit an open editor in either keymap"),
            ("Ctrl+Q", "Cancel an open editor in either keymap"),
            ("Esc", "Cancel plain editing; Vim Insert enters Normal mode"),
        ],
    ),
    (
        "Line cursor (press c first)",
        &[
            ("j / k", "Move the cursor a line at a time"),
            ("[ / ]", "Jump to the previous / next hunk"),
            ("v", "Start a range selection; v again clears it"),
            ("Enter", "Comment on the cursored line or selected span"),
            ("S", "Suggest a replacement for the line / span"),
            ("x", "Apply the cursored suggestion to the worktree"),
            ("R", "Resolve / reopen the cursored thread"),
            ("a / d", "Accept / dismiss the AI draft under the cursor"),
            ("Tab", "Jump to the next AI draft in this file"),
            ("E", "Open this file at this line in $EDITOR"),
            ("Esc / c", "Leave cursor mode"),
        ],
    ),
    (
        "Moving around",
        &[
            ("n / p", "Next / previous file"),
            ("j / k", "Scroll the patch, or walk file-tree rows"),
            ("g / G", "Jump to the top / bottom of the focused panel"),
            ("Tab", "Move focus between the file list and the patch"),
            ("PgUp / PgDn", "Scroll the patch a screen at a time"),
            ("h / l", "Collapse-or-out / expand-or-in (file list)"),
            (
                "z / Z",
                "Fold the cursored directory / whole tree (file list)",
            ),
            ("/", "Search this file's diff; n / N cycle matches"),
            ("F", "Cycle the file-list filter"),
            ("", "(all / undecided / rejected / blockers / …)"),
        ],
    ),
    (
        "Reading the diff",
        &[
            ("v", "Unified / side-by-side layout"),
            ("+ / -", "Widen / narrow the context around each hunk"),
            ("*", "Toggle straight to whole-file context and back"),
            ("W", "Toggle ignore-whitespace (git diff -w)"),
            ("e", "Expand / collapse the developer notes panel"),
            ("", "(while expanded, j / k scroll the note)"),
            ("i", "Install or repair this file's syntax parser"),
            ("E", "Open this file in $EDITOR"),
            ("b", "Diff against a different base ref"),
        ],
    ),
    (
        "Context and AI passes",
        &[
            ("w", "Generate a walkthrough for a noteless file (tokens)"),
            ("A", "AI co-review pass over this file (tokens)"),
            ("O", "Whole-changeset overview / risk summary (tokens)"),
            ("I", "Diff since the last review round (local, free)"),
            ("H", "Review-round timeline and history browser"),
        ],
    ),
    (
        "Finishing",
        &[
            (
                "t",
                "Choose where fixes go: this feature, a dedicated session, another feature, or a new one",
            ),
            ("X", "Also apply remaining suggestions when finishing"),
            ("q", "Review summary, then finish"),
            (
                "",
                "(writes feedback, may post to the PR, dispatches fixes)",
            ),
            ("Esc", "Pause — leave the review with all progress kept"),
        ],
    ),
];

/// A scrollable, read-only listing of every review-mode key (`?`). Takes full
/// key precedence while open (`handle_diff_viewer_key`), like the other review
/// modals. Groups mirror the dashboard help overlay's shape so the two read the
/// same way.
fn draw_review_help_modal(frame: &mut Frame, state: &mut DiffViewerState, theme: &Theme) {
    let area = centered_rect(72, 84, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Final Review — Keys ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(inner);

    let mut lines: Vec<Line> = Vec::new();
    for (section, binds) in REVIEW_HELP_SECTIONS {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            format!("  {section}"),
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        )));
        for (key, desc) in *binds {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {key:>14}"),
                    Style::default()
                        .fg(theme.warning.to_color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(*desc, Style::default().fg(theme.text.to_color())),
            ]));
        }
    }

    state.help_rendered_lines = lines.len();
    state.help_view_height = rows[0].height as usize;
    let scroll = state
        .help_scroll
        .min(lines.len().saturating_sub(rows[0].height as usize));

    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), rows[0]);

    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));
    let hint = Line::from(vec![
        key("j"),
        Span::raw("/"),
        key("k"),
        Span::raw(" scroll  "),
        key("g"),
        Span::raw("/"),
        key("G"),
        Span::raw(" top/bottom  "),
        key("?"),
        Span::raw("/"),
        key("q"),
        Span::raw("/"),
        key("Esc"),
        Span::raw(" close"),
    ]);
    frame.render_widget(Paragraph::new(hint), rows[1]);
}

/// Read-only review-round timeline (`H`). `Current` is generated from the
/// in-memory review so it always reflects live edits; finished rounds render
/// their preserved markdown verbatim. Historical bodies deliberately carry an
/// explicit original-diff limitation because only the latest review snapshot
/// exists today.
fn draw_review_history_modal(frame: &mut Frame, state: &mut DiffViewerState, theme: &Theme) {
    let area = centered_rect(88, 86, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Review Timeline ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .split(inner);

    let Some(history) = state.review_history.as_ref() else {
        return;
    };
    let selected = history.selected;
    let scroll = history.scroll;
    let error = history.error.clone();
    let historical = selected > 0;
    let body = if selected == 0 {
        current_review_history_markdown(state)
    } else {
        history
            .rounds
            .get(selected - 1)
            .map(|round| round.markdown.clone())
            .unwrap_or_else(|| "## Review unavailable\n".to_string())
    };

    frame.render_widget(
        Paragraph::new(review_history_timeline(
            history,
            state.unresolved_thread_count(),
            rows[0].width as usize,
            theme,
        )),
        rows[0],
    );

    let rendered =
        crate::markdown::render_markdown(&body, theme, rows[1].width.max(1) as usize, None);
    let rendered_lines = rendered.lines.len();
    frame.render_widget(
        Paragraph::new(rendered.lines)
            .scroll((scroll.min(u16::MAX as usize) as u16, 0))
            .wrap(Wrap { trim: false }),
        rows[1],
    );

    if let Some(history) = state.review_history.as_mut() {
        history.rendered_lines = rendered_lines;
        history.view_height = rows[1].height as usize;
        history.scroll = history
            .scroll
            .min(rendered_lines.saturating_sub(history.view_height));
    }

    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));
    let status = if let Some(error) = error {
        Span::styled(error, Style::default().fg(theme.danger.to_color()))
    } else if historical {
        Span::styled(
            "Historical round is read-only; its original diff snapshot is unavailable.",
            Style::default().fg(theme.text_muted.to_color()),
        )
    } else {
        Span::styled(
            "Current is live; press Enter to return to editing.",
            Style::default().fg(theme.info.to_color()),
        )
    };
    let hints = Line::from(vec![
        key("h/l"),
        Span::raw(" round  "),
        key("j/k"),
        Span::raw(" scroll  "),
        key("Enter"),
        Span::raw(" edit Current  "),
        key("q/Esc"),
        Span::raw(" close"),
    ]);
    frame.render_widget(Paragraph::new(vec![Line::from(status), hints]), rows[2]);
}

/// Window the timeline around the selected entry so long histories do not
/// wrap. Archived entries are numbered once loaded; before that, the strip
/// advertises a lazy `Older…` tail without reading it.
fn review_history_timeline(
    history: &crate::app::ReviewHistoryState,
    current_unresolved: usize,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let complete = history.archive_loaded || !history.archive_available;
    let total = history.rounds.len();
    let mut labels = Vec::with_capacity(total + 1);
    labels.push(if current_unresolved > 0 {
        format!("Current ●{current_unresolved}")
    } else {
        "Current".to_string()
    });
    for (idx, round) in history.rounds.iter().enumerate() {
        let base = if complete {
            format!("Round {}", total.saturating_sub(idx))
        } else if idx == 0 {
            "Last review".to_string()
        } else {
            format!("Earlier {}", idx)
        };
        labels.push(if round.carried_unresolved > 0 {
            format!("{base} ●{}", round.carried_unresolved)
        } else {
            base
        });
    }

    // Each entry averages ~16 columns including the separator. Keep at least
    // three visible when possible and center the selected entry in the window.
    let capacity = (width / 16).clamp(1, 7).min(labels.len().max(1));
    let mut start = history.selected.saturating_sub(capacity / 2);
    if start + capacity > labels.len() {
        start = labels.len().saturating_sub(capacity);
    }
    let end = (start + capacity).min(labels.len());

    let mut spans = Vec::new();
    if start > 0 {
        spans.push(Span::styled(
            "… ─ ",
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }
    for (idx, label) in labels.iter().enumerate().take(end).skip(start) {
        if idx > start {
            spans.push(Span::styled(
                " ─ ",
                Style::default().fg(theme.text_muted.to_color()),
            ));
        }
        let selected = idx == history.selected;
        let current = idx == 0;
        let style = if selected {
            Style::default()
                .bg(theme.shortcut_background.to_color())
                .fg(theme.shortcut_text.to_color())
                .add_modifier(Modifier::BOLD)
        } else if current {
            Style::default()
                .fg(theme.info.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text.to_color())
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }
    if end < labels.len() {
        spans.push(Span::styled(
            " ─ …",
            Style::default().fg(theme.text_muted.to_color()),
        ));
    } else if history.archive_available && !history.archive_loaded {
        spans.push(Span::styled(
            " ─ Older…",
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }
    Line::from(spans)
}

/// Compose the live `Current` history body from review state. Unlike finished
/// rounds this includes drafts and resolved threads, because it is a faithful
/// read-only projection of what the reviewer can return to and edit.
fn current_review_history_markdown(state: &DiffViewerState) -> String {
    let mut approved = 0usize;
    let mut rejected = 0usize;
    for file in &state.files {
        match state.decisions.get(&file.path) {
            Some(ReviewDecision::Approve) => approved += 1,
            Some(ReviewDecision::Reject { .. }) => rejected += 1,
            None => {}
        }
    }
    let undecided = state
        .files
        .len()
        .saturating_sub(approved)
        .saturating_sub(rejected);
    let mut out = format!(
        "## Current Review\n\n**Files:** {} | **Approved:** {approved} | **Needs work:** \
         {rejected} | **No verdict:** {undecided} | **Open threads:** {}\n\n",
        state.files.len(),
        state.unresolved_thread_count()
    );
    if state.finish_check_child.is_some() {
        out.push_str("**Check:** running…\n\n");
    }
    if !state.general_feedback.trim().is_empty() {
        out.push_str("### General Feedback\n\n");
        out.push_str(state.general_feedback.trim());
        out.push_str("\n\n");
    }
    for file in &state.files {
        let verdict = match state.decisions.get(&file.path) {
            Some(ReviewDecision::Approve) => "approved".to_string(),
            Some(ReviewDecision::Reject { severity, .. }) => {
                format!("needs work [{}]", severity.label())
            }
            None => "no verdict".to_string(),
        };
        out.push_str(&format!("### {} — {verdict}\n\n", file.path));
        if let Some(ReviewDecision::Reject { feedback, .. }) = state.decisions.get(&file.path)
            && !feedback.trim().is_empty()
        {
            out.push_str(feedback.trim());
            out.push_str("\n\n");
        }
        if let Some(comment) = state.file_comments.get(&file.path) {
            let status = if comment.resolved { "resolved" } else { "open" };
            out.push_str(&format!(
                "**File comment [{} · {status}]:** {}\n\n",
                comment.severity.label(),
                comment.text.trim()
            ));
        }
        if let Some(comments) = state.line_comments.get(&file.path) {
            for comment in comments {
                let start = comment.start.and_then(|loc| loc.new_line.or(loc.old_line));
                let end = comment.location.new_line.or(comment.location.old_line);
                let anchor = match (start, end) {
                    (Some(start), Some(end)) if start != end => format!("L{start}-{end}"),
                    (_, Some(end)) => format!("L{end}"),
                    _ => "anchor lost".to_string(),
                };
                let status = if comment.draft {
                    "AI draft"
                } else if comment.resolved {
                    "resolved"
                } else if comment.carried {
                    "open · carried"
                } else {
                    "open"
                };
                out.push_str(&format!(
                    "#### {anchor} — [{} · {status}]\n\n",
                    comment.severity.label()
                ));
                if !comment.text.trim().is_empty() {
                    out.push_str(comment.text.trim());
                    out.push_str("\n\n");
                }
                if let Some(suggestion) = &comment.suggestion {
                    out.push_str("```suggestion\n");
                    out.push_str(suggestion);
                    out.push_str("\n```\n\n");
                }
            }
        }
        if let Some(responses) = state.prior_agent_responses.get(&file.path) {
            for response in responses {
                out.push_str(&format!(
                    "**Agent reply to {}:** {}\n\n",
                    response.anchor,
                    response.response.trim()
                ));
            }
        }
    }
    out
}

/// A centered modal showing the "since last review" diff for the current
/// file (`I` in the final review): the diff between its content when the
/// last review round finished and its content now. Read-only, and takes full
/// key precedence while open, mirroring `draw_changeset_overview_modal`.
/// Reuses `draw_patch_panel` — the same unified-diff renderer as the main
/// patch panel — against the on-demand `DiffFile` computed by
/// `App::open_interdiff`, so it needs none of the comment/cursor plumbing.
fn draw_interdiff_modal(frame: &mut Frame, state: &DiffViewerState, theme: &Theme) {
    let area = centered_rect(90, 80, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(area);

    let title = state
        .interdiff_file
        .as_ref()
        .map(|file| format!("Since Last Review: {}", file.path))
        .unwrap_or_else(|| "Since Last Review".to_string());

    draw_patch_panel(
        frame,
        rows[0],
        state.interdiff_file.as_ref(),
        PatchPanelOptions {
            layout: DiffViewerLayout::Unified,
            title,
            border_color: theme.primary.to_color(),
            scroll: state.interdiff_scroll,
            include_prologue: true,
            new_file_presentation: false,
            ..Default::default()
        },
        theme,
    );

    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));
    let hint = Line::from(vec![
        key("j"),
        Span::raw("/"),
        key("k"),
        Span::raw(" scroll  "),
        key("q"),
        Span::raw("/"),
        key("Esc"),
        Span::raw(" close"),
    ]);
    frame.render_widget(Paragraph::new(hint), rows[1]);
}

/// A centered modal showing the on-demand whole-changeset overview / risk
/// summary (`O` in the final review). Read-only: while open every key is
/// captured by the modal (`handle_diff_viewer_key`) rather than the diff
/// underneath. Mirrors `draw_search_prompt`'s overlay and `draw_notes_panel`'s
/// markdown-render-and-scroll approach.
fn draw_changeset_overview_modal(frame: &mut Frame, state: &mut DiffViewerState, theme: &Theme) {
    let area = centered_rect(80, 70, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Changeset Overview ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(inner);

    let generating = state.changeset_overview_child.is_some();
    let scroll = state.changeset_overview_scroll;
    let (paragraph, rendered_lines) = if generating {
        (
            Paragraph::new("Generating changeset overview…")
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text_muted.to_color())),
            0,
        )
    } else if let Some(text) = &state.changeset_overview {
        let rendered =
            crate::markdown::render_markdown(text, theme, rows[0].width.max(1) as usize, None);
        let line_count = rendered.lines.len();
        (
            Paragraph::new(rendered.lines).scroll((scroll as u16, 0)),
            line_count,
        )
    } else {
        (
            Paragraph::new(
                "No overview generated yet.\n\nPress O to generate an on-demand summary of the \
                 whole changeset (a headless pass over every file's diff, bounded so it stays \
                 cheap on large changesets).",
            )
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text_muted.to_color())),
            0,
        )
    };
    state.changeset_overview_rendered_lines = rendered_lines;
    state.changeset_overview_view_height = rows[0].height as usize;
    frame.render_widget(paragraph, rows[0]);

    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));
    let hint = Line::from(vec![
        key("j"),
        Span::raw("/"),
        key("k"),
        Span::raw(" scroll  "),
        key("O"),
        Span::raw(" regenerate  "),
        key("q"),
        Span::raw("/"),
        key("Esc"),
        Span::raw(" close"),
    ]);
    frame.render_widget(Paragraph::new(hint), rows[1]);
}

/// A centered modal listing every verdict, open comment/suggestion and the
/// general feedback in one navigable list — the pre-finish "one last look"
/// (`q` on an undecided-free review, or `y`/`q` past the undecided-files
/// confirmation). `Enter` on a row jumps back into the diff to edit it
/// (closing the modal); `q` here is the real finish, `Esc` just closes the
/// modal and returns to reviewing. Selection scrolling is `List`/`ListState`'s
/// built-in keep-selection-visible behavior, unlike the markdown-scroll
/// modals above.
fn draw_review_summary_modal(frame: &mut Frame, state: &DiffViewerState, theme: &Theme) {
    let area = centered_rect(84, 82, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let rows_data = state.summary_items();
    let (approved, rejected, undecided) = summary_verdict_counts(state);
    let title = format!(
        " Finish Review — Summary  ({approved} approved · {rejected} needs work · {undecided} no verdict) "
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(inner);

    let mut items: Vec<ListItem> = rows_data
        .iter()
        .map(|item| ListItem::new(summary_item_line(*item, state, theme)))
        .collect();
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            "Nothing to review — no changes against the base branch.",
            Style::default().fg(theme.text_muted.to_color()),
        ))));
    }

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(theme.shortcut_background.to_color())
                .fg(theme.shortcut_text.to_color())
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");

    let mut list_state = ListState::default();
    if !rows_data.is_empty() {
        list_state.select(Some(state.summary_selected.min(rows_data.len() - 1)));
    }
    frame.render_stateful_widget(list, rows[0], &mut list_state);

    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));
    let hint = Line::from(vec![
        key("j"),
        Span::raw("/"),
        key("k"),
        Span::raw(" move  "),
        key("Enter"),
        Span::raw(" jump to edit  "),
        key("q"),
        Span::raw(" finish review  "),
        key("Esc"),
        Span::raw(" back to review"),
    ]);
    frame.render_widget(Paragraph::new(hint), rows[1]);
}

/// (approved, needs-work, no-verdict) counts across every file, for the
/// summary modal's title.
fn summary_verdict_counts(state: &DiffViewerState) -> (usize, usize, usize) {
    let mut approved = 0;
    let mut rejected = 0;
    for file in &state.files {
        match state.decisions.get(&file.path) {
            Some(ReviewDecision::Approve) => approved += 1,
            Some(ReviewDecision::Reject { .. }) => rejected += 1,
            None => {}
        }
    }
    let undecided = state
        .files
        .len()
        .saturating_sub(approved)
        .saturating_sub(rejected);
    (approved, rejected, undecided)
}

/// Truncate free text to a single flattened line for a summary row: collapse
/// whitespace/newlines and cap the length so a long comment doesn't blow out
/// the list.
fn truncate_summary(text: &str) -> String {
    const MAX_CHARS: usize = 100;
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.is_empty() {
        return "(no text)".to_string();
    }
    if flat.chars().count() > MAX_CHARS {
        format!("{}…", flat.chars().take(MAX_CHARS).collect::<String>())
    } else {
        flat
    }
}

fn summary_item_line(item: SummaryItem, state: &DiffViewerState, theme: &Theme) -> Line<'static> {
    match item {
        SummaryItem::File { file_idx } => {
            let Some(file) = state.files.get(file_idx) else {
                return Line::from("");
            };
            let (icon, color, detail) = match state.decisions.get(&file.path) {
                Some(ReviewDecision::Approve) => {
                    ("✓", theme.success.to_color(), "approved".to_string())
                }
                Some(ReviewDecision::Reject { feedback, severity }) => {
                    let text = if feedback.trim().is_empty() {
                        "needs revision — see line/file comments below".to_string()
                    } else {
                        format!("needs revision: {}", truncate_summary(feedback))
                    };
                    (
                        "✗",
                        theme.danger.to_color(),
                        format!("[{}] {}", severity.label(), text),
                    )
                }
                None => ("·", theme.text_muted.to_color(), "no verdict".to_string()),
            };
            Line::from(vec![
                Span::styled(
                    format!("{icon} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    file.path.clone(),
                    Style::default().fg(theme.text.to_color()),
                ),
                Span::raw("  "),
                Span::styled(detail, Style::default().fg(color)),
            ])
        }
        SummaryItem::LineComment {
            file_idx,
            comment_idx,
        } => {
            let Some(file) = state.files.get(file_idx) else {
                return Line::from("");
            };
            let Some(comment) = state
                .line_comments
                .get(&file.path)
                .and_then(|comments| comments.get(comment_idx))
            else {
                return Line::from("");
            };
            let line_no = comment
                .location
                .new_line
                .or(comment.location.old_line)
                .unwrap_or(0);
            let text = if comment.text.trim().is_empty() {
                "(suggested change, no comment)".to_string()
            } else {
                truncate_summary(&comment.text)
            };
            let suffix = if comment.suggestion.is_some() {
                " · suggestion"
            } else {
                ""
            };
            Line::from(vec![
                Span::raw("    "),
                Span::styled(
                    format!("L{line_no} "),
                    Style::default().fg(theme.info.to_color()),
                ),
                Span::styled(
                    format!("[{}] ", comment.severity.label()),
                    Style::default().fg(theme.text_muted.to_color()),
                ),
                Span::styled(text, Style::default().fg(theme.text.to_color())),
                Span::styled(suffix, Style::default().fg(theme.text_muted.to_color())),
            ])
        }
        SummaryItem::FileComment { file_idx } => {
            let Some(file) = state.files.get(file_idx) else {
                return Line::from("");
            };
            let Some(comment) = state.file_comments.get(&file.path) else {
                return Line::from("");
            };
            Line::from(vec![
                Span::raw("    "),
                Span::styled("file comment ", Style::default().fg(theme.info.to_color())),
                Span::styled(
                    format!("[{}] ", comment.severity.label()),
                    Style::default().fg(theme.text_muted.to_color()),
                ),
                Span::styled(
                    truncate_summary(&comment.text),
                    Style::default().fg(theme.text.to_color()),
                ),
            ])
        }
        SummaryItem::General => Line::from(vec![
            Span::styled(
                "General feedback: ",
                Style::default()
                    .fg(theme.info.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                truncate_summary(&state.general_feedback),
                Style::default().fg(theme.text.to_color()),
            ),
        ]),
    }
}

/// A small centered input overlay for searching the current file's diff. Matches
/// update incrementally as the reviewer types; the line cursor jumps to the
/// nearest hit and `n`/`N` cycle them after submitting.
fn draw_search_prompt(frame: &mut Frame, state: &DiffViewerState, theme: &Theme) {
    let area = centered_rect(60, 22, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Search Diff ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    // Live hit count for the current query.
    let status = if state.search_query.trim().is_empty() {
        "Search this file's diff (case-insensitive):".to_string()
    } else if state.search_matches.is_empty() {
        "No matches".to_string()
    } else {
        format!(
            "Match {}/{}",
            state.search_match_pos.map(|p| p + 1).unwrap_or(0),
            state.search_matches.len()
        )
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            status,
            Style::default().fg(theme.text_muted.to_color()),
        )))
        .wrap(Wrap { trim: false }),
        rows[0],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("/ ", Style::default().fg(theme.primary.to_color())),
            Span::styled(
                state.search_query.clone(),
                Style::default().fg(theme.text.to_color()),
            ),
            Span::styled("▏", Style::default().fg(theme.primary.to_color())),
        ])),
        rows[1],
    );

    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            key("Enter"),
            Span::raw(" keep (n/N to cycle)  "),
            key("Esc"),
            Span::raw(" cancel"),
        ])),
        rows[3],
    );
}

/// A small centered input overlay for choosing the diff's base ref. Submitting
/// reloads the diff against the typed ref; a blank entry reverts to the
/// auto-resolved base.
fn draw_base_ref_prompt(frame: &mut Frame, state: &DiffViewerState, theme: &Theme) {
    let area = centered_rect(60, 22, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Choose Base Ref ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "Branch, tag, or commit to compare against:",
            Style::default().fg(theme.text_muted.to_color()),
        )))
        .wrap(Wrap { trim: false }),
        rows[0],
    );

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("› ", Style::default().fg(theme.primary.to_color())),
            Span::styled(
                state.base_ref_input.clone(),
                Style::default().fg(theme.text.to_color()),
            ),
            Span::styled("▏", Style::default().fg(theme.primary.to_color())),
        ])),
        rows[1],
    );

    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            key("Enter"),
            Span::raw(" apply  "),
            key("Esc"),
            Span::raw(" cancel  "),
            Span::styled(
                "(blank = auto)",
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ])),
        rows[3],
    );
}

pub fn draw_diff_viewer_loading(
    frame: &mut Frame,
    state: &DiffViewerState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = centered_rect(54, 28, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let commit = match &state.scope {
        DiffScope::Commit(commit) => Some(commit),
        DiffScope::CurrentChanges => None,
    };
    let block = Block::default()
        .title(if commit.is_some() {
            " Commit Diff "
        } else {
            " Current Changes "
        })
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);
    let branch = if state.branch.is_empty() {
        state.from_view.feature_name.as_str()
    } else {
        state.branch.as_str()
    };
    let loading_label = if commit.is_some() {
        " Loading commit diff..."
    } else {
        " Loading current changes..."
    };
    let detail = commit
        .map(|commit| format!("{}  {}", commit.short_hash, commit.subject))
        .unwrap_or_else(|| format!("Comparing all changes for {branch}"));

    let loading = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            spinner,
            Span::styled(
                loading_label,
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            detail,
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ])
    .wrap(Wrap { trim: false });

    frame.render_widget(loading, inner);
}

fn draw_header(frame: &mut Frame, area: Rect, state: &DiffViewerState, theme: &Theme) {
    let branch = if state.branch.is_empty() {
        "(unknown branch)"
    } else {
        &state.branch
    };
    let base = if state.base_ref.is_empty() {
        "(no base)"
    } else {
        &state.base_ref
    };
    let commit = if state.base_commit.is_empty() {
        String::new()
    } else {
        let short = state.base_commit.chars().take(12).collect::<String>();
        format!(" @ {short}")
    };
    let additions: usize = state.files.iter().map(|file| file.additions).sum();
    let deletions: usize = state.files.iter().map(|file| file.deletions).sum();

    let mut second_line = vec![
        Span::styled(
            format!(" {} file(s)  ", state.files.len()),
            Style::default().fg(theme.text.to_color()),
        ),
        Span::styled(
            format!("+{additions}"),
            Style::default()
                .fg(theme.success.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            format!("-{deletions}"),
            Style::default()
                .fg(theme.danger.to_color())
                .add_modifier(Modifier::BOLD),
        ),
    ];

    if state.review {
        let (approved, rejected) = review_counts(state);
        let pending = state.files.len().saturating_sub(approved + rejected);
        second_line.push(Span::raw("   "));
        second_line.push(Span::styled(
            format!("✓ {approved}"),
            Style::default()
                .fg(theme.success.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        second_line.push(Span::raw("  "));
        second_line.push(Span::styled(
            format!("✗ {rejected}"),
            Style::default()
                .fg(theme.danger.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        second_line.push(Span::raw("  "));
        second_line.push(Span::styled(
            format!("· {pending}"),
            Style::default().fg(theme.text_muted.to_color()),
        ));
        // On a re-review, show how many files changed since the last round.
        if state.has_prior_review {
            second_line.push(Span::raw("   "));
            second_line.push(Span::styled(
                format!("Δ {} changed", state.changed_since_last.len()),
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ));
        }
    } else {
        second_line.push(Span::raw("  "));
        second_line.push(Span::styled(
            state.workdir.to_string_lossy().into_owned(),
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }

    let scope_line = match &state.scope {
        DiffScope::CurrentChanges => vec![
            Span::styled(" Branch ", Style::default().fg(theme.text_muted.to_color())),
            Span::styled(
                branch.to_string(),
                Style::default()
                    .fg(theme.project_title.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("  vs  ", Style::default().fg(theme.text_muted.to_color())),
            Span::styled(
                format!("{base}{commit}"),
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            if state.override_base_ref.is_some() {
                Span::styled("  (manual)", Style::default().fg(theme.warning.to_color()))
            } else {
                Span::raw("")
            },
        ],
        DiffScope::Commit(commit) => vec![
            Span::styled(" Commit ", Style::default().fg(theme.text_muted.to_color())),
            Span::styled(
                commit.short_hash.clone(),
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                commit.subject.clone(),
                Style::default()
                    .fg(theme.project_title.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ],
    };
    let header = Paragraph::new(vec![Line::from(scope_line), Line::from(second_line)])
        .wrap(Wrap { trim: false });

    frame.render_widget(header, area);
}

/// Count approved and rejected files in a review-mode viewer.
fn review_counts(state: &DiffViewerState) -> (usize, usize) {
    let mut approved = 0;
    let mut rejected = 0;
    for file in &state.files {
        match state.decisions.get(&file.path) {
            Some(crate::app::ReviewDecision::Approve) => approved += 1,
            Some(crate::app::ReviewDecision::Reject { .. }) => rejected += 1,
            None => {}
        }
    }
    (approved, rejected)
}

fn draw_body(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    if let Some(error) = &state.error {
        let error_widget = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                if matches!(&state.scope, DiffScope::Commit(_)) {
                    " Could not load commit diff "
                } else {
                    " Could not load current changes "
                },
                Style::default()
                    .fg(theme.danger.to_color())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                error.as_str(),
                Style::default().fg(theme.text.to_color()),
            )),
        ])
        .wrap(Wrap { trim: false });
        frame.render_widget(error_widget, area);
        return;
    }

    if state.files.is_empty() {
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                if matches!(&state.scope, DiffScope::Commit(_)) {
                    " The selected commit has no file changes "
                } else {
                    " No changes against the selected base "
                },
                Style::default()
                    .fg(theme.success.to_color())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                if matches!(&state.scope, DiffScope::Commit(_)) {
                    "This can happen for an empty commit or some merge commits."
                } else {
                    "Refresh with r after making more edits or commits."
                },
                Style::default().fg(theme.text.to_color()),
            )),
        ]);
        frame.render_widget(empty, area);
        return;
    }

    if state.review {
        draw_review_body(frame, area, state, theme);
        return;
    }

    if state.focus == DiffViewerFocus::Patch {
        draw_patch(frame, area, state, theme);
        return;
    }

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(body_constraints(area, state))
        .split(area);

    draw_file_list(frame, body[0], state, theme);
    draw_patch(frame, body[1], state, theme);
}

/// Review-mode body: file list (unless the patch is focused) on the left, and a
/// right column split into the developer-notes panel (top) over the diff. When
/// notes are expanded the panel takes the full right column.
fn draw_review_body(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    let content_area = if state.focus == DiffViewerFocus::Patch {
        area
    } else {
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(body_constraints(area, state))
            .split(area);
        draw_file_list(frame, cols[0], state, theme);
        cols[1]
    };

    if state.notes_expanded {
        draw_notes_panel(frame, content_area, state, theme);
        return;
    }

    // Give the notes panel ~20% of the column — the diff is what the reviewer
    // is actually reading, and `e` still expands notes to full height on
    // demand. Always leave room for both.
    let notes_height = (content_area.height / 5).clamp(
        5.min(content_area.height),
        content_area.height.saturating_sub(5).max(1),
    );
    let split = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(notes_height), Constraint::Min(3)])
        .split(content_area);
    draw_notes_panel(frame, split[0], state, theme);
    draw_patch(frame, split[1], state, theme);
}

fn draw_notes_panel(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    let path = state
        .files
        .get(state.selected_file)
        .map(|file| file.path.clone());
    let note = path
        .as_ref()
        .and_then(|p| state.review_notes.get(p))
        .cloned();
    let generating = path.as_ref().is_some_and(|p| {
        state.walkthrough_child.is_some() && state.walkthrough_file.as_deref() == Some(p.as_str())
    });
    let generated = path
        .as_ref()
        .and_then(|p| state.generated_notes.get(p))
        .cloned();

    // The panel titles itself after whatever it is showing, so a generated
    // walkthrough isn't mistaken for a hand-written developer note.
    let title = if note.is_none() && (generating || generated.is_some()) {
        " AI Walkthrough "
    } else {
        " Developer Notes "
    };

    let border = if state.notes_expanded {
        theme.warning.to_color()
    } else {
        theme.primary.to_color()
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let notes_scroll = state.notes_scroll;
    // Render markdown pre-wrapped to the panel width so the scroll offset maps
    // to rendered visual lines.
    let render_md = |text: &str| {
        let rendered =
            crate::markdown::render_markdown(text, theme, inner.width.max(1) as usize, None);
        let line_count = rendered.lines.len();
        (
            Paragraph::new(rendered.lines).scroll((notes_scroll as u16, 0)),
            line_count,
        )
    };
    // On a re-review, the feature agent's replies to last round's items for this
    // file (parsed from the feedback file). Rendered as a markdown section under
    // the developer note / walkthrough so the reviewer sees what the agent said.
    let responses_md = path
        .as_ref()
        .and_then(|p| state.prior_agent_responses.get(p))
        .filter(|v| !v.is_empty())
        .map(|responses| {
            let mut section = String::from("### Agent replies (last round)\n\n");
            for r in responses {
                section.push_str(&format!("**{}**\n\n{}\n\n", r.anchor, r.response));
            }
            section
        });

    let (paragraph, rendered_lines) = if generating {
        (
            Paragraph::new("Generating walkthrough…")
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text_muted.to_color())),
            0,
        )
    } else {
        // A developer note wins over a generated walkthrough as the base body;
        // the agent-replies section (if any) follows either, separated by a rule.
        let base = note.as_deref().or(generated.as_deref());
        match (base, responses_md.as_deref()) {
            (Some(text), Some(resp)) => render_md(&format!("{text}\n\n---\n\n{resp}")),
            (Some(text), None) => render_md(text),
            (None, Some(resp)) => render_md(resp),
            (None, None) => (
                Paragraph::new(
                    "No developer note for this file.\n\nPress w to generate an AI walkthrough of \
                     this file's diff. Review mode records per-file reasoning in \
                     .claude/review-notes.md as changes are made.",
                )
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(theme.text_muted.to_color())),
                0,
            ),
        }
    };

    // Record the rendered (wrapped) line count and viewport height so the scroll
    // clamp can reach the visual bottom of soft-wrapped / markdown notes.
    state.notes_rendered_lines = rendered_lines;
    state.notes_view_height = inner.height as usize;

    frame.render_widget(paragraph, inner);
}

fn body_constraints(area: Rect, state: &DiffViewerState) -> [Constraint; 2] {
    match (&effective_layout(state), &state.focus) {
        (DiffViewerLayout::SideBySide, DiffViewerFocus::Patch) => {
            let file_width = area.width.saturating_mul(22) / 100;
            [Constraint::Length(file_width.max(24)), Constraint::Min(40)]
        }
        (DiffViewerLayout::SideBySide, DiffViewerFocus::FileList) => {
            let file_width = area.width.saturating_mul(30) / 100;
            [Constraint::Length(file_width.max(30)), Constraint::Min(34)]
        }
        _ => [Constraint::Percentage(32), Constraint::Percentage(68)],
    }
}

/// A changed file whose diff crosses this many total added+removed lines is
/// flagged `L` (large) in the file list — big enough that a reviewer skimming
/// the list should expect it to take real time, small enough that it still
/// fires on plenty of ordinary changes worth flagging.
const LARGE_FILE_CHANGE_THRESHOLD: usize = 300;

/// True if any file in the changeset looks like a test file. Used as the
/// changeset-wide signal behind the file list's "no test coverage" marker:
/// there is no per-file test-mapping convention anywhere in this codebase (a
/// module's tests usually live in a sibling `tests.rs` or an inline
/// `#[cfg(test)]` block elsewhere), so this settles for "did the changeset
/// touch anything test-shaped at all".
fn changeset_has_test_changes(files: &[DiffFile]) -> bool {
    files.iter().any(|f| looks_like_test_path(&f.path))
}

fn looks_like_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/tests/")
        || lower.starts_with("tests/")
        || lower.ends_with("tests.rs")
        || lower.ends_with("_test.rs")
        || lower.ends_with("_tests.rs")
        || lower.contains("test_")
        || lower.contains("_spec.")
}

/// Non-test source extensions the "no test coverage" marker considers —
/// config/docs/markdown/lockfiles etc. are excluded since "no tests touched
/// this README" isn't a useful signal.
fn looks_like_source_path(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    matches!(
        ext,
        "rs" | "ts"
            | "tsx"
            | "js"
            | "jsx"
            | "py"
            | "go"
            | "rb"
            | "java"
            | "c"
            | "cc"
            | "cpp"
            | "h"
            | "hpp"
            | "cs"
            | "swift"
            | "kt"
    )
}

/// Build the `[L,N,T]`-style risk-marker span for a file's row (empty when no
/// flag applies): `L` large change, `N` no developer note / walkthrough yet,
/// `T` changeset has no test-looking file at all. Review mode only, mirroring
/// the `Δ` changed-since-last marker.
fn file_risk_marker(
    file: &DiffFile,
    state: &DiffViewerState,
    changeset_has_tests: bool,
    theme: &Theme,
) -> Option<Span<'static>> {
    if !state.review || file.is_binary {
        return None;
    }
    let mut flags = Vec::new();
    if file.additions + file.deletions >= LARGE_FILE_CHANGE_THRESHOLD {
        flags.push("L");
    }
    if !state.review_notes.contains_key(&file.path)
        && !state.generated_notes.contains_key(&file.path)
    {
        flags.push("N");
    }
    if !changeset_has_tests
        && looks_like_source_path(&file.path)
        && !looks_like_test_path(&file.path)
    {
        flags.push("T");
    }
    if flags.is_empty() {
        return None;
    }
    Some(Span::styled(
        format!(" [{}]", flags.join(",")),
        Style::default().fg(theme.warning.to_color()),
    ))
}

/// A collapsed directory's row summarises what it is hiding, so folding a tree
/// never hides the fact that something under it still needs attention: how many
/// files, how many still undecided, and whether any changed since the last
/// review round. Every count comes from `visible` — the files the active filter
/// shows — so the badges always describe the same set as the row's `(n)`, which
/// the tree also counts per filter.
fn dir_row_summary(
    dir: &str,
    files: usize,
    visible: &[usize],
    state: &DiffViewerState,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let prefix = format!("{dir}/");
    let under: Vec<&crate::diff::DiffFile> = visible
        .iter()
        .filter_map(|&idx| state.files.get(idx))
        .filter(|file| file.path.starts_with(&prefix))
        .collect();
    let mut spans = vec![Span::styled(
        format!("  ({files})"),
        Style::default().fg(theme.text_muted.to_color()),
    )];
    if !state.review {
        return spans;
    }
    let undecided = under
        .iter()
        .filter(|file| !state.decisions.contains_key(&file.path))
        .count();
    if undecided > 0 {
        spans.push(Span::styled(
            format!(" ·{undecided}"),
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }
    let rejected = under
        .iter()
        .filter(|file| {
            matches!(
                state.decisions.get(&file.path),
                Some(crate::app::ReviewDecision::Reject { .. })
            )
        })
        .count();
    if rejected > 0 {
        spans.push(Span::styled(
            format!(" ✗{rejected}"),
            Style::default().fg(theme.danger.to_color()),
        ));
    }
    if under
        .iter()
        .any(|file| state.changed_since_last.contains(&file.path))
    {
        spans.push(Span::styled(
            " Δ",
            Style::default().fg(theme.warning.to_color()),
        ));
    }
    spans
}

fn draw_file_list(frame: &mut Frame, area: Rect, state: &DiffViewerState, theme: &Theme) {
    let visible = state.visible_file_indices();
    let rows = state.file_tree_rows();
    let changeset_has_tests = changeset_has_test_changes(&state.files);
    let mut items: Vec<ListItem<'static>> = rows
        .iter()
        .map(|row| {
            let (idx, depth) = match row {
                crate::app::FileTreeRow::Dir {
                    path,
                    label,
                    depth,
                    collapsed,
                    files,
                } => {
                    let mut spans = vec![
                        Span::raw(" ".repeat(depth * 2 + 1)),
                        Span::styled(
                            if *collapsed { "▸ " } else { "▾ " },
                            Style::default().fg(theme.text_muted.to_color()),
                        ),
                        Span::styled(
                            format!("{label}/"),
                            Style::default()
                                .fg(theme.info.to_color())
                                .add_modifier(Modifier::BOLD),
                        ),
                    ];
                    if *collapsed {
                        spans.extend(dir_row_summary(path, *files, &visible, state, theme));
                    }
                    return ListItem::new(Line::from(spans));
                }
                crate::app::FileTreeRow::File { index, depth, .. } => (*index, *depth),
            };
            let file = &state.files[idx];
            let status_style = Style::default()
                .fg(status_color(&file.status, theme))
                .add_modifier(Modifier::BOLD);
            let mut spans = Vec::new();
            if depth > 0 {
                spans.push(Span::raw(" ".repeat(depth * 2)));
            }
            if state.review {
                let (symbol, color) = match state.decisions.get(&file.path) {
                    Some(crate::app::ReviewDecision::Approve) => ("✓", theme.success.to_color()),
                    Some(crate::app::ReviewDecision::Reject { .. }) => {
                        ("✗", theme.danger.to_color())
                    }
                    None => ("·", theme.text_muted.to_color()),
                };
                spans.push(Span::styled(
                    format!(" {symbol} "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
            }
            spans.push(Span::styled(
                format!(" {} ", status_label(&file.status)),
                status_style,
            ));
            // Flag files that changed since the last finished review round so
            // they stand out when re-reviewing.
            if state.review && state.changed_since_last.contains(&file.path) {
                spans.push(Span::styled(
                    "Δ ",
                    Style::default()
                        .fg(theme.warning.to_color())
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if state.review
                && let Some(comment) = state.file_comments.get(&file.path)
            {
                let (marker, color) = if comment.resolved {
                    ("◇ ", theme.text_muted.to_color())
                } else {
                    ("◆ ", theme.info.to_color())
                };
                spans.push(Span::styled(
                    marker,
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ));
            }
            // Only the basename: the directories are the rows above it.
            spans.push(Span::styled(
                match row {
                    crate::app::FileTreeRow::File { name, .. } => name.clone(),
                    crate::app::FileTreeRow::Dir { .. } => file.path.clone(),
                },
                Style::default().fg(theme.text.to_color()),
            ));
            spans.push(Span::styled(
                format!("  +{} -{}", file.additions, file.deletions),
                Style::default().fg(theme.text_muted.to_color()),
            ));
            if let Some(marker) = file_risk_marker(file, state, changeset_has_tests, theme) {
                spans.push(marker);
            }
            ListItem::new(Line::from(spans))
        })
        .collect();

    // A filter that matches nothing shows a muted placeholder rather than an
    // empty box.
    if items.is_empty() {
        items.push(ListItem::new(Line::from(Span::styled(
            format!(" (no {} files) ", state.file_filter.label()),
            Style::default().fg(theme.text_muted.to_color()),
        ))));
    }

    let border = if state.focus == DiffViewerFocus::FileList {
        theme.warning.to_color()
    } else {
        theme.primary.to_color()
    };
    let title = if state.review && state.file_filter != crate::app::FileFilter::All {
        format!(
            " Files ({}/{}) · {} ",
            visible.len(),
            state.files.len(),
            state.file_filter.label()
        )
    } else {
        format!(" Files ({}) ", state.files.len())
    };
    let list = List::new(items)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border)),
        )
        .highlight_style(
            Style::default()
                .bg(theme.shortcut_background.to_color())
                .fg(theme.shortcut_text.to_color())
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol(">");

    let mut list_state = ListState::default();
    // Highlight maps onto the rendered tree rows: the cursored directory when
    // the cursor is parked on one, else the selected file's row. None when the
    // selection is hidden by the active filter.
    list_state.select(state.tree_cursor_row(&rows));
    frame.render_stateful_widget(list, area, &mut list_state);
}

fn draw_patch(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    let border = if state.focus == DiffViewerFocus::Patch {
        theme.warning.to_color()
    } else {
        theme.primary.to_color()
    };
    let effective_layout = effective_layout(state);
    let ReviewLineMarkers {
        cursor: cursor_loc,
        commented,
        draft,
        blocker,
        selection,
        matched,
    } = review_cursor_info(state);

    // Keep the comment cursor visible by nudging the patch scroll. Unified only:
    // it's the layout with a per-line cursor and a stable row index.
    if state.cursor_sync_to_view
        && matches!(effective_layout, DiffViewerLayout::Unified)
        && cursor_loc.is_some()
    {
        let viewport = area.height.saturating_sub(2) as usize;
        let synced = state.files.get(state.selected_file).map(|file| {
            let width = area.width.saturating_sub(2);
            let mut cursor_row = None;
            let lines = patch_lines(
                file,
                width,
                theme,
                true,
                is_new_diff_file(file),
                cursor_loc,
                &commented,
                &draft,
                &blocker,
                &selection,
                &matched,
                &mut cursor_row,
            );
            (lines.len(), cursor_row)
        });
        if let Some((total_lines, Some(row))) = synced {
            if row < state.patch_scroll {
                state.patch_scroll = row;
            } else if viewport > 0 && row >= state.patch_scroll + viewport {
                state.patch_scroll = row + 1 - viewport;
            }
            let max = total_lines.saturating_sub(viewport.max(1));
            state.patch_scroll = state.patch_scroll.min(max);
        }
        state.cursor_sync_to_view = false;
    }

    let file = state.files.get(state.selected_file);
    let title = file
        .map(|file| {
            if is_new_diff_file(file) {
                format!("New File: {}", file.path)
            } else {
                format!("Patch: {}", file.path)
            }
        })
        .unwrap_or_else(|| "Patch".to_string());

    draw_patch_panel(
        frame,
        area,
        file,
        PatchPanelOptions {
            layout: effective_layout,
            title,
            border_color: border,
            scroll: state.patch_scroll,
            include_prologue: true,
            new_file_presentation: file.map(is_new_diff_file).unwrap_or(false),
            cursor: cursor_loc,
            commented,
            draft,
            blocker,
            selection,
            matched,
        },
        theme,
    );
}

/// The diff-line markers a review viewer needs for the current file: the
/// cursored location, every location covered by a stored comment (so a
/// multi-line comment marks its whole span), and the locations in the
/// in-progress selection (anchor..cursor). All empty outside review mode or
/// when no cursor is active.
struct ReviewLineMarkers {
    cursor: Option<DiffLineLocation>,
    commented: std::collections::HashSet<DiffLineLocation>,
    /// Lines covered by an unaccepted AI draft comment — marked distinctly from
    /// `commented` so the reviewer can tell suggestions apart from kept comments.
    draft: std::collections::HashSet<DiffLineLocation>,
    /// Subset of `commented` covered by a `Blocker`-severity comment, so the
    /// gutter can flag must-fix lines in a higher-contrast colour.
    blocker: std::collections::HashSet<DiffLineLocation>,
    selection: std::collections::HashSet<DiffLineLocation>,
    /// Lines matching the active diff search, so the gutter can mark every hit
    /// (the current match is already the cursor).
    matched: std::collections::HashSet<DiffLineLocation>,
}

fn review_cursor_info(state: &DiffViewerState) -> ReviewLineMarkers {
    use std::collections::HashSet;
    let empty = || ReviewLineMarkers {
        cursor: None,
        commented: HashSet::new(),
        draft: HashSet::new(),
        blocker: HashSet::new(),
        selection: HashSet::new(),
        matched: HashSet::new(),
    };
    if !state.review {
        return empty();
    }
    let Some(file) = state.files.get(state.selected_file) else {
        return empty();
    };
    let locs = file.addressable_lines();
    let cursor = state.comment_cursor.and_then(|idx| locs.get(idx).copied());
    let mut commented = HashSet::new();
    let mut draft = HashSet::new();
    let mut blocker = HashSet::new();
    if let Some(comments) = state.line_comments.get(&file.path) {
        for comment in comments {
            if let Some(range) = comment.covered_indices(&locs) {
                let covered: Vec<DiffLineLocation> =
                    range.filter_map(|idx| locs.get(idx).copied()).collect();
                if comment.draft {
                    draft.extend(covered);
                } else {
                    if comment.severity.is_blocker() {
                        blocker.extend(covered.iter().copied());
                    }
                    commented.extend(covered);
                }
            }
        }
    }
    // The active selection span (only meaningful while an anchor is set).
    let selection = match (state.comment_anchor, state.comment_cursor) {
        (Some(anchor), Some(cur)) => (anchor.min(cur)..=anchor.max(cur))
            .filter_map(|idx| locs.get(idx).copied())
            .collect(),
        _ => HashSet::new(),
    };
    // Every diff-search hit in the current file (empty when no search is active).
    let matched = state
        .search_matches
        .iter()
        .filter_map(|&idx| locs.get(idx).copied())
        .collect();
    ReviewLineMarkers {
        cursor,
        commented,
        draft,
        blocker,
        selection,
        matched,
    }
}

/// The review comment whose span covers the line the comment cursor is
/// currently on, if any. `None` outside review mode, when no cursor is active,
/// or when the cursored line carries no comment.
fn cursor_comment(state: &DiffViewerState) -> Option<&crate::app::LineComment> {
    if !state.review {
        return None;
    }
    let cursor = state.comment_cursor?;
    let file = state.files.get(state.selected_file)?;
    let locs = file.addressable_lines();
    state.line_comments.get(&file.path)?.iter().find(|c| {
        c.covered_indices(&locs)
            .is_some_and(|range| range.contains(&cursor))
    })
}

/// The body text of [`cursor_comment`].
#[cfg(test)]
fn cursor_comment_text(state: &DiffViewerState) -> Option<&str> {
    cursor_comment(state).map(|c| c.text.as_str())
}

/// The lines shown in the cursor-comment peek box: the prose (if any) followed
/// by a labelled preview of the suggested change (if any). Never empty.
fn cursor_comment_peek_lines(comment: &crate::app::LineComment) -> Vec<String> {
    let mut lines: Vec<String> = comment.text.lines().map(|l| l.to_string()).collect();
    // Lead a kept comment's peek with its severity so a reviewer scrolling the
    // diff sees each annotation's priority. Drafts always default to the neutral
    // severity, so tagging them would be noise.
    if !comment.draft {
        lines.insert(0, format!("[{}]", comment.severity.label()));
    }
    if let Some(suggestion) = &comment.suggestion {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.push("suggested change:".to_string());
        lines.extend(suggestion.lines().map(|l| l.to_string()));
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Height (in rows, including the box border) the footer needs to peek the
/// comment on the cursored line; `0` when there is nothing to preview. The body
/// is capped so a long comment is glimpsed rather than fully scrolled — the
/// editor (Enter) still shows it in full.
fn cursor_comment_preview_rows(state: &DiffViewerState) -> u16 {
    match cursor_comment(state) {
        Some(comment) => {
            let content = cursor_comment_peek_lines(comment).len().clamp(1, 6) as u16;
            content + 2
        }
        None => 0,
    }
}

pub(crate) struct PatchPanelOptions {
    pub layout: DiffViewerLayout,
    pub title: String,
    pub border_color: Color,
    pub scroll: usize,
    pub include_prologue: bool,
    pub new_file_presentation: bool,
    /// Review line cursor location (unified view only); highlighted when set.
    pub cursor: Option<DiffLineLocation>,
    /// Diff-line locations that carry a kept (non-draft) review comment (unified
    /// view only).
    pub commented: std::collections::HashSet<DiffLineLocation>,
    /// Diff-line locations covered by an unaccepted AI draft comment (unified
    /// view only); marked distinctly from `commented`.
    pub draft: std::collections::HashSet<DiffLineLocation>,
    /// Subset of `commented` covered by a `Blocker`-severity comment (unified
    /// view only); its gutter marker reads in the danger colour.
    pub blocker: std::collections::HashSet<DiffLineLocation>,
    /// Diff-line locations in the in-progress multi-line selection (unified view
    /// only); the gutter is tinted across the span while the reviewer extends it.
    pub selection: std::collections::HashSet<DiffLineLocation>,
    /// Diff-line locations matching the active diff search (unified view only);
    /// each hit gets a gutter marker so matches are visible while scrolling.
    pub matched: std::collections::HashSet<DiffLineLocation>,
}

impl Default for PatchPanelOptions {
    fn default() -> Self {
        Self {
            layout: DiffViewerLayout::Unified,
            title: String::new(),
            border_color: Color::Reset,
            scroll: 0,
            include_prologue: true,
            new_file_presentation: false,
            cursor: None,
            commented: std::collections::HashSet::new(),
            draft: std::collections::HashSet::new(),
            blocker: std::collections::HashSet::new(),
            selection: std::collections::HashSet::new(),
            matched: std::collections::HashSet::new(),
        }
    }
}

pub(crate) fn draw_patch_panel(
    frame: &mut Frame,
    area: Rect,
    file: Option<&DiffFile>,
    options: PatchPanelOptions,
    theme: &Theme,
) {
    let layout_label = match options.layout {
        DiffViewerLayout::Unified => "unified",
        DiffViewerLayout::SideBySide => "side-by-side",
    };
    let title = if options.new_file_presentation {
        Span::styled(
            format!(" {} [{layout_label}] ", options.title),
            Style::default()
                .fg(theme.success.to_color())
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::raw(format!(" {} [{layout_label}] ", options.title))
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(options.border_color));

    let scroll = u16::try_from(options.scroll).unwrap_or(u16::MAX);
    match file {
        Some(file) if matches!(options.layout, DiffViewerLayout::SideBySide) => {
            let lines = side_by_side_lines(
                file,
                area.width.saturating_sub(2),
                theme,
                options.include_prologue,
            );
            frame.render_widget(Paragraph::new(lines).block(block).scroll((scroll, 0)), area);
        }
        Some(file) => {
            let mut cursor_row = None;
            let lines = patch_lines(
                file,
                area.width.saturating_sub(2),
                theme,
                options.include_prologue,
                options.new_file_presentation,
                options.cursor,
                &options.commented,
                &options.draft,
                &options.blocker,
                &options.selection,
                &options.matched,
                &mut cursor_row,
            );
            frame.render_widget(
                Paragraph::new(lines)
                    .block(block)
                    .scroll((scroll, 0))
                    .wrap(Wrap { trim: false }),
                area,
            );
        }
        None => {
            let patch = Paragraph::new("No file selected").block(block);
            frame.render_widget(patch, area);
        }
    }
}

fn draw_footer(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    if state.review {
        draw_review_footer(frame, area, state, theme);
        return;
    }
    let focus = match state.focus {
        DiffViewerFocus::FileList => "files",
        DiffViewerFocus::Patch => "patch",
    };
    let new_file_selected = state
        .files
        .get(state.selected_file)
        .map(is_new_diff_file)
        .unwrap_or(false);
    let layout = match effective_layout(state) {
        DiffViewerLayout::Unified => "unified",
        DiffViewerLayout::SideBySide => "side-by-side",
    };
    let syntax_status = state
        .files
        .get(state.selected_file)
        .and_then(|file| highlight::language_install_state_for_path(Path::new(&file.path)));
    let context_level = state
        .files
        .get(state.selected_file)
        .filter(|file| file.can_expand_context())
        .map(|file| {
            state
                .context_expansion
                .get(&file.path)
                .copied()
                .unwrap_or(crate::diff::DIFF_DEFAULT_CONTEXT)
        });
    let footer = Paragraph::new(diff_footer_lines(
        focus,
        layout,
        new_file_selected,
        syntax_status,
        matches!(&state.scope, DiffScope::CurrentChanges),
        context_level,
        state.ignore_whitespace,
        theme,
    ))
    .wrap(Wrap { trim: false });
    frame.render_widget(footer, area);
}

/// Whether the `E $EDITOR` hint earns its footer slot for the selected file.
/// Shared by the cursor and non-cursor footers so the two can't drift: a
/// deleted or binary file is still selectable (and, in cursor mode, still has
/// addressable removed lines), but `E` on it can only report why it won't open.
fn editor_hint_applies(state: &DiffViewerState) -> bool {
    state
        .files
        .get(state.selected_file)
        .is_some_and(|file| file.can_open_in_editor())
}

/// Ceiling on the review footer's key hints. Both rows wrapped in full can eat
/// a lot of vertical space on a narrow terminal, and the diff is what the
/// reviewer is actually there to read.
const REVIEW_HINT_MAX_ROWS: u16 = 8;

/// Rows a hint line occupies when rendered with `Wrap { trim: false }` at
/// `width`. Ratatui breaks on word boundaries, so `ceil(total / width)`
/// undercounts; this walks the same greedy word / whitespace runs the wrapper
/// does, which is what the footer has to be sized against.
fn wrapped_line_height(line: &Line, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    let width = width as usize;
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let mut rows: usize = 1;
    let mut used: usize = 0;
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let whitespace = rest.starts_with(char::is_whitespace);
        let end = rest
            .find(|c: char| c.is_whitespace() != whitespace)
            .unwrap_or(rest.len());
        let (run, tail) = rest.split_at(end);
        rest = tail;
        let run_width = run.width();
        if whitespace {
            // Whitespace that runs off the edge is absorbed there rather than
            // pushing a row of its own.
            used = (used + run_width).min(width);
            continue;
        }
        if used > 0 && used + run_width > width {
            rows += 1;
            used = 0;
        }
        if run_width > width {
            // A word wider than the row is hard-broken across rows.
            rows += (run_width - 1) / width;
            used = run_width % width;
            if used == 0 {
                used = width;
            }
        } else {
            used += run_width;
        }
    }
    rows.min(u16::MAX as usize) as u16
}

/// Rows the two hint lines want at `width`, before any clamp to the space
/// actually available.
fn hint_rows_height(first: &Line, second: &Line, width: u16) -> u16 {
    wrapped_line_height(first, width).saturating_add(wrapped_line_height(second, width))
}

/// Height the review footer's key hints need inside `inner`, clamped so a
/// heavily-wrapped footer can't crowd the diff off a short terminal.
fn review_hint_height(state: &DiffViewerState, theme: &Theme, inner: Rect) -> u16 {
    let [first, second] = review_hint_lines(state, theme);
    // Mirror the feedback editor's reservation: header (2) + the body's Min(8).
    let cap = inner
        .height
        .saturating_sub(10)
        .clamp(2, REVIEW_HINT_MAX_ROWS);
    hint_rows_height(&first, &second, inner.width).clamp(2, cap)
}

/// Draw the two hint rows into sub-areas of their own. A single wrapping
/// `Paragraph` lets a long first row consume the whole footer and silently drop
/// the second one, taking the round-level keys (`b`, `F`, `t`, `X`, `q`, `Esc`)
/// with it — a drop that is invisible until the terminal happens to be narrow
/// enough. Sizing the second row first means the overflow lands on the first
/// row's tail instead, and that row leads with `? keys`.
fn render_hint_rows(frame: &mut Frame, area: Rect, [first, second]: [Line<'static>; 2]) {
    if area.height <= 1 {
        frame.render_widget(
            Paragraph::new(vec![first, second]).wrap(Wrap { trim: false }),
            area,
        );
        return;
    }
    let second_height = wrapped_line_height(&second, area.width).clamp(1, area.height - 1);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(area.height - second_height),
            Constraint::Length(second_height),
        ])
        .split(area);
    frame.render_widget(Paragraph::new(first).wrap(Wrap { trim: false }), chunks[0]);
    frame.render_widget(Paragraph::new(second).wrap(Wrap { trim: false }), chunks[1]);
}

fn draw_review_footer(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    if state.feedback_editing
        || state.editing_general
        || state.editing_line_comment
        || state.editing_file_comment
        || state.editing_suggestion
    {
        draw_feedback_editor(frame, area, state, theme);
        return;
    }

    let [first, second] = review_hint_lines(state, theme);

    // When the cursor sits on a commented line, peek the comment body in a
    // bordered box above the hints so the reviewer can read what they wrote
    // without re-opening the editor.
    if let Some(comment) = cursor_comment(state) {
        let hint_rows = hint_rows_height(&first, &second, area.width)
            .clamp(1, area.height.saturating_sub(3).max(1));
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(3), Constraint::Length(hint_rows)])
            .split(area);
        // A draft can never be resolved (only a kept comment can be), so the
        // resolved variants only need to branch off the non-draft arms.
        let title = match (comment.draft, comment.is_range(), comment.resolved) {
            (true, true, _) => " AI draft on these lines (a accept · d dismiss · Enter edit) ",
            (true, false, _) => " AI draft on this line (a accept · d dismiss · Enter edit) ",
            (false, true, true) => " resolved thread on these lines (Enter to edit · R reopen) ",
            (false, false, true) => " resolved thread on this line (Enter to edit · R reopen) ",
            (false, true, false) if comment.suggestion.is_some() => {
                " suggestion on these lines (x apply · Enter comment · S edit · R resolve) "
            }
            (false, false, false) if comment.suggestion.is_some() => {
                " suggestion on this line (x apply · Enter comment · S edit · R resolve) "
            }
            (false, true, false) => " comment on these lines (Enter to edit · R resolve) ",
            (false, false, false) => " comment on this line (Enter to edit · R resolve) ",
        };
        let box_color = if comment.draft {
            theme.warning.to_color()
        } else {
            theme.info.to_color()
        };
        let preview = Paragraph::new(
            cursor_comment_peek_lines(comment)
                .into_iter()
                .map(Line::from)
                .collect::<Vec<_>>(),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(box_color))
                .title(Span::styled(title, Style::default().fg(box_color))),
        )
        .wrap(Wrap { trim: false });
        frame.render_widget(preview, chunks[0]);
        render_hint_rows(frame, chunks[1], [first, second]);
        return;
    }

    render_hint_rows(frame, area, [first, second]);
}

/// The two key-hint rows the review footer shows right now: the finish
/// confirmation, the line cursor's bindings, or the standard verdict row.
/// Built separately from rendering so the footer can be *sized* to them —
/// see `review_hint_height`.
fn review_hint_lines(state: &DiffViewerState, theme: &Theme) -> [Line<'static>; 2] {
    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));

    // A pending finish confirmation overrides the normal hints: some files have
    // no verdict and the reviewer just tried to finish.
    if state.finish_confirm {
        let undecided = state
            .files
            .iter()
            .filter(|file| !state.decisions.contains_key(&file.path))
            .count();
        let first = Line::from(vec![Span::styled(
            format!(" {undecided} file(s) have no verdict — finish anyway? "),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        )]);
        let second = Line::from(vec![
            key(" q"),
            Span::raw("/"),
            key("y"),
            Span::raw(" finish anyway  "),
            key("u"),
            Span::raw(" next undecided  "),
            key("Esc"),
            Span::raw(" keep reviewing"),
        ]);
        return [first, second];
    }

    // While the line cursor is active, show its dedicated key hints instead of
    // the standard scroll/verdict row.
    if let Some(cursor) = state.comment_cursor {
        let comment_count = state
            .files
            .get(state.selected_file)
            .map(|file| {
                state
                    .line_comments
                    .get(&file.path)
                    .map(|c| c.len())
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        // When a selection anchor is set, label the span being marked instead of
        // the lone cursor line.
        let position_label = match state.comment_anchor {
            Some(anchor) => {
                let lo = anchor.min(cursor) + 1;
                let hi = anchor.max(cursor) + 1;
                format!(" selecting {lo}-{hi} ")
            }
            None => format!(" line cursor @ {} ", cursor + 1),
        };
        // Cursor mode inverts the non-cursor footer's shape — here the first
        // line is the short one and the key row is what wraps — so the help
        // hint rides on the position label instead of leading the key row.
        let mut first_spans = vec![
            Span::styled(
                position_label,
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("({comment_count} comment(s) on this file)  "),
                Style::default().fg(theme.info.to_color()),
            ),
            key("?"),
            Span::raw(" keys  "),
        ];
        // Surface a committed search so its shadowing of n/N is discoverable.
        if !state.editing_search && !state.search_query.trim().is_empty() {
            let count = if state.search_matches.is_empty() {
                "no match".to_string()
            } else {
                format!(
                    "{}/{}",
                    state.search_match_pos.map(|p| p + 1).unwrap_or(0),
                    state.search_matches.len()
                )
            };
            first_spans.push(Span::styled(
                format!("search:{} ({count})  ", state.search_query),
                Style::default().fg(theme.primary.to_color()),
            ));
            first_spans.push(key("n"));
            first_spans.push(Span::raw("/"));
            first_spans.push(key("N"));
            first_spans.push(Span::raw(" match  "));
            first_spans.push(key("Esc"));
            first_spans.push(Span::raw(" clear"));
        }
        let first = Line::from(first_spans);
        let mut second_spans = vec![
            key(" j"),
            Span::raw("/"),
            key("k"),
            Span::raw(" move  "),
            key("["),
            Span::raw("/"),
            key("]"),
            Span::raw(" hunk  "),
            key("v"),
            Span::raw(if state.comment_anchor.is_some() {
                " clear range  "
            } else {
                " select range  "
            }),
            key("Enter"),
            Span::raw(" comment  "),
            key("S"),
            Span::raw(" suggest  "),
            key("x"),
            Span::raw(" apply suggestion  "),
            key("R"),
            Span::raw(" resolve/reopen  "),
        ];
        // Same gating as the non-cursor footer: a deleted or binary file has
        // nothing for an editor to open, even though its removed lines stay
        // addressable and so can still be cursored.
        if editor_hint_applies(state) {
            second_spans.extend([key("E"), Span::raw(" $EDITOR  ")]);
        }
        // Same gating as the non-cursor footer: cross-file comment navigation
        // only earns its slot once there is a comment somewhere to jump to.
        if state.line_comments.values().any(|cs| !cs.is_empty()) {
            second_spans.extend([key("{"), Span::raw("/"), key("}"), Span::raw(" comment  ")]);
        }
        second_spans.extend([
            key("n"),
            Span::raw("/"),
            key("p"),
            Span::raw(" file  "),
            key("c"),
            Span::raw("/"),
            key("Esc"),
            Span::raw(" exit cursor  "),
            key("Ctrl+T"),
            Span::raw(if state.vim_enabled {
                " editor vim: on  "
            } else {
                " editor vim: off  "
            }),
            key("q"),
            Span::raw(" finish"),
        ]);
        return [first, Line::from(second_spans)];
    }

    let mut second_line = vec![key(" j"), Span::raw("/"), key("k"), Span::raw(" scroll  ")];

    // Surface the syntax-highlight install/select affordance for the selected
    // file, mirroring the read-only diff viewer footer. Highlighting itself is
    // already applied by draw_patch_panel; `i` (handled by the shared key
    // handler) opens the language picker to install or repair the parser.
    if let Some((language, status)) = state
        .files
        .get(state.selected_file)
        .and_then(|file| highlight::language_install_state_for_path(Path::new(&file.path)))
    {
        let (label, color) = match status {
            highlight::HighlightInstallState::Installed => (
                format!("syntax:{} installed  ", language.display_name()),
                theme.info.to_color(),
            ),
            highlight::HighlightInstallState::Available => (
                format!("install {} parser  ", language.display_name()),
                theme.warning.to_color(),
            ),
            highlight::HighlightInstallState::Broken => (
                format!("repair {} parser  ", language.display_name()),
                theme.danger.to_color(),
            ),
        };
        second_line.push(key("i"));
        second_line.push(Span::raw(" "));
        second_line.push(Span::styled(label, Style::default().fg(color)));
    }

    second_line.push(key("b"));
    second_line.push(Span::raw(" base ref  "));
    second_line.push(key("F"));
    if state.file_filter == crate::app::FileFilter::All {
        second_line.push(Span::raw(" filter  "));
    } else {
        second_line.push(Span::styled(
            format!(" filter: {}  ", state.file_filter.label()),
            Style::default().fg(theme.info.to_color()),
        ));
    }
    second_line.push(key("t"));
    let (target_label, target_color) = match state.fix_target {
        crate::app::pr_review::FixTarget::DedicatedReview => {
            (" target: dedicated  ", theme.info.to_color())
        }
        crate::app::pr_review::FixTarget::NewFeature => {
            (" target: new feature  ", theme.info.to_color())
        }
        crate::app::pr_review::FixTarget::ExistingFeature => {
            (" target: other feature  ", theme.info.to_color())
        }
        crate::app::pr_review::FixTarget::ExistingLive => {
            (" target: live  ", theme.text_muted.to_color())
        }
    };
    second_line.push(Span::styled(
        target_label,
        Style::default().fg(target_color),
    ));
    let pending_suggestions = state.pending_suggestion_count();
    if pending_suggestions > 0 {
        second_line.push(key("X"));
        second_line.push(Span::styled(
            if state.apply_suggestions_on_finish {
                format!(" apply at finish: on ({pending_suggestions})  ")
            } else {
                format!(" apply {pending_suggestions} at finish  ")
            },
            Style::default().fg(if state.apply_suggestions_on_finish {
                theme.info.to_color()
            } else {
                theme.text_muted.to_color()
            }),
        ));
    }
    second_line.push(key("Ctrl+T"));
    second_line.push(Span::styled(
        if state.vim_enabled {
            " editor vim: on  "
        } else {
            " editor vim: off  "
        },
        Style::default().fg(if state.vim_enabled {
            theme.info.to_color()
        } else {
            theme.text_muted.to_color()
        }),
    ));
    second_line.push(key("q"));
    second_line.push(Span::raw(" review summary → finish  "));
    second_line.push(key("Esc"));
    second_line.push(Span::raw(" pause (keep progress)"));

    // `?` leads the row rather than joining the second one: the first line is
    // long enough to wrap into both footer rows on a narrow terminal, clipping
    // the second, and the pointer to the full key list is the one hint that
    // must never be the thing that falls off.
    let mut first_line = vec![
        key(" ?"),
        Span::raw(" keys  "),
        key("a"),
        Span::raw(" approve  "),
        key("r"),
        Span::raw(" reject  "),
        key("s"),
        Span::raw(" skip  "),
        key("f"),
        Span::raw(" general feedback  "),
        key("m"),
        Span::raw(" file comment  "),
        key("c"),
        Span::raw(" line comments  "),
        key("/"),
        Span::raw(" search"),
    ];
    if !state.general_feedback.trim().is_empty() {
        first_line.push(Span::styled(
            " ✎ note set",
            Style::default().fg(theme.info.to_color()),
        ));
    }
    if let Some(comment) = state
        .files
        .get(state.selected_file)
        .and_then(|file| state.file_comments.get(&file.path))
    {
        first_line.push(Span::styled(
            if comment.resolved {
                " ◇ file comment resolved  "
            } else {
                " ◆ file comment set  "
            },
            Style::default().fg(if comment.resolved {
                theme.text_muted.to_color()
            } else {
                theme.info.to_color()
            }),
        ));
        first_line.push(key("M"));
        first_line.push(Span::raw(if comment.resolved {
            " reopen"
        } else {
            " resolve"
        }));
    }
    first_line.extend([
        Span::raw("  "),
        key("n"),
        Span::raw("/"),
        key("p"),
        Span::raw(" file  "),
    ]);
    // Cross-file comment navigation only means something once something has
    // been commented on, so it stays out of an otherwise dense footer until then.
    if state.line_comments.values().any(|cs| !cs.is_empty()) {
        first_line.extend([key("{"), Span::raw("/"), key("}"), Span::raw(" comment  ")]);
    }
    // Likewise the undo hint: shown only while there's a verdict to take back.
    if !state.verdict_undo.is_empty() {
        first_line.extend([key("U"), Span::raw(" undo verdict  ")]);
    }
    // Tree folding only means something once the changeset spans directories.
    if state.files.iter().any(|file| file.path.contains('/')) {
        first_line.extend([key("z"), Span::raw("/"), key("Z"), Span::raw(" fold  ")]);
    }
    first_line.extend([
        key("e"),
        Span::raw(if state.notes_expanded {
            " show diff  "
        } else {
            " expand notes  "
        }),
        key("Tab"),
        Span::raw(" focus  "),
    ]);
    // Opening in `$EDITOR` needs a file that exists on disk with text in it, so
    // it stays hidden for a deletion or a binary blob rather than advertising a
    // key that can only report why it can't work.
    if editor_hint_applies(state) {
        first_line.extend([key("E"), Span::raw(" $EDITOR  ")]);
    }
    let layout_label = match effective_layout(state) {
        DiffViewerLayout::Unified => "unified",
        DiffViewerLayout::SideBySide => "side-by-side",
    };
    let new_file_selected = state
        .files
        .get(state.selected_file)
        .map(is_new_diff_file)
        .unwrap_or(false);
    if new_file_selected {
        first_line.push(Span::raw(format!(" layout:{layout_label} (new file)")));
    } else {
        first_line.push(key("v"));
        first_line.push(Span::raw(format!(" layout:{layout_label}")));
    }

    // Context expansion only means something for a file with hunks to widen —
    // an added/deleted/binary file already shows everything it can.
    if let Some(file) = state.files.get(state.selected_file)
        && file.can_expand_context()
    {
        let level = state
            .context_expansion
            .get(&file.path)
            .copied()
            .unwrap_or(crate::diff::DIFF_DEFAULT_CONTEXT);
        first_line.push(Span::raw("  "));
        first_line.push(key("+"));
        first_line.push(Span::raw("/"));
        first_line.push(key("-"));
        first_line.push(Span::styled(
            format!(" context:{}", crate::app::context_level_label(level)),
            Style::default().fg(if level == crate::diff::DIFF_DEFAULT_CONTEXT {
                theme.text_muted.to_color()
            } else {
                theme.info.to_color()
            }),
        ));
    }

    first_line.push(Span::raw("  "));
    first_line.push(key("W"));
    first_line.push(Span::styled(
        if state.ignore_whitespace {
            " ws: ignored"
        } else {
            " ws: shown"
        },
        Style::default().fg(if state.ignore_whitespace {
            theme.info.to_color()
        } else {
            theme.text_muted.to_color()
        }),
    ));

    // Offer the on-demand walkthrough only when the current file has no
    // developer note (the case where the notes panel would otherwise be empty).
    let has_note = state
        .files
        .get(state.selected_file)
        .is_some_and(|file| state.review_notes.contains_key(&file.path));
    if !has_note {
        first_line.push(Span::raw("  "));
        first_line.push(key("w"));
        first_line.push(Span::raw(" gen walkthrough"));
    }

    // Reviewer-triggered AI co-review pass over the current file.
    first_line.push(Span::raw("  "));
    first_line.push(key("A"));
    first_line.push(Span::raw(" AI review"));

    // Reviewer-triggered whole-changeset overview / risk summary.
    first_line.push(Span::raw("  "));
    first_line.push(key("O"));
    first_line.push(Span::raw(" overview"));

    // Read-only timeline across the live review and every finished round.
    first_line.push(Span::raw("  "));
    first_line.push(key("H"));
    first_line.push(Span::raw(" history"));

    // Offer the interdiff only for a file that actually changed since the
    // last review — the case where re-reading the whole diff to find the fix
    // is the exact pain this feature answers.
    let current_changed = state
        .files
        .get(state.selected_file)
        .is_some_and(|file| state.changed_since_last.contains(&file.path));
    if current_changed {
        first_line.push(Span::raw("  "));
        first_line.push(key("I"));
        first_line.push(Span::raw(" since last review"));
    }

    // Surface the jump-to-next-undecided affordance only while files still lack
    // a verdict.
    let undecided = state
        .files
        .iter()
        .filter(|file| !state.decisions.contains_key(&file.path))
        .count();
    if undecided > 0 {
        first_line.push(Span::raw("  "));
        first_line.push(key("u"));
        first_line.push(Span::styled(
            format!(" next undecided ({undecided})"),
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }

    [Line::from(first_line), Line::from(second_line)]
}

/// Render the multi-line feedback editor (per-file rejection or general
/// feedback) into the review footer: a titled box with the editor text and a
/// key hint, mirroring the steering-prompt dialog.
fn draw_feedback_editor(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));

    let vim = state.feedback_editor.vim_mode();
    let mode_label = match vim {
        Some(VimMode::Insert) => " [Vim Insert]",
        Some(VimMode::Normal) => " [Vim Normal]",
        None => "",
    };
    // The line-comment and rejection editors carry a conventional-comments
    // severity (cycled with Ctrl+E); surface the current one in the title.
    let carries_severity =
        state.editing_line_comment || state.editing_file_comment || state.feedback_editing;
    let severity_title = if carries_severity {
        format!(" [{}]", state.comment_severity.label())
    } else {
        String::new()
    };
    let (title, border_color) = if state.editing_general {
        (
            format!(" General Feedback{mode_label} "),
            theme.info.to_color(),
        )
    } else if state.editing_file_comment {
        let path = state
            .files
            .get(state.selected_file)
            .map(|file| file.path.as_str())
            .unwrap_or("file");
        (
            format!(" File Comment — {path}{severity_title}{mode_label} "),
            theme.info.to_color(),
        )
    } else if state.editing_line_comment || state.editing_suggestion {
        let anchor = state
            .comment_cursor
            .and_then(|idx| {
                state.files.get(state.selected_file).and_then(|file| {
                    file.addressable_lines().get(idx).copied().map(|loc| {
                        match (loc.new_line, loc.old_line) {
                            (Some(new_line), _) => format!("{}:{new_line}", file.path),
                            (None, Some(old_line)) => format!("{}:{old_line} (base)", file.path),
                            (None, None) => file.path.clone(),
                        }
                    })
                })
            })
            .unwrap_or_else(|| "line".to_string());
        let label = if state.editing_suggestion {
            "Suggested change"
        } else {
            "Line Comment"
        };
        (
            format!(" {label} — {anchor}{severity_title}{mode_label} "),
            theme.warning.to_color(),
        )
    } else {
        (
            format!(" Rejection Feedback{severity_title}{mode_label} "),
            theme.danger.to_color(),
        )
    };

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let editor_inner = block.inner(rows[0]);
    frame.render_widget(block, rows[0]);

    let placeholder = if state.editing_suggestion {
        "Edit the replacement code for these line(s)."
    } else {
        "Write feedback for the agent. Markdown is fine."
    };
    let editor_lines = super::editor_view::editor_lines(&state.feedback_editor, theme, placeholder);
    let visible_lines = editor_inner.height as usize;
    let mut wrap_width = editor_inner.width as usize;
    let mut total_visual_lines =
        super::editor_view::count_wrapped_editor_lines(&editor_lines, wrap_width);
    if total_visual_lines > visible_lines && wrap_width > 1 {
        wrap_width -= 1;
        total_visual_lines =
            super::editor_view::count_wrapped_editor_lines(&editor_lines, wrap_width);
    }
    super::editor_view::sync_editor_scroll(
        &state.feedback_editor,
        &mut state.feedback_scroll,
        &mut state.feedback_sync_to_cursor,
        visible_lines,
        wrap_width,
        total_visual_lines,
    );

    let paragraph = Paragraph::new(editor_lines)
        .wrap(Wrap { trim: false })
        .scroll((state.feedback_scroll.min(u16::MAX as usize) as u16, 0));
    frame.render_widget(paragraph, editor_inner);

    if total_visual_lines > visible_lines {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(total_visual_lines)
            .position(state.feedback_scroll)
            .viewport_content_length(visible_lines);
        frame.render_stateful_widget(scrollbar, rows[0], &mut scrollbar_state);
    }

    let cancel_hint = if vim.is_some() {
        key("Ctrl+Q")
    } else {
        key("Esc")
    };
    let mut hint_spans = vec![
        key(" Tab"),
        Span::raw(" submit  "),
        cancel_hint,
        Span::raw(" cancel  "),
        key("Enter"),
        Span::raw(" newline  "),
    ];
    if carries_severity {
        hint_spans.push(key("Ctrl+E"));
        hint_spans.push(Span::raw(format!(
            " severity: [{}]  ",
            state.comment_severity.label()
        )));
    }
    hint_spans.extend([
        key("Ctrl+T"),
        Span::raw(if vim.is_some() {
            " session vim off  "
        } else {
            " session vim on  "
        }),
        key("Ctrl+J/K"),
        Span::raw(" scroll"),
    ]);
    frame.render_widget(Paragraph::new(Line::from(hint_spans)), rows[1]);
}

#[allow(clippy::too_many_arguments)]
fn diff_footer_lines(
    focus: &str,
    layout: &str,
    new_file_selected: bool,
    syntax_status: Option<(
        highlight::HighlightLanguage,
        highlight::HighlightInstallState,
    )>,
    can_change_base: bool,
    context_level: Option<usize>,
    ignore_whitespace: bool,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut primary = vec![
        Span::styled(" Tab", Style::default().fg(theme.warning.to_color())),
        Span::raw(format!(" focus:{focus}  ")),
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
        Span::raw(" close"),
    ]);

    let mut secondary = Vec::new();
    if new_file_selected {
        secondary.push(Span::styled(
            format!(" layout:{layout} (new file)  "),
            Style::default().fg(theme.info.to_color()),
        ));
    } else {
        secondary.push(Span::styled(
            "v",
            Style::default().fg(theme.warning.to_color()),
        ));
        secondary.push(Span::raw(format!(" layout:{layout}  ")));
    }
    secondary.extend(vec![
        Span::styled("j/k", Style::default().fg(theme.warning.to_color())),
        Span::raw(" move  "),
        Span::styled("z/Z", Style::default().fg(theme.warning.to_color())),
        Span::raw(" fold  "),
        Span::styled("PgUp/PgDn", Style::default().fg(theme.warning.to_color())),
        Span::raw(" patch  "),
        Span::styled("g/G", Style::default().fg(theme.warning.to_color())),
        Span::raw(" top/bottom  "),
        Span::styled("r", Style::default().fg(theme.warning.to_color())),
        Span::raw(" refresh  "),
    ]);
    if let Some(level) = context_level {
        secondary.push(Span::styled(
            "+/-",
            Style::default().fg(theme.warning.to_color()),
        ));
        secondary.push(Span::styled(
            format!(" context:{}  ", crate::app::context_level_label(level)),
            Style::default().fg(if level == crate::diff::DIFF_DEFAULT_CONTEXT {
                theme.text_muted.to_color()
            } else {
                theme.info.to_color()
            }),
        ));
    }
    secondary.push(Span::styled(
        "W",
        Style::default().fg(theme.warning.to_color()),
    ));
    secondary.push(Span::styled(
        if ignore_whitespace {
            " ws: ignored  "
        } else {
            " ws: shown  "
        },
        Style::default().fg(if ignore_whitespace {
            theme.info.to_color()
        } else {
            theme.text_muted.to_color()
        }),
    ));
    if can_change_base {
        secondary.push(Span::styled(
            "b",
            Style::default().fg(theme.warning.to_color()),
        ));
        secondary.push(Span::raw(" base ref"));
    }

    vec![Line::from(primary), Line::from(secondary)]
}

#[allow(clippy::too_many_arguments)]
fn patch_lines(
    file: &DiffFile,
    width: u16,
    theme: &Theme,
    include_prologue: bool,
    new_file_presentation: bool,
    cursor: Option<DiffLineLocation>,
    commented: &std::collections::HashSet<DiffLineLocation>,
    draft: &std::collections::HashSet<DiffLineLocation>,
    blocker: &std::collections::HashSet<DiffLineLocation>,
    selection: &std::collections::HashSet<DiffLineLocation>,
    matched: &std::collections::HashSet<DiffLineLocation>,
    cursor_row: &mut Option<usize>,
) -> Vec<Line<'static>> {
    let content_width = width as usize;
    if file.is_binary || file.hunks.is_empty() || content_width < 16 {
        return raw_patch_wrapped_lines(file, content_width, theme);
    }

    let number_width = line_number_width(file);
    let gutter_width = number_width * 2 + 4;
    if content_width <= gutter_width + 4 {
        return raw_patch_wrapped_lines(file, content_width, theme);
    }
    let text_width = content_width - gutter_width;
    let highlights = file_highlights(file);
    let added_style = if new_file_presentation {
        new_file_added_row_style(theme)
    } else {
        added_row_style(theme)
    };
    let hunk_style = if new_file_presentation {
        new_file_hunk_header_style(theme)
    } else {
        hunk_header_style(theme)
    };

    let annotation = |loc: DiffLineLocation| GutterAnnotation {
        cursor: cursor == Some(loc),
        has_comment: commented.contains(&loc),
        draft: draft.contains(&loc),
        is_blocker: blocker.contains(&loc),
        selected: selection.contains(&loc),
        search_match: matched.contains(&loc),
    };

    let mut lines = Vec::new();
    if include_prologue {
        for meta in patch_prologue(file) {
            lines.extend(wrap_gutter_line(
                None,
                None,
                plain_chunks(meta, meta_style(meta, theme)),
                meta_style(meta, theme),
                number_width,
                text_width,
                GutterAnnotation::default(),
                theme,
            ));
        }
    }

    for (idx, hunk) in file.hunks.iter().enumerate() {
        if idx > 0 {
            lines.push(Line::from(""));
        }
        lines.extend(wrap_gutter_line(
            None,
            None,
            plain_chunks(&format_hunk_header(hunk), hunk_style),
            hunk_style,
            number_width,
            text_width,
            GutterAnnotation::default(),
            theme,
        ));

        let mut old_line = hunk.old_start;
        let mut new_line = hunk.new_start;
        // Which tokens changed within each paired removed/added line.
        let intra = hunk_intra_line_ranges(hunk);
        for (line_idx, diff_line) in hunk.lines.iter().enumerate() {
            match diff_line.kind {
                DiffLineKind::Context => {
                    let loc = DiffLineLocation {
                        old_line: Some(old_line),
                        new_line: Some(new_line),
                    };
                    let ann = annotation(loc);
                    if ann.cursor {
                        *cursor_row = Some(lines.len());
                    }
                    lines.extend(wrap_gutter_line(
                        Some(old_line),
                        Some(new_line),
                        diff_chunks(
                            &diff_line.text,
                            context_row_style(theme),
                            theme,
                            highlighted_line(highlights.new.as_ref(), new_line)
                                .or_else(|| highlighted_line(highlights.old.as_ref(), old_line)),
                        ),
                        context_row_style(theme),
                        number_width,
                        text_width,
                        ann,
                        theme,
                    ));
                    old_line += 1;
                    new_line += 1;
                }
                DiffLineKind::Removed => {
                    let loc = DiffLineLocation {
                        old_line: Some(old_line),
                        new_line: None,
                    };
                    let ann = annotation(loc);
                    if ann.cursor {
                        *cursor_row = Some(lines.len());
                    }
                    lines.extend(wrap_gutter_line(
                        Some(old_line),
                        None,
                        diff_chunks_emphasized(
                            &diff_line.text,
                            removed_row_style(theme),
                            theme,
                            highlighted_line(highlights.old.as_ref(), old_line),
                            intra[line_idx]
                                .clone()
                                .map(|ranges| IntraLineEmphasis {
                                    ranges,
                                    style: removed_emphasis_style(theme),
                                })
                                .as_ref(),
                        ),
                        removed_row_style(theme),
                        number_width,
                        text_width,
                        ann,
                        theme,
                    ));
                    old_line += 1;
                }
                DiffLineKind::Added => {
                    let loc = DiffLineLocation {
                        old_line: None,
                        new_line: Some(new_line),
                    };
                    let ann = annotation(loc);
                    if ann.cursor {
                        *cursor_row = Some(lines.len());
                    }
                    lines.extend(wrap_gutter_line(
                        None,
                        Some(new_line),
                        diff_chunks_emphasized(
                            &diff_line.text,
                            added_style,
                            theme,
                            highlighted_line(highlights.new.as_ref(), new_line),
                            // A brand-new file has no counterpart lines, so its
                            // rows never carry emphasis.
                            intra[line_idx]
                                .clone()
                                .map(|ranges| IntraLineEmphasis {
                                    ranges,
                                    style: added_emphasis_style(theme),
                                })
                                .as_ref(),
                        ),
                        added_style,
                        number_width,
                        text_width,
                        ann,
                        theme,
                    ));
                    new_line += 1;
                }
                DiffLineKind::NoNewlineMarker => {
                    lines.extend(wrap_gutter_line(
                        None,
                        None,
                        plain_chunks(&diff_line.text, meta_subtle_style(theme)),
                        meta_subtle_style(theme),
                        number_width,
                        text_width,
                        GutterAnnotation::default(),
                        theme,
                    ));
                }
            }
        }
    }

    lines
}

fn side_by_side_lines(
    file: &DiffFile,
    width: u16,
    theme: &Theme,
    include_prologue: bool,
) -> Vec<Line<'static>> {
    if file.is_binary || file.hunks.is_empty() || width < 24 {
        return patch_lines(
            file,
            width,
            theme,
            include_prologue,
            false,
            None,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &mut None,
        );
    }

    let inner_width = width as usize;
    let separator = " | ";
    let column_width = inner_width.saturating_sub(separator.len()) / 2;
    let number_width = line_number_width(file);
    let cell_prefix_width = number_width + 2;
    if column_width <= cell_prefix_width + 6 {
        return patch_lines(
            file,
            width,
            theme,
            include_prologue,
            false,
            None,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &mut None,
        );
    }
    let cell_text_width = column_width - cell_prefix_width;
    let highlights = file_highlights(file);

    let mut lines = vec![Line::from(vec![
        Span::styled(
            pad_cell("BASE", column_width),
            removed_row_style(theme).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            separator.to_string(),
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            pad_cell("CURRENT", column_width),
            added_row_style(theme).add_modifier(Modifier::BOLD),
        ),
    ])];

    if include_prologue {
        for meta in patch_prologue(file) {
            for chunk in wrap_text_to_width(meta, inner_width) {
                lines.push(Line::from(Span::styled(chunk, meta_style(meta, theme))));
            }
        }
    }

    for (idx, hunk) in file.hunks.iter().enumerate() {
        if idx > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            format_hunk_header(hunk),
            hunk_header_style(theme),
        )));

        let mut index = 0usize;
        let mut old_line = hunk.old_start;
        let mut new_line = hunk.new_start;
        while index < hunk.lines.len() {
            match hunk.lines[index].kind {
                DiffLineKind::Context => {
                    let text = trim_diff_prefix(&hunk.lines[index]).to_string();
                    lines.extend(side_by_side_rows(
                        Some(old_line),
                        Some(new_line),
                        format!(" {text}"),
                        format!(" {text}"),
                        highlighted_line(highlights.old.as_ref(), old_line),
                        highlighted_line(highlights.new.as_ref(), new_line),
                        context_row_style(theme),
                        context_row_style(theme),
                        number_width,
                        cell_text_width,
                        separator,
                        theme,
                    ));
                    index += 1;
                    old_line += 1;
                    new_line += 1;
                }
                DiffLineKind::Removed => {
                    let removed = collect_run(&hunk.lines, &mut index, DiffLineKind::Removed);
                    let added = collect_run(&hunk.lines, &mut index, DiffLineKind::Added);
                    let row_count = removed.len().max(added.len());
                    for row in 0..row_count {
                        let left = removed
                            .get(row)
                            .map(|line| format!("-{}", trim_diff_prefix(line)))
                            .unwrap_or_default();
                        let right = added
                            .get(row)
                            .map(|line| format!("+{}", trim_diff_prefix(line)))
                            .unwrap_or_default();
                        let left_number = removed.get(row).map(|_| old_line + row);
                        let right_number = added.get(row).map(|_| new_line + row);
                        lines.extend(side_by_side_rows(
                            left_number,
                            right_number,
                            left,
                            right,
                            left_number
                                .and_then(|line| highlighted_line(highlights.old.as_ref(), line)),
                            right_number
                                .and_then(|line| highlighted_line(highlights.new.as_ref(), line)),
                            removed_row_style(theme),
                            added_row_style(theme),
                            number_width,
                            cell_text_width,
                            separator,
                            theme,
                        ));
                    }
                    old_line += removed.len();
                    new_line += added.len();
                }
                DiffLineKind::Added => {
                    let added = collect_run(&hunk.lines, &mut index, DiffLineKind::Added);
                    for (row, line) in added.iter().enumerate() {
                        lines.extend(side_by_side_rows(
                            None,
                            Some(new_line + row),
                            String::new(),
                            format!("+{}", trim_diff_prefix(line)),
                            None,
                            highlighted_line(highlights.new.as_ref(), new_line + row),
                            neutral_side_style(theme),
                            added_row_style(theme),
                            number_width,
                            cell_text_width,
                            separator,
                            theme,
                        ));
                    }
                    new_line += added.len();
                }
                DiffLineKind::NoNewlineMarker => {
                    lines.push(Line::from(Span::styled(
                        hunk.lines[index].text.clone(),
                        meta_subtle_style(theme),
                    )));
                    index += 1;
                }
            }
        }
    }

    lines
}

fn format_hunk_header(hunk: &crate::diff::DiffHunk) -> String {
    format!(
        " Change: base {} -> current {} ",
        format_hunk_range(hunk.old_start, hunk.old_lines),
        format_hunk_range(hunk.new_start, hunk.new_lines)
    )
}

fn format_hunk_range(start: usize, len: usize) -> String {
    match len {
        0 => format!("{start}"),
        1 => format!("{start}"),
        _ => format!("{start}-{}", start + len - 1),
    }
}

fn effective_layout(state: &DiffViewerState) -> DiffViewerLayout {
    state
        .files
        .get(state.selected_file)
        .filter(|file| is_new_diff_file(file))
        .map(|_| DiffViewerLayout::Unified)
        .unwrap_or_else(|| state.layout.clone())
}

fn is_new_diff_file(file: &DiffFile) -> bool {
    matches!(
        file.status,
        DiffFileStatus::Added | DiffFileStatus::Untracked
    )
}

fn status_label(status: &DiffFileStatus) -> &'static str {
    match status {
        DiffFileStatus::Added => "A",
        DiffFileStatus::Modified => "M",
        DiffFileStatus::Deleted => "D",
        DiffFileStatus::Renamed => "R",
        DiffFileStatus::Copied => "C",
        DiffFileStatus::TypeChanged => "T",
        DiffFileStatus::Untracked => "U",
    }
}

fn status_color(status: &DiffFileStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        DiffFileStatus::Added | DiffFileStatus::Untracked => theme.success.to_color(),
        DiffFileStatus::Modified | DiffFileStatus::Renamed | DiffFileStatus::Copied => {
            theme.warning.to_color()
        }
        DiffFileStatus::Deleted => theme.danger.to_color(),
        DiffFileStatus::TypeChanged => theme.info.to_color(),
    }
}

fn collect_run(lines: &[DiffLine], index: &mut usize, kind: DiffLineKind) -> Vec<DiffLine> {
    let mut run = Vec::new();
    while *index < lines.len() && lines[*index].kind == kind {
        run.push(lines[*index].clone());
        *index += 1;
    }
    run
}

fn trim_diff_prefix(line: &DiffLine) -> &str {
    line.text
        .strip_prefix(['+', '-', ' '])
        .unwrap_or(line.text.as_str())
}

// Layout inputs for one paired diff row; they vary per call site, so a
// struct would be built and torn down for every row.
#[allow(clippy::too_many_arguments)]
fn side_by_side_rows(
    left_number: Option<usize>,
    right_number: Option<usize>,
    left: String,
    right: String,
    left_highlight: Option<&highlight::HighlightedLine>,
    right_highlight: Option<&highlight::HighlightedLine>,
    left_style: Style,
    right_style: Style,
    number_width: usize,
    text_width: usize,
    separator: &str,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let base_bg = popup_base_bg(theme);
    let paired_change_row = left_number.is_some()
        && right_number.is_some()
        && left_style.bg != Some(base_bg)
        && right_style.bg != Some(base_bg);
    // A row showing a removal beside its replacement is exactly the pair a
    // word-level diff describes, so derive the emphasis here rather than
    // threading it through yet another parameter.
    let intra = if paired_change_row {
        crate::worddiff::word_diff_cached(line_content(&left), line_content(&right))
    } else {
        None
    };
    let left_wrapped = if left_number.is_none() && left.is_empty() {
        vec![plain_chunks(
            &hatch_fill(text_width, 0),
            hatched_side_style(right_style, theme),
        )]
    } else {
        wrap_chunks(
            &diff_chunks_emphasized(
                &left,
                left_style,
                theme,
                left_highlight,
                intra
                    .as_ref()
                    .map(|diff| IntraLineEmphasis {
                        ranges: diff.old.clone(),
                        style: removed_emphasis_style(theme),
                    })
                    .as_ref(),
            ),
            text_width,
            left_style,
        )
    };
    let right_wrapped = if right_number.is_none() && right.is_empty() {
        vec![plain_chunks(
            &hatch_fill(text_width, 0),
            hatched_side_style(left_style, theme),
        )]
    } else {
        wrap_chunks(
            &diff_chunks_emphasized(
                &right,
                right_style,
                theme,
                right_highlight,
                intra
                    .as_ref()
                    .map(|diff| IntraLineEmphasis {
                        ranges: diff.new.clone(),
                        style: added_emphasis_style(theme),
                    })
                    .as_ref(),
            ),
            text_width,
            right_style,
        )
    };
    let row_count = left_wrapped.len().max(right_wrapped.len());
    let mut rows = Vec::with_capacity(row_count);
    let left_missing = left_number.is_none() && left.is_empty();
    let right_missing = right_number.is_none() && right.is_empty();

    for row in 0..row_count {
        let left_has_content = left_wrapped
            .get(row)
            .map(|chunks| !chunks.is_empty())
            .unwrap_or(false);
        let right_has_content = right_wrapped
            .get(row)
            .map(|chunks| !chunks.is_empty())
            .unwrap_or(false);
        let left_prefix = if row == 0 {
            line_number_label(left_number, number_width)
        } else {
            blank_line_number_label(number_width)
        };
        let right_prefix = if row == 0 {
            line_number_label(right_number, number_width)
        } else {
            blank_line_number_label(number_width)
        };
        let left_cell_style = if left_missing {
            hatched_side_style(right_style, theme)
        } else if left_has_content {
            left_style
        } else {
            context_row_style(theme)
        };
        let right_cell_style = if right_missing {
            hatched_side_style(left_style, theme)
        } else if right_has_content {
            right_style
        } else {
            context_row_style(theme)
        };
        let left_bg = left_cell_style.bg.unwrap_or_else(|| popup_base_bg(theme));
        let right_bg = right_cell_style.bg.unwrap_or_else(|| popup_base_bg(theme));
        let left_cell = if left_missing {
            pad_chunks_to_width(
                plain_chunks(&hatch_fill(text_width, row), left_cell_style),
                text_width,
                left_cell_style,
            )
        } else {
            pad_chunks_to_width(
                left_wrapped.get(row).cloned().unwrap_or_default(),
                text_width,
                if paired_change_row {
                    Style::default().bg(base_bg)
                } else {
                    left_style
                },
            )
        };
        let right_cell = if right_missing {
            pad_chunks_to_width(
                plain_chunks(&hatch_fill(text_width, row), right_cell_style),
                text_width,
                right_cell_style,
            )
        } else {
            pad_chunks_to_width(
                right_wrapped.get(row).cloned().unwrap_or_default(),
                text_width,
                if paired_change_row {
                    Style::default().bg(base_bg)
                } else {
                    right_style
                },
            )
        };
        let mut line = vec![Span::styled(left_prefix, left_cell_style)];
        line.push(Span::styled(
            "│ ",
            Style::default()
                .fg(line_number_fg(left_cell_style, left_bg))
                .bg(left_bg),
        ));
        line.extend(chunks_to_spans(left_cell));
        line.push(Span::styled(
            separator.to_string(),
            Style::default().fg(theme.text_muted.to_color()).bg(
                if paired_change_row && (!left_has_content || !right_has_content) {
                    base_bg
                } else {
                    blend_color(left_bg, right_bg, 0.5)
                },
            ),
        ));
        line.push(Span::styled(right_prefix, right_cell_style));
        line.push(Span::styled(
            "│ ",
            Style::default()
                .fg(line_number_fg(right_cell_style, right_bg))
                .bg(right_bg),
        ));
        line.extend(chunks_to_spans(right_cell));
        rows.push(Line::from(line));
    }

    rows
}

fn raw_patch_wrapped_lines(file: &DiffFile, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for line in file.patch.lines() {
        for chunk in wrap_text_to_width(line, width) {
            lines.push(Line::from(Span::styled(chunk, meta_style(line, theme))));
        }
    }
    lines
}

/// Per-row marker state for the review line cursor. `cursor` is the line the
/// reviewer is positioned on; `has_comment` is a line that already carries a
/// comment; `selected` is a line inside the in-progress multi-line selection.
/// Default (all false) renders the ordinary `│ ` gutter.
#[derive(Clone, Copy, Default)]
struct GutterAnnotation {
    cursor: bool,
    has_comment: bool,
    /// Line carries an unaccepted AI draft comment (rendered as a hollow marker
    /// distinct from a kept comment's filled one).
    draft: bool,
    /// The kept comment on this line is `Blocker`-severity, so its marker reads
    /// in the higher-contrast danger colour.
    is_blocker: bool,
    selected: bool,
    /// The line matches the active diff search. Lowest-priority marker: shown
    /// only when the line isn't the cursor / commented / a draft (the current
    /// match is already the cursor's solid marker).
    search_match: bool,
}

#[allow(clippy::too_many_arguments)]
fn wrap_gutter_line(
    old_number: Option<usize>,
    new_number: Option<usize>,
    chunks: Vec<StyledChunk>,
    style: Style,
    number_width: usize,
    text_width: usize,
    annotation: GutterAnnotation,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let wrapped = wrap_chunks(&chunks, text_width, style);
    let mut lines = Vec::with_capacity(wrapped.len());
    let row_bg = style.bg.unwrap_or(Color::Black);
    // The cursor row and every line in the active selection get a
    // selection-tinted gutter; the cursor row also gets a high-contrast number
    // colour so it reads clearly against any diff-row background.
    let gutter_bg = if annotation.cursor || annotation.selected {
        theme.selection.to_color()
    } else {
        row_bg
    };
    let number_fg = if annotation.cursor {
        theme.text.to_color()
    } else {
        line_number_fg(style, row_bg)
    };
    // A kept comment takes priority over a draft marker when both somehow land on
    // a line. Drafts read as a hollow circle in the warning colour ("pending
    // adjudication") vs a kept comment's filled info-coloured dot.
    let (marker, marker_fg) = if annotation.cursor {
        if annotation.has_comment {
            ("◆ ", theme.warning.to_color())
        } else if annotation.draft {
            ("◈ ", theme.warning.to_color())
        } else {
            ("▶ ", theme.warning.to_color())
        }
    } else if annotation.has_comment {
        // A blocker's dot reads in danger so must-fix lines stand out.
        let color = if annotation.is_blocker {
            theme.danger.to_color()
        } else {
            theme.info.to_color()
        };
        ("● ", color)
    } else if annotation.draft {
        ("○ ", theme.warning.to_color())
    } else if annotation.search_match {
        // A hollow triangle echoes the cursor's solid ▶ so a match reads as
        // "another place to jump", in the primary colour to stay clear of the
        // comment / draft marker hues.
        ("▷ ", theme.primary.to_color())
    } else {
        ("│ ", line_number_fg(style, row_bg))
    };
    for (index, chunk_line) in wrapped.into_iter().enumerate() {
        let old_label = if index == 0 {
            line_number_label(old_number, number_width)
        } else {
            blank_line_number_label(number_width)
        };
        let new_label = if index == 0 {
            line_number_label(new_number, number_width)
        } else {
            blank_line_number_label(number_width)
        };
        // The marker glyph only shows on the first visual line of a logical
        // row; continuation lines keep the plain separator (tinted on cursor).
        let (sep_text, sep_fg) = if index == 0 {
            (marker, marker_fg)
        } else {
            ("│ ", number_fg)
        };
        let mut line = vec![
            Span::styled(old_label, Style::default().fg(number_fg).bg(gutter_bg)),
            Span::styled(" ", Style::default().bg(gutter_bg)),
            Span::styled(new_label, Style::default().fg(number_fg).bg(gutter_bg)),
            Span::styled(" ", Style::default().bg(gutter_bg)),
            Span::styled(
                sep_text.to_string(),
                Style::default().fg(sep_fg).bg(gutter_bg),
            ),
        ];
        if annotation.cursor {
            // Bold the content of the cursor row for extra emphasis without
            // disturbing syntax-highlight colours.
            line.extend(chunks_to_spans(chunk_line).into_iter().map(|span| {
                let style = span.style.add_modifier(Modifier::BOLD);
                Span::styled(span.content, style)
            }));
        } else {
            line.extend(chunks_to_spans(chunk_line));
        }
        lines.push(Line::from(line));
    }
    lines
}

fn diff_chunks(
    text: &str,
    row_style: Style,
    theme: &Theme,
    highlighted_line: Option<&highlight::HighlightedLine>,
) -> Vec<StyledChunk> {
    diff_chunks_emphasized(text, row_style, theme, highlighted_line, None)
}

/// As `diff_chunks`, but additionally brightens the byte ranges in `emphasis`
/// (offsets into the line's content, prefix excluded) — the tokens a
/// word-level diff found actually changed. Only the background is touched, so
/// syntax highlighting shows through unchanged.
fn diff_chunks_emphasized(
    text: &str,
    row_style: Style,
    theme: &Theme,
    highlighted_line: Option<&highlight::HighlightedLine>,
    emphasis: Option<&IntraLineEmphasis>,
) -> Vec<StyledChunk> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut chars = text.chars();
    let first = chars.next().expect("diff chunk text should not be empty");
    let (prefix, content) = if matches!(first, '+' | '-' | ' ') {
        (Some(first), chars.as_str())
    } else {
        (None, text)
    };

    let mut chunks = Vec::new();
    if let Some(prefix) = prefix {
        chunks.push(StyledChunk {
            text: prefix.to_string(),
            style: row_style,
        });
    }

    if !content.is_empty() {
        let mut content_chunks = Vec::new();
        append_highlighted_content(
            &mut content_chunks,
            content,
            row_style,
            theme,
            highlighted_line,
        );
        if let Some(emphasis) = emphasis {
            content_chunks =
                apply_intra_line_emphasis(content_chunks, &emphasis.ranges, emphasis.style);
        }
        chunks.extend(content_chunks);
    }

    chunks
}

/// The changed byte ranges of one line plus the style to mark them with.
struct IntraLineEmphasis {
    ranges: Vec<std::ops::Range<usize>>,
    style: Style,
}

/// Split `chunks` at every emphasis boundary and patch `style` onto the parts
/// that fall inside a changed range. `chunks` must cover the line's content
/// contiguously from offset 0, which is what `append_highlighted_content`
/// guarantees.
fn apply_intra_line_emphasis(
    chunks: Vec<StyledChunk>,
    ranges: &[std::ops::Range<usize>],
    style: Style,
) -> Vec<StyledChunk> {
    if ranges.is_empty() {
        return chunks;
    }
    let mut rebuilt: Vec<StyledChunk> = Vec::with_capacity(chunks.len());
    let mut offset = 0usize;
    for chunk in chunks.iter() {
        let chunk_start = offset;
        offset += chunk.text.len();
        // Cut points inside this chunk where emphasis starts or stops.
        let mut cuts: Vec<usize> = vec![0, chunk.text.len()];
        for range in ranges {
            for edge in [range.start, range.end] {
                if edge > chunk_start && edge < offset {
                    cuts.push(edge - chunk_start);
                }
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for pair in cuts.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            let Some(text) = chunk.text.get(from..to) else {
                // A cut landed mid-character: keep the chunk whole rather than
                // slicing a UTF-8 boundary.
                continue;
            };
            let absolute = chunk_start + from;
            let emphasized = ranges
                .iter()
                .any(|range| absolute >= range.start && absolute < range.end);
            rebuilt.push(StyledChunk {
                text: text.to_string(),
                style: if emphasized {
                    chunk.style.patch(style)
                } else {
                    chunk.style
                },
            });
        }
    }
    rebuilt
}

fn append_highlighted_content(
    chunks: &mut Vec<StyledChunk>,
    content: &str,
    row_style: Style,
    theme: &Theme,
    highlighted_line: Option<&highlight::HighlightedLine>,
) {
    let Some(highlighted_line) = highlighted_line else {
        chunks.push(StyledChunk {
            text: content.to_string(),
            style: row_style,
        });
        return;
    };

    if highlighted_line.spans.is_empty() {
        chunks.push(StyledChunk {
            text: content.to_string(),
            style: row_style,
        });
        return;
    }

    let mut rendered_any = false;
    let mut remaining = content;
    for span in &highlighted_line.spans {
        if remaining.is_empty() {
            break;
        }
        if span.text.is_empty() {
            continue;
        }

        let consumed = consume_shared_prefix(remaining, &span.text);
        if consumed.is_empty() {
            continue;
        }

        chunks.push(StyledChunk {
            text: consumed.to_string(),
            style: row_style.patch(highlight::style_for_class(span.class, theme)),
        });
        remaining = &remaining[consumed.len()..];
        rendered_any = true;
    }

    if !remaining.is_empty() {
        chunks.push(StyledChunk {
            text: remaining.to_string(),
            style: row_style,
        });
    } else if !rendered_any {
        chunks.push(StyledChunk {
            text: content.to_string(),
            style: row_style,
        });
    }
}

fn consume_shared_prefix<'a>(content: &'a str, highlighted: &str) -> &'a str {
    let mut end = 0usize;
    for (left, right) in content.chars().zip(highlighted.chars()) {
        if left != right {
            break;
        }
        end += left.len_utf8();
    }
    &content[..end]
}

fn file_highlights(file: &DiffFile) -> FileHighlights {
    let old = file.old_content.as_deref().map(|source| {
        highlight::highlight_source(highlight::HighlightRequest {
            path: file.old_path.as_deref().map(Path::new),
            language_hint: None,
            source,
        })
    });
    let new = file.new_content.as_deref().map(|source| {
        highlight::highlight_source(highlight::HighlightRequest {
            path: Some(Path::new(&file.path)),
            language_hint: None,
            source,
        })
    });

    FileHighlights { old, new }
}

fn highlighted_line(
    highlighted: Option<&highlight::HighlightedText>,
    line_number: usize,
) -> Option<&highlight::HighlightedLine> {
    line_number
        .checked_sub(1)
        .and_then(|index| highlighted.and_then(|text| text.lines.get(index)))
}

fn plain_chunks(text: &str, style: Style) -> Vec<StyledChunk> {
    if text.is_empty() {
        Vec::new()
    } else {
        vec![StyledChunk {
            text: text.to_string(),
            style,
        }]
    }
}

fn wrap_chunks(
    chunks: &[StyledChunk],
    width: usize,
    fallback_style: Style,
) -> Vec<Vec<StyledChunk>> {
    if width == 0 {
        return vec![Vec::new()];
    }
    if chunks.is_empty() {
        return vec![Vec::new()];
    }

    let mut lines = Vec::new();
    let mut current = Vec::new();
    let mut used = 0usize;

    for chunk in chunks {
        for ch in chunk.text.chars() {
            let ch_width = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4])).max(1);
            if used + ch_width > width && !current.is_empty() {
                lines.push(current);
                current = Vec::new();
                used = 0;
            }

            push_chunk_char(&mut current, chunk.style, ch);
            used += ch_width;

            if used >= width {
                lines.push(current);
                current = Vec::new();
                used = 0;
            }
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(plain_chunks("", fallback_style));
    }

    lines
}

fn push_chunk_char(chunks: &mut Vec<StyledChunk>, style: Style, ch: char) {
    if let Some(last) = chunks.last_mut()
        && last.style == style
    {
        last.text.push(ch);
        return;
    }
    chunks.push(StyledChunk {
        text: ch.to_string(),
        style,
    });
}

fn chunks_to_spans(chunks: Vec<StyledChunk>) -> Vec<Span<'static>> {
    chunks
        .into_iter()
        .map(|chunk| Span::styled(chunk.text, chunk.style))
        .collect()
}

fn pad_chunks_to_width(
    mut chunks: Vec<StyledChunk>,
    width: usize,
    pad_style: Style,
) -> Vec<StyledChunk> {
    let used = chunks_width(&chunks);
    if used < width {
        chunks.push(StyledChunk {
            text: " ".repeat(width - used),
            style: pad_style,
        });
    }
    chunks
}

fn chunks_width(chunks: &[StyledChunk]) -> usize {
    chunks
        .iter()
        .map(|chunk| UnicodeWidthStr::width(chunk.text.as_str()))
        .sum()
}

fn line_number_width(file: &DiffFile) -> usize {
    let mut max_line = 1usize;
    for hunk in &file.hunks {
        max_line = max_line.max(hunk.old_start.saturating_add(hunk.old_lines));
        max_line = max_line.max(hunk.new_start.saturating_add(hunk.new_lines));
    }
    max_line.to_string().len().max(1)
}

fn patch_prologue(file: &DiffFile) -> Vec<&str> {
    if is_new_diff_file(file) {
        return vec!["NEW FILE added in this branch"];
    }
    file.patch
        .lines()
        .take_while(|line| !line.starts_with("@@ "))
        .collect()
}

fn meta_style(line: &str, theme: &Theme) -> Style {
    if line.starts_with("NEW FILE ") || line.starts_with("New file ") {
        new_file_hunk_header_style(theme)
    } else if line.starts_with("diff --git ")
        || line.starts_with("index ")
        || line.starts_with("new file mode ")
        || line.starts_with("deleted file mode ")
        || line.starts_with("rename from ")
        || line.starts_with("rename to ")
        || line.starts_with("copy from ")
        || line.starts_with("copy to ")
    {
        meta_subtle_style(theme)
    } else if line.starts_with("@@ ") {
        hunk_header_style(theme)
    } else if line.starts_with('+') && !line.starts_with("+++") {
        added_row_style(theme)
    } else if line.starts_with('-') && !line.starts_with("---") {
        removed_row_style(theme)
    } else if line.starts_with("+++ ") || line.starts_with("--- ") {
        meta_subtle_style(theme)
    } else {
        context_row_style(theme)
    }
}

fn line_number_label(number: Option<usize>, width: usize) -> String {
    match number {
        Some(number) => format!("{number:>width$}", width = width),
        None => blank_line_number_label(width),
    }
}

fn blank_line_number_label(width: usize) -> String {
    " ".repeat(width)
}

fn popup_base_bg(theme: &Theme) -> Color {
    theme.background.to_color()
}

fn context_row_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.text.to_color())
        .bg(popup_base_bg(theme))
}

fn neutral_side_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.text_muted.to_color())
        .bg(blend_color(
            popup_base_bg(theme),
            theme.header_background.to_color(),
            0.42,
        ))
}

fn added_row_style(theme: &Theme) -> Style {
    Style::default().fg(theme.text.to_color()).bg(blend_color(
        popup_base_bg(theme),
        theme.success.to_color(),
        0.28,
    ))
}

/// Background for the tokens a word-level diff found changed: the row's own
/// hue, blended harder so the changed part reads as "more added" / "more
/// removed" rather than as a different kind of thing. Only the background is
/// set, so the syntax-highlight foreground survives.
fn added_emphasis_style(theme: &Theme) -> Style {
    Style::default().bg(blend_color(
        popup_base_bg(theme),
        theme.success.to_color(),
        0.62,
    ))
}

fn removed_emphasis_style(theme: &Theme) -> Style {
    Style::default().bg(blend_color(
        popup_base_bg(theme),
        theme.danger.to_color(),
        0.58,
    ))
}

/// Word-level emphasis for every line of a hunk, indexed the same as
/// `hunk.lines`.
///
/// Consecutive removed lines followed by consecutive added lines form a change
/// block; within it the i-th removal pairs with the i-th addition, which is how
/// git lays out a rewritten run and therefore what the reviewer reads as "this
/// line became that line". Unpaired leftovers (a block that removes 3 and adds
/// 1) get no emphasis — there is no counterpart to diff against.
///
/// Runs on every frame, so the per-pair token diff comes from
/// `worddiff::word_diff_cached` rather than being recomputed: only the (cheap)
/// walk over the hunk's lines is repeated.
fn hunk_intra_line_ranges(hunk: &DiffHunk) -> Vec<Option<Vec<std::ops::Range<usize>>>> {
    let mut out: Vec<Option<Vec<std::ops::Range<usize>>>> = vec![None; hunk.lines.len()];
    let mut idx = 0usize;
    while idx < hunk.lines.len() {
        if !matches!(hunk.lines[idx].kind, DiffLineKind::Removed) {
            idx += 1;
            continue;
        }
        let removed_start = idx;
        while idx < hunk.lines.len() && matches!(hunk.lines[idx].kind, DiffLineKind::Removed) {
            idx += 1;
        }
        let added_start = idx;
        while idx < hunk.lines.len() && matches!(hunk.lines[idx].kind, DiffLineKind::Added) {
            idx += 1;
        }

        let removed = removed_start..added_start;
        let added = added_start..idx;
        for (old_idx, new_idx) in removed.zip(added) {
            let old_text = line_content(&hunk.lines[old_idx].text);
            let new_text = line_content(&hunk.lines[new_idx].text);
            if let Some(diff) = crate::worddiff::word_diff_cached(old_text, new_text) {
                out[old_idx] = Some(diff.old.clone());
                out[new_idx] = Some(diff.new.clone());
            }
        }
    }
    out
}

/// A diff line's text with its leading `+`/`-`/space prefix removed, matching
/// the offsets `diff_chunks_emphasized` measures emphasis ranges against.
fn line_content(text: &str) -> &str {
    let mut chars = text.chars();
    match chars.next() {
        Some('+' | '-' | ' ') => chars.as_str(),
        _ => text,
    }
}

fn removed_row_style(theme: &Theme) -> Style {
    Style::default().fg(theme.text.to_color()).bg(blend_color(
        popup_base_bg(theme),
        theme.danger.to_color(),
        0.26,
    ))
}

fn hunk_header_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.info.to_color())
        .bg(blend_color(
            popup_base_bg(theme),
            theme.info.to_color(),
            0.12,
        ))
        .add_modifier(Modifier::BOLD)
}

fn new_file_added_row_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.success.to_color())
        .bg(popup_base_bg(theme))
}

fn new_file_hunk_header_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.success.to_color())
        .bg(blend_color(
            popup_base_bg(theme),
            theme.success.to_color(),
            0.18,
        ))
        .add_modifier(Modifier::BOLD)
}

fn meta_subtle_style(theme: &Theme) -> Style {
    Style::default()
        .fg(theme.text_muted.to_color())
        .bg(blend_color(
            popup_base_bg(theme),
            theme.primary.to_color(),
            0.08,
        ))
}

fn line_number_fg(style: Style, row_bg: Color) -> Color {
    style.fg.unwrap_or(blend_color(row_bg, Color::White, 0.55))
}

fn hatched_side_style(reference: Style, theme: &Theme) -> Style {
    let row_bg = reference.bg.unwrap_or_else(|| popup_base_bg(theme));
    Style::default()
        .fg(blend_color(row_bg, Color::White, 0.42))
        .bg(row_bg)
}

fn hatch_fill(width: usize, row: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let _ = row;
    " ".repeat(width)
}

fn blend_color(base: Color, overlay: Color, alpha: f32) -> Color {
    let alpha = alpha.clamp(0.0, 1.0);
    let (br, bg, bb) = color_to_rgb(base);
    let (or, og, ob) = color_to_rgb(overlay);
    Color::Rgb(
        ((br as f32 * (1.0 - alpha)) + (or as f32 * alpha)).round() as u8,
        ((bg as f32 * (1.0 - alpha)) + (og as f32 * alpha)).round() as u8,
        ((bb as f32 * (1.0 - alpha)) + (ob as f32 * alpha)).round() as u8,
    )
}

fn color_to_rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Black => (0, 0, 0),
        Color::Red => (205, 49, 49),
        Color::Green => (13, 188, 121),
        Color::Yellow => (229, 229, 16),
        Color::Blue => (36, 114, 200),
        Color::Magenta => (188, 63, 188),
        Color::Cyan => (17, 168, 205),
        Color::Gray => (204, 204, 204),
        Color::DarkGray => (118, 118, 118),
        Color::LightRed => (241, 76, 76),
        Color::LightGreen => (35, 209, 139),
        Color::LightYellow => (245, 245, 67),
        Color::LightBlue => (59, 142, 234),
        Color::LightMagenta => (214, 112, 214),
        Color::LightCyan => (41, 184, 219),
        Color::White => (242, 242, 242),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(i) => (i, i, i),
        Color::Reset => (48, 52, 70),
    }
}

fn pad_cell(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }

    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthStr::width(ch.encode_utf8(&mut [0; 4]));
        if used + ch_width > width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }

    if used < width {
        out.push_str(&" ".repeat(width - used));
    }

    out
}

fn wrap_text_to_width(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut out = Vec::new();
    let mut current = String::new();
    let mut used = 0usize;

    for ch in text.chars() {
        let mut buf = [0; 4];
        let ch_str = ch.encode_utf8(&mut buf);
        let ch_width = UnicodeWidthStr::width(ch_str).max(1);

        if used + ch_width > width && !current.is_empty() {
            out.push(current);
            current = String::new();
            used = 0;
        }

        current.push(ch);
        used += ch_width;

        if used >= width {
            out.push(current);
            current = String::new();
            used = 0;
        }
    }

    if !current.is_empty() {
        out.push(current);
    }

    if out.is_empty() {
        out.push(String::new());
    }

    out
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

    fn changed_pair_hunk(old: &str, new: &str) -> DiffHunk {
        DiffHunk {
            header: "@@ -1 +1 @@".into(),
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: format!("-{old}"),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: format!("+{new}"),
                },
            ],
        }
    }

    #[test]
    fn intra_line_ranges_pair_a_removal_with_its_replacement() {
        let hunk = changed_pair_hunk("let x = foo(1);", "let x = foo(2);");
        let ranges = hunk_intra_line_ranges(&hunk);

        // Index 0 is the removal, index 1 its paired addition.
        let old = ranges[0].as_ref().expect("removal has emphasis");
        let new = ranges[1].as_ref().expect("addition has emphasis");
        assert_eq!(&"let x = foo(1);"[old[0].clone()], "1");
        assert_eq!(&"let x = foo(2);"[new[0].clone()], "2");
    }

    #[test]
    fn intra_line_ranges_skip_unpaired_and_context_lines() {
        // Two removals, one addition: only the first pair has a counterpart.
        let hunk = DiffHunk {
            header: "@@ -1,3 +1,2 @@".into(),
            old_start: 1,
            old_lines: 3,
            new_start: 1,
            new_lines: 2,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: " unchanged".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: "-value = 1;".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Removed,
                    text: "-dropped();".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "+value = 2;".into(),
                },
            ],
        };
        let ranges = hunk_intra_line_ranges(&hunk);

        assert!(ranges[0].is_none(), "context lines are never emphasised");
        assert!(ranges[1].is_some(), "first removal pairs with the addition");
        assert!(ranges[2].is_none(), "second removal has no counterpart");
        assert!(ranges[3].is_some());
    }

    #[test]
    fn emphasis_splits_chunks_without_losing_or_reordering_text() {
        let theme = Theme::default();
        let row = added_row_style(&theme);
        let emphasis = IntraLineEmphasis {
            // Two disjoint runs inside "abcdef": "bc" and "e".
            ranges: Vec::from([1..3, 4..5]),
            style: added_emphasis_style(&theme),
        };
        let chunks = diff_chunks_emphasized("+abcdef", row, &theme, None, Some(&emphasis));

        let text: String = chunks.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(text, "+abcdef", "no text may be lost or reordered");

        // Only the emphasised slices carry the brighter background; the rest
        // keeps the plain row background.
        let emphasised: String = chunks
            .iter()
            .filter(|c| c.style.bg == added_emphasis_style(&theme).bg)
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(emphasised, "bce");
    }

    #[test]
    fn emphasis_offsets_are_measured_past_the_diff_prefix() {
        let theme = Theme::default();
        // Offsets are into the *content*, not the raw line — starting a range
        // at 0 must highlight `a`, never the `+` prefix.
        let emphasis = IntraLineEmphasis {
            ranges: Vec::from([0..3, 5..6]),
            style: added_emphasis_style(&theme),
        };
        let chunks = diff_chunks_emphasized(
            "+abcdef",
            added_row_style(&theme),
            &theme,
            None,
            Some(&emphasis),
        );

        let emphasised: String = chunks
            .iter()
            .filter(|c| c.style.bg == added_emphasis_style(&theme).bg)
            .map(|c| c.text.as_str())
            .collect();
        assert_eq!(emphasised, "abcf");
    }

    #[test]
    fn diff_footer_shows_the_ignore_whitespace_state() {
        let theme = Theme::default();
        let shown = diff_footer_lines("files", "unified", false, None, true, None, false, &theme);
        assert!(line_text(&shown[1]).contains("ws: shown"));

        let ignored = diff_footer_lines("files", "unified", false, None, true, None, true, &theme);
        assert!(line_text(&ignored[1]).contains("ws: ignored"));
    }

    #[test]
    fn diff_footer_prioritizes_syntax_install_hint() {
        let theme = Theme::default();
        let lines = diff_footer_lines(
            "files",
            "unified",
            true,
            Some((
                highlight::HighlightLanguage::Tsx,
                highlight::HighlightInstallState::Available,
            )),
            true,
            None,
            false,
            &theme,
        );

        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).contains("install tsx parser"));
        assert!(line_text(&lines[0]).contains("Esc close"));
        assert!(line_text(&lines[1]).contains("layout:unified (new file)"));
    }

    #[test]
    fn file_comment_editor_expands_footer_and_renders_edit_box() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.editing_file_comment = true;
        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("File Comment — a.rs"));
        assert!(rendered.contains("Write feedback for the agent. Markdown is fine."));
    }

    #[test]
    fn review_footer_reports_session_vim_state() {
        let (mut state, _) = single_added_line_review_state();
        let theme = Theme::default();

        let plain = review_hint_lines(&state, &theme);
        assert!(line_text(&plain[1]).contains("editor vim: off"));

        state.toggle_feedback_vim();
        let vim = review_hint_lines(&state, &theme);
        assert!(line_text(&vim[1]).contains("editor vim: on"));
    }

    #[test]
    fn active_review_editor_reports_normal_mode_and_session_toggle() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.editing_file_comment = true;
        state.toggle_feedback_vim();
        let backend = TestBackend::new(120, 36);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("[Vim Normal]"));
        assert!(rendered.contains("session vim off"));
    }

    #[test]
    fn review_help_documents_session_vim_submit_and_cancel_controls() {
        let comments = REVIEW_HELP_SECTIONS
            .iter()
            .find(|(title, _)| *title == "Comments")
            .expect("Comments help section")
            .1;

        assert!(comments.iter().any(|(key, text)| {
            *key == "Ctrl+T" && text.contains("every editor in this review session")
        }));
        assert!(
            comments
                .iter()
                .any(|(key, text)| *key == "Tab" && text.contains("either keymap"))
        );
        assert!(
            comments
                .iter()
                .any(|(key, text)| *key == "Ctrl+Q" && text.contains("Cancel"))
        );
        assert!(
            comments
                .iter()
                .any(|(_, text)| text.contains("enabling Vim enters Normal mode"))
        );
    }

    #[test]
    fn new_file_patch_lines_preserve_syntax_coloring_for_indented_javascript() {
        if highlight::HighlightLanguage::JavaScript.install_state()
            != highlight::HighlightInstallState::Installed
        {
            return;
        }

        crate::highlight::reload_runtime_state();

        let theme = Theme::default();
        let file = DiffFile {
            old_path: None,
            path: "syntax-test.js".to_string(),
            status: DiffFileStatus::Added,
            additions: 3,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: Some("const palette = {\n  primary: \"#0f172a\",\n};\n".to_string()),
            patch: "\
diff --git a/syntax-test.js b/syntax-test.js
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/syntax-test.js
@@ -0,0 +1,3 @@
+const palette = {
+  primary: \"#0f172a\",
+};
"
            .to_string(),
            hunks: vec![crate::diff::DiffHunk {
                header: "@@ -0,0 +1,3 @@".to_string(),
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: 3,
                lines: vec![
                    crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: "+const palette = {".to_string(),
                    },
                    crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: "+  primary: \"#0f172a\",".to_string(),
                    },
                    crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: "+};".to_string(),
                    },
                ],
            }],
        };

        let lines = patch_lines(
            &file,
            100,
            &theme,
            false,
            true,
            None,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &mut None,
        );
        let indented_code_line = &lines[2];
        let default_added_fg = new_file_added_row_style(&theme).fg;
        let has_syntax_colored_token = indented_code_line.spans.iter().any(|span| {
            !span.content.trim().is_empty()
                && span.content.contains("primary")
                && span.style.fg != default_added_fg
        });

        assert!(
            has_syntax_colored_token,
            "expected indented JavaScript property to keep syntax coloring in new-file diff rows"
        );
    }

    #[test]
    fn new_file_patch_lines_preserve_syntax_coloring_for_typescript() {
        if highlight::HighlightLanguage::TypeScript.install_state()
            != highlight::HighlightInstallState::Installed
        {
            return;
        }

        crate::highlight::reload_runtime_state();

        let theme = Theme::default();
        let file = DiffFile {
            old_path: None,
            path: "docs/syntax-tests/syntax-test-highlight.ts".to_string(),
            status: DiffFileStatus::Added,
            additions: 74,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: Some(
                include_str!("../../../docs/syntax-tests/syntax-test-highlight.ts").to_string(),
            ),
            patch: "\
diff --git a/docs/syntax-tests/syntax-test-highlight.ts b/docs/syntax-tests/syntax-test-highlight.ts
new file mode 100644
index 0000000..1111111
--- /dev/null
+++ b/docs/syntax-tests/syntax-test-highlight.ts
@@ -0,0 +1,74 @@
"
            .to_string(),
            hunks: vec![crate::diff::DiffHunk {
                header: "@@ -0,0 +1,74 @@".to_string(),
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: 74,
                lines: include_str!("../../../docs/syntax-tests/syntax-test-highlight.ts")
                    .lines()
                    .map(|line| crate::diff::DiffLine {
                        kind: crate::diff::DiffLineKind::Added,
                        text: format!("+{line}"),
                    })
                    .collect(),
            }],
        };

        let lines = patch_lines(
            &file,
            120,
            &theme,
            false,
            true,
            None,
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &std::collections::HashSet::new(),
            &mut None,
        );
        let has_syntax_colored_token =
            lines.iter().flat_map(|line| line.spans.iter()).any(|span| {
                !span.content.trim().is_empty()
                    && span.content.contains("JsonPrimitive")
                    && span.style.fg != new_file_added_row_style(&theme).fg
            });

        assert!(
            has_syntax_colored_token,
            "expected TypeScript diff rows to keep syntax coloring in new-file diff rows"
        );
    }

    fn single_added_line_review_state() -> (DiffViewerState, DiffLineLocation) {
        let mut state = DiffViewerState::new(
            crate::app::ViewState::new(
                "proj".into(),
                "feat".into(),
                "sess".into(),
                "claude".into(),
                "Claude".into(),
                crate::project::SessionKind::Claude,
                crate::project::VibeMode::Vibeless,
                true,
            ),
            std::path::PathBuf::from("/tmp"),
        );
        state.review = true;
        let file = DiffFile {
            old_path: None,
            path: "a.rs".to_string(),
            status: DiffFileStatus::Added,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: Some("x\n".to_string()),
            patch: String::new(),
            hunks: vec![crate::diff::DiffHunk {
                header: "@@ -0,0 +1,1 @@".to_string(),
                old_start: 0,
                old_lines: 0,
                new_start: 1,
                new_lines: 1,
                lines: vec![crate::diff::DiffLine {
                    kind: crate::diff::DiffLineKind::Added,
                    text: "+x".to_string(),
                }],
            }],
        };
        let loc = file.addressable_lines()[0];
        state.files = vec![file];
        state.selected_file = 0;
        (state, loc)
    }

    #[test]
    fn review_history_modal_renders_current_state_and_lazy_archive_tail() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state
            .decisions
            .insert("a.rs".to_string(), ReviewDecision::Approve);
        state.review_history = Some(crate::app::ReviewHistoryState {
            rounds: vec![crate::app::ReviewHistoryRound {
                title: "Review — r1".to_string(),
                markdown: "## Review — r1\n\n**Approved:** 1\n".to_string(),
                carried_unresolved: 0,
            }],
            selected: 0,
            scroll: 0,
            rendered_lines: 0,
            view_height: 0,
            archive_available: true,
            archive_loaded: false,
            error: None,
        });
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("Review Timeline"));
        assert!(rendered.contains("Current"));
        assert!(rendered.contains("Last review"));
        assert!(rendered.contains("Older"));
        assert!(rendered.contains("Current Review"));
        assert!(rendered.contains("Approved: 1"));
        assert!(rendered.contains("press Enter to return to editing"));
    }

    #[test]
    fn help_modal_renders_grouped_keys_and_records_its_scroll_extent() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.help_open = true;

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
            .unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();

        assert!(rendered.contains("Final Review"));
        assert!(rendered.contains("Verdicts"));
        assert!(rendered.contains("Approve the current file"));
        assert!(rendered.contains("Line cursor"));

        // The overlay is taller than its viewport, and the renderer has to
        // report both so `G` can clamp to the real bottom.
        assert!(state.help_rendered_lines > state.help_view_height);
        assert!(state.help_view_height > 0);
    }

    /// Structural guard on the help table: a section that renders as a bare
    /// heading, or a continuation line with nothing above it to continue, is a
    /// rendering bug that only shows up on screen.
    #[test]
    fn help_sections_are_non_empty_and_uniquely_titled() {
        let mut titles = std::collections::HashSet::new();
        for (title, binds) in REVIEW_HELP_SECTIONS {
            assert!(titles.insert(*title), "duplicate help section: {title}");
            assert!(!binds.is_empty(), "empty help section: {title}");
            // A blank key column is a continuation line, so it must follow a
            // real binding rather than lead a section.
            assert!(
                !binds[0].0.is_empty(),
                "section {title} starts with a continuation line"
            );
            for (_, desc) in *binds {
                assert!(!desc.is_empty(), "empty help description in {title}");
            }
        }
    }

    #[test]
    fn review_footer_always_advertises_the_help_key() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();

        // Both footer shapes — the cursor-off hints and the cursor-mode hints —
        // point at `?`, since it's the only way to see the keys they omit. The
        // narrow width is the case that matters: the dense key row wraps into
        // both footer rows there, so a hint on the wrong line is clipped away.
        for width in [200u16, 90] {
            for cursor in [None, Some(0)] {
                state.comment_cursor = cursor;
                let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
                terminal
                    .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
                    .unwrap();
                let rendered = terminal
                    .backend()
                    .buffer()
                    .content()
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect::<String>();
                assert!(
                    rendered.contains("? keys"),
                    "footer missing the help hint (width: {width}, cursor: {cursor:?})"
                );
            }
        }
    }

    /// The whole screen as one whitespace-normalized string. Rows are joined and
    /// the panel border is blanked out, so a hint the wrapper split across two
    /// rows still reads as the phrase it is — which a raw cell-by-cell
    /// concatenation (trailing padding and border glyphs and all) does not.
    fn normalized_screen(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        let area = *buffer.area();
        (0..area.height)
            .map(|y| {
                (0..area.width)
                    .map(|x| match buffer[(x, y)].symbol() {
                        "│" | "┃" | "║" => " ",
                        symbol => symbol,
                    })
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join(" ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The round-level keys live on the footer's *second* row, and the first row
    /// is dense enough to wrap into two rows on its own at an ordinary terminal
    /// width — which used to silently swallow the second row whole. The footer
    /// now grows to fit and renders each row into its own area, so the width has
    /// to stop mattering.
    #[test]
    fn review_footer_second_row_survives_every_terminal_width() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, loc) = single_added_line_review_state();
        // A realistic mid-review footer: every conditional hint on the first row
        // is showing, which is exactly when it wraps.
        state.general_feedback = "looks close".to_string();
        state.verdict_undo.push(crate::app::VerdictUndo {
            path: "a.rs".to_string(),
            previous: None,
            previous_auto_rejected: false,
        });
        state.changed_since_last.insert("a.rs".to_string());

        for width in [200u16, 160, 120, 100, 80] {
            for cursor in [None, Some(0)] {
                state.comment_cursor = cursor;
                let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
                terminal
                    .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
                    .unwrap();
                let rendered = normalized_screen(&terminal);

                // The first row's pointer to the full key list, and hints that
                // only ever appear on the second row.
                assert!(
                    rendered.contains("? keys"),
                    "footer dropped the help hint (width: {width}, cursor: {cursor:?})"
                );
                if cursor.is_none() {
                    for hint in ["b base ref", "F filter", "target: live", "Esc pause"] {
                        assert!(
                            rendered.contains(hint),
                            "footer dropped {hint:?} (width: {width})"
                        );
                    }
                } else {
                    for hint in ["c/Esc exit cursor", "R resolve/reopen"] {
                        assert!(
                            rendered.contains(hint),
                            "cursor footer dropped {hint:?} (width: {width})"
                        );
                    }
                }
            }
        }

        // The peek box must not squeeze the hints out either: with the cursor on
        // a commented line the footer hosts both.
        state.comment_cursor = Some(0);
        state.line_comments.insert(
            "a.rs".to_string(),
            vec![crate::app::LineComment {
                location: loc,
                start: None,
                text: "needs a guard".to_string(),
                draft: false,
                suggestion: None,
                severity: crate::app::Severity::default(),
                anchor_context: None,
                start_anchor_context: None,
                anchor_lost: false,
                resolved: false,
                carried: false,
            }],
        );
        for width in [200u16, 120, 80] {
            let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
            terminal
                .draw(|frame| draw_diff_viewer(frame, &mut state, &Theme::default()))
                .unwrap();
            let rendered = normalized_screen(&terminal);
            assert!(
                rendered.contains("needs a guard"),
                "peek box lost its body (width: {width})"
            );
            assert!(
                rendered.contains("c/Esc exit cursor"),
                "peek box crowded out the hint rows (width: {width})"
            );
        }
    }

    /// The wrapper breaks on word boundaries, so the height has to be measured
    /// the same way — a `ceil(total / width)` estimate undercounts and puts the
    /// footer right back to clipping a row.
    #[test]
    fn wrapped_line_height_counts_word_wrapping_not_raw_length() {
        let line = Line::from("aaaa bbbb cccc");
        assert_eq!(wrapped_line_height(&line, 14), 1);
        // "aaaa bbbb" fits in 10; "cccc" moves down whole rather than splitting
        // at the 10th column the way a raw division would assume.
        assert_eq!(wrapped_line_height(&line, 10), 2);
        assert_eq!(wrapped_line_height(&line, 5), 3);
        // A word wider than the row is hard-broken instead of overflowing.
        assert_eq!(wrapped_line_height(&Line::from("aaaaaaaaa"), 3), 3);
        // Degenerate widths must still report a drawable row.
        assert_eq!(wrapped_line_height(&Line::from(""), 20), 1);
        assert_eq!(wrapped_line_height(&line, 0), 1);
    }

    #[test]
    fn cursor_comment_preview_surfaces_only_when_cursor_lands_on_a_comment() {
        let (mut state, loc) = single_added_line_review_state();

        // No cursor → nothing to peek.
        assert!(cursor_comment_text(&state).is_none());
        assert_eq!(cursor_comment_preview_rows(&state), 0);

        // Cursor on an un-commented line → still nothing.
        state.comment_cursor = Some(0);
        assert!(cursor_comment_text(&state).is_none());
        assert_eq!(cursor_comment_preview_rows(&state), 0);

        // A comment on the cursored line surfaces, sized to its body + border.
        state.line_comments.insert(
            "a.rs".to_string(),
            vec![crate::app::LineComment {
                location: loc,
                start: None,
                text: "needs a guard\nfor None".to_string(),
                draft: false,
                suggestion: None,
                severity: crate::app::Severity::default(),
                anchor_context: None,
                start_anchor_context: None,
                anchor_lost: false,
                resolved: false,
                carried: false,
            }],
        );
        assert_eq!(cursor_comment_text(&state), Some("needs a guard\nfor None"));
        // 2 body lines + the severity header + 2 border rows.
        assert_eq!(cursor_comment_preview_rows(&state), 5);
    }

    #[test]
    fn cursor_comment_preview_caps_long_bodies() {
        let (mut state, loc) = single_added_line_review_state();
        state.comment_cursor = Some(0);
        let body = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        state.line_comments.insert(
            "a.rs".to_string(),
            vec![crate::app::LineComment {
                location: loc,
                start: None,
                text: body,
                draft: false,
                suggestion: None,
                severity: crate::app::Severity::default(),
                anchor_context: None,
                start_anchor_context: None,
                anchor_lost: false,
                resolved: false,
                carried: false,
            }],
        );
        // 20 body lines (+ severity header) clamp to 6 visible + 2 border rows.
        assert_eq!(cursor_comment_preview_rows(&state), 8);
    }

    #[test]
    fn editor_hint_hides_for_a_deleted_file_in_both_footers() {
        use ratatui::{Terminal, backend::TestBackend};

        fn footer_text(state: &mut DiffViewerState) -> String {
            let backend = TestBackend::new(200, 40);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw_diff_viewer(frame, state, &Theme::default()))
                .unwrap();
            terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        }

        let (mut state, _) = single_added_line_review_state();
        // A live file advertises `E` whether or not the line cursor is up.
        assert!(footer_text(&mut state).contains("$EDITOR"));
        state.comment_cursor = Some(0);
        assert!(footer_text(&mut state).contains("$EDITOR"));

        // A deletion keeps its removed lines addressable, so the cursor can
        // still sit on one — but there is nothing on disk for `E` to open.
        state.files[0].status = DiffFileStatus::Deleted;
        state.files[0].hunks[0].lines[0] = crate::diff::DiffLine {
            kind: crate::diff::DiffLineKind::Removed,
            text: "-x".to_string(),
        };
        assert!(!state.files[0].addressable_lines().is_empty());
        assert!(!footer_text(&mut state).contains("$EDITOR"));
        state.comment_cursor = None;
        assert!(!footer_text(&mut state).contains("$EDITOR"));

        // Same for a binary blob, which has nothing an editor can show.
        state.files[0].status = DiffFileStatus::Modified;
        state.files[0].is_binary = true;
        assert!(!footer_text(&mut state).contains("$EDITOR"));
        state.comment_cursor = Some(0);
        assert!(!footer_text(&mut state).contains("$EDITOR"));
    }

    #[test]
    fn file_risk_marker_flags_large_no_note_and_no_tests() {
        let (mut state, _) = single_added_line_review_state();
        let theme = Theme::default();
        // Make the single existing file large and give it no note, so it picks
        // up all three flags (the changeset also has no test-looking file).
        state.files[0].additions = 500;
        let changeset_has_tests = changeset_has_test_changes(&state.files);
        assert!(!changeset_has_tests);
        let marker = file_risk_marker(&state.files[0], &state, changeset_has_tests, &theme)
            .expect("expected a risk marker");
        assert_eq!(marker.content.as_ref(), " [L,N,T]");

        // A small file with a developer note, in a changeset that now includes
        // a test file, gets no marker at all.
        state.files[0].additions = 1;
        state
            .review_notes
            .insert("a.rs".to_string(), "reasoning".to_string());
        state.files.push(DiffFile {
            old_path: None,
            path: "tests/a_test.rs".to_string(),
            status: DiffFileStatus::Added,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        });
        let changeset_has_tests = changeset_has_test_changes(&state.files);
        assert!(changeset_has_tests);
        assert!(file_risk_marker(&state.files[0], &state, changeset_has_tests, &theme).is_none());
    }
    #[test]
    fn file_list_renders_a_directory_tree_and_folds_it() {
        use ratatui::{Terminal, backend::TestBackend};

        let (mut state, _) = single_added_line_review_state();
        state.files = ["src/app/mod.rs", "src/app/state.rs", "src/ui/diff.rs"]
            .iter()
            .map(|path| DiffFile {
                old_path: Some((*path).to_string()),
                path: (*path).to_string(),
                status: DiffFileStatus::Modified,
                additions: 2,
                deletions: 1,
                is_binary: false,
                old_content: None,
                new_content: None,
                patch: String::new(),
                hunks: vec![],
            })
            .collect();
        state.selected_file = 0;

        // Only the file-list column: the patch panel beside it still titles
        // itself with the full path, which would mask what the rows show.
        const WIDTH: u16 = 120;
        const FILE_LIST_COLS: usize = 34;
        let render = |state: &mut DiffViewerState| {
            let backend = TestBackend::new(WIDTH, 30);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| draw_diff_viewer(frame, state, &Theme::default()))
                .unwrap();
            let cells: Vec<String> = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol().to_string())
                .collect();
            cells
                .chunks(WIDTH as usize)
                .map(|row| row[..FILE_LIST_COLS].concat())
                .collect::<Vec<String>>()
                .join("\n")
        };

        let rendered = render(&mut state);
        // Directory headers replace the repeated path prefixes, and file rows
        // show only their basename.
        assert!(rendered.contains("▾ src/"), "expected a src directory row");
        assert!(
            rendered.contains("  ▾ app/"),
            "expected a nested, indented app directory row: {rendered}"
        );
        assert!(rendered.contains("mod.rs"));
        assert!(
            !rendered.contains("src/app/mod.rs"),
            "file rows should drop the prefix their directory rows already show"
        );

        // Folding hides the files beneath and summarises what it swallowed.
        state.toggle_dir_collapsed("src/app");
        let folded = render(&mut state);
        assert!(!folded.contains("mod.rs"));
        assert!(folded.contains("▸ app/"));
        assert!(
            folded.contains("(2)"),
            "collapsed row should count its files"
        );
    }

    #[test]
    fn collapsed_dir_summary_only_counts_files_the_filter_shows() {
        let (mut state, _) = single_added_line_review_state();
        state.files = ["src/app/mod.rs", "src/app/state.rs", "src/app/ui.rs"]
            .iter()
            .map(|path| DiffFile {
                old_path: Some((*path).to_string()),
                path: (*path).to_string(),
                status: DiffFileStatus::Modified,
                additions: 2,
                deletions: 1,
                is_binary: false,
                old_content: None,
                new_content: None,
                patch: String::new(),
                hunks: vec![],
            })
            .collect();
        state.selected_file = 0;
        // Only mod.rs stays undecided; the other two are decided and one of them
        // also changed since the last round.
        state
            .decisions
            .insert("src/app/state.rs".to_string(), ReviewDecision::Approve);
        state.decisions.insert(
            "src/app/ui.rs".to_string(),
            ReviewDecision::Reject {
                feedback: "no".to_string(),
                severity: crate::app::Severity::default(),
            },
        );
        state
            .changed_since_last
            .insert("src/app/state.rs".to_string());
        state.file_filter = crate::app::FileFilter::Undecided;
        state.toggle_dir_collapsed("src/app");

        let rows = state.file_tree_rows();
        let files = rows
            .iter()
            .find_map(|row| match row {
                crate::app::FileTreeRow::Dir { path, files, .. } if path == "src/app" => {
                    Some(*files)
                }
                _ => None,
            })
            .expect("the collapsed src/app row");
        let visible = state.visible_file_indices();
        let summary: String =
            dir_row_summary("src/app", files, &visible, &state, &Theme::default())
                .iter()
                .map(|span| span.content.as_ref())
                .collect();

        // The badges describe the one file the row is actually hiding — not the
        // decisions and Δ of the two the filter dropped.
        assert_eq!(summary.trim(), "(1) ·1", "summary was {summary:?}");
    }
}
