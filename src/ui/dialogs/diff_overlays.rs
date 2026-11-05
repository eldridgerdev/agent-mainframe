use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use super::{PatchPanelOptions, draw_patch_panel};
use crate::ui::dashboard::centered_rect;
use crate::{
    app::{DiffScope, DiffViewerLayout, DiffViewerState, ReviewDecision, SummaryItem},
    theme::Theme,
    ui::draw_modal_overlay,
};

/// The PR review's submit dialog: the event to post as, the summary, what
/// will be posted where, and — after an attempt — what happened.
pub(super) fn draw_pr_submit_modal(frame: &mut Frame, state: &DiffViewerState, theme: &Theme) {
    let (Some(submit), DiffScope::PullRequest(target)) = (&state.pr_submit, &state.scope) else {
        return;
    };
    let area = centered_rect(70, 70, frame.area());
    draw_modal_overlay(frame, area, theme);
    let block = Block::default()
        .title(format!(" Submit review — PR #{} ", target.pr.number))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let muted = Style::default().fg(theme.text_muted.to_color());
    let text = Style::default().fg(theme.text.to_color());
    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));
    let mut lines = vec![Line::from(Span::styled(" Post as:", muted))];
    for event in crate::app::PrReviewEvent::ALL {
        let chosen = event == submit.event;
        let unavailable =
            submit.own_pr == Some(true) && event != crate::app::PrReviewEvent::Comment;
        let marker = if chosen { "(•)" } else { "( )" };
        let style = if unavailable {
            muted
        } else if chosen {
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            text
        };
        let mut spans = vec![Span::styled(
            format!("   {marker} {}", event.label()),
            style,
        )];
        if unavailable {
            spans.push(Span::styled("  — not on your own PR", muted));
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" Summary:", muted)));
    let summary = state.general_feedback.trim();
    if summary.is_empty() {
        lines.push(Line::from(Span::styled(
            "   (none — e to write one)",
            muted,
        )));
    } else {
        for line in summary.lines().take(6) {
            lines.push(Line::from(Span::styled(format!("   {line}"), text)));
        }
        if summary.lines().count() > 6 {
            lines.push(Line::from(Span::styled("   …", muted)));
        }
    }

    let (inline, files, in_summary) = crate::app::review::submission_counts(state);
    lines.push(Line::from(""));
    let mut counts = format!(" {inline} inline comment(s) · {files} file comment(s)");
    if in_summary > 0 {
        counts.push_str(&format!(
            " · {in_summary} added to the summary (can't go on the diff)"
        ));
    }
    lines.push(Line::from(Span::styled(counts, text)));
    lines.push(Line::from(Span::styled(
        format!(
            " Pinned to {} {}",
            target.pr.branch_label(),
            target.pr.head_oid.get(..7).unwrap_or(&target.pr.head_oid)
        ),
        muted,
    )));

    lines.push(Line::from(""));
    let danger = Style::default().fg(theme.danger.to_color());
    let footer = match &submit.status {
        crate::app::PrSubmitStatus::Ready => vec![
            key(" ←/→"),
            Span::raw(" choose  "),
            key("e"),
            Span::raw(" edit summary  "),
            key("⏎"),
            Span::raw(" post  "),
            key("esc"),
            Span::raw(" cancel (draft kept)"),
        ],
        crate::app::PrSubmitStatus::Posting { .. } => {
            lines.push(Line::from(Span::styled(
                " Posting to GitHub…",
                Style::default().fg(theme.warning.to_color()),
            )));
            vec![Span::styled(" wait for GitHub's answer", muted)]
        }
        crate::app::PrSubmitStatus::Failed(err) => {
            lines.push(Line::from(Span::styled(
                format!(" Not posted: {err}"),
                danger,
            )));
            vec![
                key(" ⏎"),
                Span::raw(" retry  "),
                key("e"),
                Span::raw(" edit summary  "),
                key("esc"),
                Span::raw(" close (draft kept)"),
            ]
        }
        crate::app::PrSubmitStatus::HeadMoved { current_head } => {
            lines.push(Line::from(Span::styled(
                format!(
                    " Not posted: PR #{} has new commits ({} → {}). Posting now would pin \
                     your comments to code you haven't seen.",
                    target.pr.number,
                    target.pr.head_oid.get(..7).unwrap_or(&target.pr.head_oid),
                    current_head.get(..7).unwrap_or(current_head)
                ),
                danger,
            )));
            vec![
                key(" o"),
                Span::raw(" reopen at the new head (draft kept, changes flagged)  "),
                key("esc"),
                Span::raw(" close"),
            ]
        }
    };
    lines.push(Line::from(footer));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// The review key surface, grouped by what the reviewer is trying to do. This
/// is the `?` overlay's content: the two footer rows can only ever show the
/// keys that apply right now, so the full set needs somewhere to live.
///
/// An empty key column is a continuation line for the entry above it.
pub(super) const REVIEW_HELP_SECTIONS: &[(&str, &[(&str, &str)])] = &[
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
            (
                "Q",
                "Ask AI about this review with read-only repository discovery",
            ),
            ("Esc", "Cancel plain editing; Vim Insert enters Normal mode"),
        ],
    ),
    (
        "Line cursor (press c first)",
        &[
            ("j / k", "Move the cursor a line at a time"),
            ("Ctrl+J / K", "Move the cursor 10 lines at a time"),
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
            ("Ctrl+J / K", "Scroll the patch 10 lines at a time"),
            (
                "Mouse wheel",
                "Scroll the patch, or the open note / overlay",
            ),
            ("g / G", "Jump to the top / bottom of the focused panel"),
            ("Tab", "Move focus between the file list and the patch"),
            ("PgUp / PgDn", "Scroll the patch a screen at a time"),
            ("h / l", "Collapse-or-out / expand-or-in (file list)"),
            (
                "z / Z",
                "Fold the cursored directory / whole tree (file list)",
            ),
            (".", "Show / fold the files AMF generated (listed last)"),
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

/// The help overlay's sections for this review. A PR review leads with how
/// to submit it and leaves out what it refuses (the keys that need a local
/// feature, the AI passes, and finishing into an agent), so the overlay never
/// advertises a key that can only explain why it won't work.
pub(super) fn review_help_sections(
    pr_review: bool,
) -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    const PR_REVIEW_SECTION: (&str, &[(&str, &str)]) = (
        "Submitting (PR review)",
        &[
            ("q", "Submit to GitHub: Comment, Approve or Request changes"),
            ("", "(inline comments pinned to the reviewed commit)"),
            ("", "(dialog: ←/→ choose, e summary, ⏎ post, esc cancel)"),
            ("o", "After a blocked post: reopen the PR at its new head"),
            ("Esc", "Pause — leave with the draft saved"),
            ("", "(Enter on the PR in the Review tab resumes it)"),
        ],
    );
    const PR_HIDDEN_SECTIONS: &[&str] = &["Context and AI passes", "Finishing"];
    const PR_HIDDEN_KEYS: &[&str] = &["x", "E", "b"];

    if !pr_review {
        return REVIEW_HELP_SECTIONS
            .iter()
            .map(|(title, binds)| (*title, binds.to_vec()))
            .collect();
    }
    let mut sections = vec![(PR_REVIEW_SECTION.0, PR_REVIEW_SECTION.1.to_vec())];
    for (title, binds) in REVIEW_HELP_SECTIONS {
        if PR_HIDDEN_SECTIONS.contains(title) {
            continue;
        }
        let mut kept = Vec::new();
        let mut dropping = false;
        for &(key, desc) in *binds {
            // A continuation line belongs to the row above it.
            if key.is_empty() {
                if !dropping {
                    kept.push((key, desc));
                }
                continue;
            }
            dropping = PR_HIDDEN_KEYS.contains(&key);
            if !dropping {
                kept.push((key, desc));
            }
        }
        if !kept.is_empty() {
            sections.push((*title, kept));
        }
    }
    sections
}

/// A scrollable, read-only listing of every review-mode key (`?`). Takes full
/// key precedence while open (`handle_diff_viewer_key`), like the other review
/// modals. Groups mirror the dashboard help overlay's shape so the two read the
/// same way.
pub(super) fn draw_review_help_modal(
    frame: &mut Frame,
    state: &mut DiffViewerState,
    theme: &Theme,
) {
    let area = centered_rect(72, 84, frame.area());
    draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(if state.is_pr_review() {
            " PR Review — Keys "
        } else {
            " Final Review — Keys "
        })
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
    for (section, binds) in review_help_sections(state.is_pr_review()) {
        if !lines.is_empty() {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            format!("  {section}"),
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        )));
        for (key, desc) in binds {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {key:>14}"),
                    Style::default()
                        .fg(theme.warning.to_color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(desc, Style::default().fg(theme.text.to_color())),
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
pub(super) fn draw_review_history_modal(
    frame: &mut Frame,
    state: &mut DiffViewerState,
    theme: &Theme,
) {
    let area = centered_rect(88, 86, frame.area());
    draw_modal_overlay(frame, area, theme);

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
    let error = history.error_message();
    let historical = selected > 0;
    let body = if selected == 0 {
        crate::app::review::current_review_history_markdown(state)
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

/// A centered modal showing the "since last review" diff for the current
/// file (`I` in the final review): the diff between its content when the
/// last review round finished and its content now. Read-only, and takes full
/// key precedence while open, mirroring `draw_changeset_overview_modal`.
/// Reuses `draw_patch_panel` — the same unified-diff renderer as the main
/// patch panel — against the on-demand `DiffFile` computed by
/// `App::open_interdiff`, so it needs none of the comment/cursor plumbing.
pub(super) fn draw_interdiff_modal(frame: &mut Frame, state: &DiffViewerState, theme: &Theme) {
    let area = centered_rect(90, 80, frame.area());
    draw_modal_overlay(frame, area, theme);

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
pub(super) fn draw_changeset_overview_modal(
    frame: &mut Frame,
    state: &mut DiffViewerState,
    theme: &Theme,
) {
    let area = centered_rect(80, 70, frame.area());
    draw_modal_overlay(frame, area, theme);

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
pub(super) fn draw_review_summary_modal(frame: &mut Frame, state: &DiffViewerState, theme: &Theme) {
    let area = centered_rect(84, 82, frame.area());
    draw_modal_overlay(frame, area, theme);

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
pub(super) fn draw_search_prompt(frame: &mut Frame, state: &DiffViewerState, theme: &Theme) {
    let area = centered_rect(60, 22, frame.area());
    draw_modal_overlay(frame, area, theme);

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
pub(super) fn draw_base_ref_prompt(frame: &mut Frame, state: &DiffViewerState, theme: &Theme) {
    let area = centered_rect(60, 22, frame.area());
    draw_modal_overlay(frame, area, theme);

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
