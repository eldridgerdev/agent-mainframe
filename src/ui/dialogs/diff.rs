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
    app::{DiffViewerFocus, DiffViewerLayout, DiffViewerState},
    diff::{DiffFile, DiffFileStatus, DiffLine, DiffLineKind, DiffLineLocation},
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
        } else {
            " Branch Diff "
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
            || state.editing_suggestion)
    {
        inner.height.saturating_sub(10).clamp(4, 12)
    } else if state.review {
        // Grow to host the line-comment peek box above the two-row hints when the
        // cursor is parked on a line that already carries a comment.
        2 + cursor_comment_preview_rows(state)
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

    let block = Block::default()
        .title(" Branch Diff ")
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

    let loading = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            spinner,
            Span::styled(
                " Loading branch diff...",
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("Comparing changes for {branch}"),
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

    let header = Paragraph::new(vec![
        Line::from(vec![
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
                Span::styled(
                    "  (manual)",
                    Style::default().fg(theme.warning.to_color()),
                )
            } else {
                Span::raw("")
            },
        ]),
        Line::from(second_line),
    ])
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
                " Could not load branch diff ",
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
                " No changes against the selected base ",
                Style::default()
                    .fg(theme.success.to_color())
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Refresh with r after making more edits or commits.",
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

    // Give the notes panel ~40% of the column, but always leave room for both.
    let notes_height = (content_area.height * 2 / 5).clamp(
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
    let path = state.files.get(state.selected_file).map(|file| file.path.clone());
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

fn draw_file_list(frame: &mut Frame, area: Rect, state: &DiffViewerState, theme: &Theme) {
    let visible = state.visible_file_indices();
    let mut items: Vec<ListItem<'static>> = visible
        .iter()
        .map(|&idx| {
            let file = &state.files[idx];
            let status_style = Style::default()
                .fg(status_color(&file.status, theme))
                .add_modifier(Modifier::BOLD);
            let mut spans = Vec::new();
            if state.review {
                let (symbol, color) = match state.decisions.get(&file.path) {
                    Some(crate::app::ReviewDecision::Approve) => ("✓", theme.success.to_color()),
                    Some(crate::app::ReviewDecision::Reject { .. }) => ("✗", theme.danger.to_color()),
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
            spans.push(Span::styled(
                file.path.clone(),
                Style::default().fg(theme.text.to_color()),
            ));
            spans.push(Span::styled(
                format!("  +{} -{}", file.additions, file.deletions),
                Style::default().fg(theme.text_muted.to_color()),
            ));
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
    // Highlight maps onto the visible subset; None when the selection is hidden
    // by the active filter.
    list_state.select(visible.iter().position(|&i| i == state.selected_file));
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
    let footer = Paragraph::new(diff_footer_lines(
        focus,
        layout,
        new_file_selected,
        syntax_status,
        theme,
    ))
    .wrap(Wrap { trim: false });
    frame.render_widget(footer, area);
}

fn draw_review_footer(frame: &mut Frame, area: Rect, state: &mut DiffViewerState, theme: &Theme) {
    let key = |k: &'static str| Span::styled(k, Style::default().fg(theme.warning.to_color()));

    if state.feedback_editing
        || state.editing_general
        || state.editing_line_comment
        || state.editing_suggestion
    {
        draw_feedback_editor(frame, area, state, theme);
        return;
    }

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
        frame.render_widget(
            Paragraph::new(vec![first, second]).wrap(Wrap { trim: false }),
            area,
        );
        return;
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
        let second = Line::from(vec![
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
            key("n"),
            Span::raw("/"),
            key("p"),
            Span::raw(" file  "),
            key("c"),
            Span::raw("/"),
            key("Esc"),
            Span::raw(" exit cursor  "),
            key("q"),
            Span::raw(" finish"),
        ]);

        // When the cursor sits on a commented line, peek the comment body in a
        // bordered box above the hints so the reviewer can read what they wrote
        // without re-opening the editor.
        if let Some(comment) = cursor_comment(state) {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(3), Constraint::Length(2)])
                .split(area);
            let title = match (comment.draft, comment.is_range()) {
                (true, true) => " AI draft on these lines (a accept · d dismiss · Enter edit) ",
                (true, false) => " AI draft on this line (a accept · d dismiss · Enter edit) ",
                (false, true) if comment.suggestion.is_some() => {
                    " suggestion on these lines (Enter comment · S suggest) "
                }
                (false, false) if comment.suggestion.is_some() => {
                    " suggestion on this line (Enter comment · S suggest) "
                }
                (false, true) => " comment on these lines (Enter to edit) ",
                (false, false) => " comment on this line (Enter to edit) ",
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
            frame.render_widget(
                Paragraph::new(vec![first, second]).wrap(Wrap { trim: false }),
                chunks[1],
            );
            return;
        }

        frame.render_widget(
            Paragraph::new(vec![first, second]).wrap(Wrap { trim: false }),
            area,
        );
        return;
    }

    let mut second_line = vec![
        key(" j"),
        Span::raw("/"),
        key("k"),
        Span::raw(" scroll  "),
    ];

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
        crate::app::pr_review::FixTarget::ExistingLive => {
            (" target: live  ", theme.text_muted.to_color())
        }
    };
    second_line.push(Span::styled(target_label, Style::default().fg(target_color)));
    second_line.push(key("q"));
    second_line.push(Span::raw(" finish review (writes feedback)"));

    let mut first_line = vec![
        key(" a"),
        Span::raw(" approve  "),
        key("r"),
        Span::raw(" reject  "),
        key("s"),
        Span::raw(" skip  "),
        key("f"),
        Span::raw(" general feedback  "),
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
    first_line.extend([
        Span::raw("  "),
        key("n"),
        Span::raw("/"),
        key("p"),
        Span::raw(" file  "),
        key("e"),
        Span::raw(if state.notes_expanded {
            " show diff  "
        } else {
            " expand notes  "
        }),
        key("Tab"),
        Span::raw(" focus  "),
        key("v"),
        Span::raw(" layout"),
    ]);

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

    let lines = vec![Line::from(first_line), Line::from(second_line)];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
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
    let carries_severity = state.editing_line_comment || state.feedback_editing;
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
    } else if state.editing_line_comment || state.editing_suggestion {
        let anchor = state
            .comment_cursor
            .and_then(|idx| {
                state
                    .files
                    .get(state.selected_file)
                    .and_then(|file| file.addressable_lines().get(idx).copied().map(|loc| {
                        match (loc.new_line, loc.old_line) {
                            (Some(new_line), _) => format!("{}:{new_line}", file.path),
                            (None, Some(old_line)) => format!("{}:{old_line} (base)", file.path),
                            (None, None) => file.path.clone(),
                        }
                    }))
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
    let editor_lines =
        super::editor_view::editor_lines(&state.feedback_editor, theme, placeholder);
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
        hint_spans.push(Span::raw(format!(" severity: [{}]  ", state.comment_severity.label())));
    }
    hint_spans.extend([
        key("Ctrl+T"),
        Span::raw(if vim.is_some() { " vim off  " } else { " vim on  " }),
        key("Ctrl+J/K"),
        Span::raw(" scroll"),
    ]);
    frame.render_widget(Paragraph::new(Line::from(hint_spans)), rows[1]);
}

fn diff_footer_lines(
    focus: &str,
    layout: &str,
    new_file_selected: bool,
    syntax_status: Option<(
        highlight::HighlightLanguage,
        highlight::HighlightInstallState,
    )>,
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
        Span::styled("PgUp/PgDn", Style::default().fg(theme.warning.to_color())),
        Span::raw(" patch  "),
        Span::styled("g/G", Style::default().fg(theme.warning.to_color())),
        Span::raw(" top/bottom  "),
        Span::styled("r", Style::default().fg(theme.warning.to_color())),
        Span::raw(" refresh  "),
        Span::styled("b", Style::default().fg(theme.warning.to_color())),
        Span::raw(" base ref"),
    ]);

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
        for diff_line in &hunk.lines {
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
                        diff_chunks(
                            &diff_line.text,
                            removed_row_style(theme),
                            theme,
                            highlighted_line(highlights.old.as_ref(), old_line),
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
                        diff_chunks(
                            &diff_line.text,
                            added_style,
                            theme,
                            highlighted_line(highlights.new.as_ref(), new_line),
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
    let left_wrapped = if left_number.is_none() && left.is_empty() {
        vec![plain_chunks(
            &hatch_fill(text_width, 0),
            hatched_side_style(right_style, theme),
        )]
    } else {
        wrap_chunks(
            &diff_chunks(&left, left_style, theme, left_highlight),
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
            &diff_chunks(&right, right_style, theme, right_highlight),
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
            Span::styled(sep_text.to_string(), Style::default().fg(sep_fg).bg(gutter_bg)),
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
        append_highlighted_content(&mut chunks, content, row_style, theme, highlighted_line);
    }

    chunks
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
            &theme,
        );

        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).contains("install tsx parser"));
        assert!(line_text(&lines[0]).contains("Esc close"));
        assert!(line_text(&lines[1]).contains("layout:unified (new file)"));
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
            }],
        );
        // 20 body lines (+ severity header) clamp to 6 visible + 2 border rows.
        assert_eq!(cursor_comment_preview_rows(&state), 8);
    }
}
