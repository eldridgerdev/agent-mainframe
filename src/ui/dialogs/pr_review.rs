use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::{
    app::pr_review::{CommentKind, PrComment},
    app::{PrNumberPromptState, PrPickerState, PrReviewLoadState, PrReviewState},
    theme::Theme,
};

/// Modal prompt for a manual PR number, shown when the branch has no
/// auto-detectable open PR. Collects digits and surfaces resolve errors inline.
pub fn draw_pr_number_prompt(frame: &mut Frame, state: &PrNumberPromptState, theme: &Theme) {
    let area = super::super::dashboard::centered_rect(50, 25, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Review PR by number (experimental) ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // hint
            Constraint::Length(2), // input
            Constraint::Min(0),    // error
        ])
        .split(inner);

    let hint = Paragraph::new(Line::from(Span::styled(
        " No open PR detected for this branch — enter a number:",
        Style::default().fg(theme.text_muted.to_color()),
    )));
    frame.render_widget(hint, chunks[0]);

    let input = Paragraph::new(Line::from(vec![
        Span::styled(" PR #", Style::default().fg(theme.primary.to_color())),
        Span::styled(&state.input, Style::default().fg(theme.text.to_color())),
        Span::styled("\u{2588}", Style::default().fg(theme.primary.to_color())),
    ]));
    frame.render_widget(input, chunks[1]);

    if let Some(err) = &state.error {
        let error = Paragraph::new(Line::from(Span::styled(
            format!(" {err}"),
            Style::default().fg(theme.danger.to_color()),
        )))
        .wrap(Wrap { trim: false });
        frame.render_widget(error, chunks[2]);
    }
}

/// Full-screen PR picker: a scrollable list of the repo's PRs to open for
/// review. `⏎` opens the highlighted one, `a` toggles closed/merged, `#` drops
/// to the manual number prompt.
pub fn draw_pr_picker(frame: &mut Frame, state: &PrPickerState, theme: &Theme) {
    let area = frame.area();
    let block = pane_block(theme).title(" Pick a PR to review (experimental) ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let scope = if state.include_closed {
        "open + closed/merged"
    } else {
        "open"
    };
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(1),    // list
            Constraint::Length(1), // error
            Constraint::Length(1), // footer
        ])
        .split(inner);

    let header = Paragraph::new(Line::from(Span::styled(
        format!(" {} PR(s) · {scope}", state.entries.len()),
        Style::default().fg(theme.text_muted.to_color()),
    )));
    frame.render_widget(header, layout[0]);

    if state.entries.is_empty() {
        let empty = Paragraph::new(vec![
            Line::from(""),
            Line::from(Span::styled(
                "  No PRs to show.",
                Style::default().fg(theme.text.to_color()),
            )),
            Line::from(Span::styled(
                "  Press a to include closed/merged, or # to enter a number.",
                Style::default().fg(theme.text_muted.to_color()),
            )),
        ]);
        frame.render_widget(empty, layout[1]);
    } else {
        let items: Vec<ListItem> = state
            .entries
            .iter()
            .map(|entry| ListItem::new(pr_picker_row(entry, theme)))
            .collect();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .bg(theme.effective_selection_bg())
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
        let mut list_state = ListState::default();
        list_state.select(Some(state.selected.min(state.entries.len().saturating_sub(1))));
        frame.render_stateful_widget(list, layout[1], &mut list_state);
    }

    if let Some(err) = &state.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {err}"),
                Style::default().fg(theme.danger.to_color()),
            )))
            .wrap(Wrap { trim: false }),
            layout[2],
        );
    }

    let toggle = if state.include_closed {
        "a open-only"
    } else {
        "a include-closed"
    };
    let footer = Paragraph::new(Line::from(Span::styled(
        format!(" j/k move   \u{23ce} open   {toggle}   # number   esc close"),
        Style::default().fg(theme.text_muted.to_color()),
    )));
    frame.render_widget(footer, layout[3]);
}

/// One PR row: `#123  title  · @author · branch` plus a state chip for anything
/// that isn't a plain open PR (draft / merged / closed).
fn pr_picker_row(entry: &crate::github::PrListEntry, theme: &Theme) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            format!("#{} ", entry.number),
            Style::default().fg(theme.primary.to_color()),
        ),
        Span::styled(
            entry.title.clone(),
            Style::default().fg(theme.text.to_color()),
        ),
        Span::styled(
            format!("  · @{} · {}", entry.author, entry.head_ref),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ];
    if entry.is_draft {
        spans.push(chip("draft", theme.text_muted.to_color()));
    }
    match entry.state.as_str() {
        "MERGED" => spans.push(chip("merged", theme.info.to_color())),
        "CLOSED" => spans.push(chip("closed", theme.danger.to_color())),
        _ => {}
    }
    Line::from(spans)
}

/// Full-screen loading frame shown while a PR's comments are fetched.
pub fn draw_pr_review_loading(
    frame: &mut Frame,
    state: &PrReviewLoadState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = frame.area();
    let block = pane_block(theme).title(format!(" PR #{} (experimental) ", state.pr.number));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);

    let body = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            spinner,
            Span::styled(
                " Fetching PR comments (experimental)...",
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            format!("PR #{}  ·  {}", state.pr.number, state.pr.url),
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            "esc to cancel",
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ])
    .wrap(Wrap { trim: false });
    frame.render_widget(body, inner);
}

/// Full-screen PR comment-review pane: comment list on the left, detail on the
/// right.
pub fn draw_pr_review(frame: &mut Frame, state: &mut PrReviewState, theme: &Theme) {
    let area = frame.area();
    let review = &state.review;

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Min(1),    // body
            Constraint::Length(2), // footer (keys + marker legend)
        ])
        .split(area);

    // Header.
    let header = Line::from(vec![
        Span::styled(
            format!(" PR #{} (experimental) ", review.pr.number),
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(
                "{} comments ({} open)",
                review.comments.len(),
                review.open_count()
            ),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]);
    frame.render_widget(Paragraph::new(header), outer[0]);

    // Body: list | detail.
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(outer[1]);

    draw_comment_list(frame, body[0], state, theme);
    let detail_lines = draw_comment_detail(
        frame,
        body[1],
        state.selected_comment(),
        state.detail_scroll,
        theme,
    );
    // Record what the detail pane drew so the scroll handler can clamp against
    // the real content height (the layout is no longer a 1:1 source-line map).
    state.detail_content_lines = detail_lines;

    // Footer: key hints, then a legend spelling out the list markers.
    let toggle_hint = if state.hide_resolved {
        "h show-resolved"
    } else {
        "h hide-resolved"
    };
    let keys = Paragraph::new(Line::from(Span::styled(
        format!(
            " j/k move   f fix→{}   R reply-done   n not-needed   x resolve   t target   m done   s skip   {toggle_hint}   i syntax   r refresh   g other-PR   esc/q close",
            state.fix_target.tag()
        ),
        Style::default().fg(theme.text_muted.to_color()),
    )));
    let footer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(outer[2]);
    frame.render_widget(keys, footer[0]);
    frame.render_widget(Paragraph::new(marker_legend(theme)), footer[1]);

    // Harness picker overlays the pane on the first fix of a dedicated review.
    if let Some(pick) = &state.harness_pick {
        draw_harness_pick(frame, pick, theme);
    }
    // Fix confirm/edit dialog overlays the pane when open.
    if let Some(confirm) = &state.fix_confirm {
        draw_fix_confirm(frame, confirm, state.fix_target, theme);
    }
    // Reply dialog overlays the pane when open.
    if let Some(reply) = &state.reply {
        let author = state
            .review
            .comments
            .iter()
            .find(|c| c.id == reply.comment_id)
            .map(|c| c.author.as_str())
            .unwrap_or("reviewer");
        draw_reply_dialog(frame, reply, author, theme);
    }
}

/// Reply dialog: a contextual, editable reply (a "done in `<sha>`." report or a
/// "not needed" explanation) shown before it is posted. Posting happens only on
/// the user's explicit confirm.
fn draw_reply_dialog(
    frame: &mut Frame,
    reply: &crate::app::ReplyState,
    author: &str,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(70, 50, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(format!(" {} · @{author} ", reply.kind.title()))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // reply body
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    let body_lines = super::editor_view::editor_lines(&reply.editor, theme, "(type a reply)");
    frame.render_widget(
        Paragraph::new(body_lines).wrap(Wrap { trim: false }),
        chunks[0],
    );

    let hints = if reply.editing {
        "[esc] done editing"
    } else {
        "[⏎] post   [e] edit   [esc] cancel"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[1],
    );
}

/// Confirm/edit dialog: shows the exact prompt that will be injected (no file
/// contents — token principle #3) with a `~N tokens` preview, and lets the user
/// edit it before it reaches the agent.
fn draw_fix_confirm(
    frame: &mut Frame,
    confirm: &crate::app::FixConfirmState,
    target: crate::app::pr_review::FixTarget,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(70, 70, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Inject fix into agent session ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // target line
            Constraint::Length(1), // spacer
            Constraint::Min(1),    // prompt body / editor
            Constraint::Length(1), // token preview
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Will inject into the ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled(
                target.label(),
                Style::default()
                    .fg(theme.secondary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":", Style::default().fg(theme.text_muted.to_color())),
        ])),
        chunks[0],
    );

    let prompt_lines = super::editor_view::editor_lines(&confirm.editor, theme, "(empty prompt)");
    frame.render_widget(
        Paragraph::new(prompt_lines).wrap(Wrap { trim: false }),
        chunks[2],
    );

    let tokens = crate::app::pr_review::estimate_tokens(confirm.editor.text());
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("~{tokens} tokens · no file contents included"),
            Style::default().fg(theme.text_muted.to_color()),
        ))),
        chunks[3],
    );

    let hints = if confirm.editing {
        "[esc] done editing"
    } else {
        "[⏎] inject   [e] edit   [esc] cancel"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[4],
    );
}

/// Single-select harness picker for the dedicated review session, shown on the
/// first fix of a PR. The chosen harness is remembered for the rest of the PR
/// (the session is created once and reused).
fn draw_harness_pick(
    frame: &mut Frame,
    pick: &crate::app::HarnessPickState,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(50, 40, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Harness for the review session ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // header
            Constraint::Min(1),    // harness list
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  Run this PR's fixes on:",
            Style::default().fg(theme.text_muted.to_color()),
        )))
        .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let mut lines: Vec<Line> = Vec::new();
    for (i, agent) in pick.agents.iter().enumerate() {
        let is_selected = i == pick.selected;
        let marker = if is_selected { ">" } else { " " };
        let name_style = if is_selected {
            Style::default()
                .fg(theme.text.to_color())
                .bg(theme.effective_selection_bg())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text.to_color())
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {marker} "),
                Style::default().fg(theme.warning.to_color()),
            ),
            Span::styled(agent.display_name().to_string(), name_style),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), chunks[1]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[⏎] choose   [j/k] move   [esc] cancel",
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[2],
    );
}

fn draw_comment_list(frame: &mut Frame, area: Rect, state: &PrReviewState, theme: &Theme) {
    let block = pane_block(theme).title(" Comments ");

    if state.review.comments.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "No comments on this PR.",
            Style::default().fg(theme.text_muted.to_color()),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let visible = state.visible_indices();
    if visible.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "All comments resolved (h to show).",
            Style::default().fg(theme.text_muted.to_color()),
        )))
        .block(block);
        frame.render_widget(empty, area);
        return;
    }

    let mut items: Vec<ListItem> = visible
        .iter()
        .map(|&i| ListItem::new(comment_list_line(&state.review.comments[i], theme)))
        .collect();

    let hidden = state.hidden_resolved_count();
    if hidden > 0 {
        items.push(ListItem::new(Line::from(Span::styled(
            format!("  ─ {hidden} resolved hidden (h to show) ─"),
            Style::default().fg(theme.text_muted.to_color()),
        ))));
    }

    let list = List::new(items).block(block).highlight_style(
        Style::default()
            .bg(theme.primary.to_color())
            .fg(theme.effective_bg())
            .add_modifier(Modifier::BOLD),
    );

    // The list renders only visible comments, so translate the absolute
    // selection index into its position within the visible slice.
    let highlight = visible.iter().position(|&i| i == state.selected);
    let mut list_state = ListState::default();
    list_state.select(highlight);
    frame.render_stateful_widget(list, area, &mut list_state);
}

/// One row in the comment list: a local-triage checkbox, a resolution marker,
/// location, author, snippet.
fn comment_list_line<'a>(c: &'a PrComment, theme: &Theme) -> Line<'a> {
    let marker = if c.is_resolved { "✓" } else { " " };
    let location = match (&c.path, c.line) {
        (Some(path), Some(line)) => format!("{path}:{line}"),
        (Some(path), None) => path.clone(),
        (None, _) => kind_label(&c.kind).to_string(),
    };

    let location_style = if c.is_resolved {
        Style::default().fg(theme.text_muted.to_color())
    } else {
        Style::default().fg(theme.text.to_color())
    };

    Line::from(vec![
        Span::styled(
            format!("[{}] ", c.triage.marker()),
            Style::default().fg(triage_color(c.triage, theme)),
        ),
        Span::styled(
            format!("{marker} "),
            Style::default().fg(theme.success.to_color()),
        ),
        Span::styled(location, location_style),
        Span::styled(
            format!("  @{}", c.author),
            Style::default().fg(theme.secondary.to_color()),
        ),
        Span::styled(
            format!("  {}", c.snippet),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ])
}

/// Render the detail pane for the selected comment and return the number of
/// content lines built (used by the caller to clamp detail scrolling). The
/// detail is laid out as distinct sections — a chip-laden header, the diff hunk
/// (colored by add/remove/context), the Markdown-rendered body, and any local
/// triage note — separated by subtle dividers.
fn draw_comment_detail(
    frame: &mut Frame,
    area: Rect,
    comment: Option<&PrComment>,
    scroll: usize,
    theme: &Theme,
) -> usize {
    // The detail pane is the unfocused side (the list takes key input), so give
    // it a muted border to keep focus visually on the list.
    let block = pane_block(theme)
        .border_style(Style::default().fg(theme.text_muted.to_color()))
        .title(" Detail ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let Some(c) = comment else {
        return 0;
    };

    let width = inner.width.max(1) as usize;
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Header: location, then resolution/outdated/triage as compact chips.
    let mut header_spans = vec![Span::styled(
        match (&c.path, c.line) {
            (Some(path), Some(line)) => format!("{path}:{line}"),
            (Some(path), None) => path.clone(),
            (None, _) => kind_label(&c.kind).to_string(),
        },
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )];
    if c.outdated {
        header_spans.push(chip("outdated", theme.warning.to_color()));
    }
    if c.is_resolved {
        header_spans.push(chip("✓ resolved", theme.success.to_color()));
    }
    if let Some(label) = c.triage.label() {
        header_spans.push(chip(label, triage_color(c.triage, theme)));
    }
    lines.push(Line::from(header_spans));

    // Author / role / kind chips.
    lines.push(Line::from(vec![
        chip(&format!("@{}", c.author), theme.secondary.to_color()),
        chip(
            if c.is_bot { "bot" } else { "human" },
            theme.text_muted.to_color(),
        ),
        chip(kind_label(&c.kind), theme.text_muted.to_color()),
    ]));

    // Diff hunk, colored like a diff (add/remove/context/hunk-header).
    if let Some(hunk) = &c.diff_hunk {
        lines.push(divider(width, theme));
        lines.push(section_label("Diff hunk", theme));
        // When the hunk's language is recognized but its parser isn't installed,
        // the highlighter silently falls back to plain marker coloring. Surface
        // the `i` affordance so the user can install it without guessing.
        if let Some(hint) = syntax_install_hint(c.path.as_deref()) {
            lines.push(Line::from(Span::styled(
                hint,
                Style::default().fg(theme.warning.to_color()),
            )));
        }
        lines.extend(diff_hunk_lines(hunk, c.path.as_deref(), theme));
    }

    // Body, rendered as Markdown (reuses the in-app renderer).
    lines.push(divider(width, theme));
    if c.body.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            "(no body)",
            Style::default().fg(theme.text_muted.to_color()),
        )));
    } else {
        lines.extend(crate::markdown::render_markdown(&c.body, theme, width, None).lines);
    }

    // Local triage note (skip reason / "not needed" explanation), if any.
    if let Some(note) = c.local_note.as_ref().filter(|n| !n.trim().is_empty()) {
        lines.push(divider(width, theme));
        lines.push(section_label("Note", theme));
        for nl in note.lines() {
            lines.push(Line::from(Span::styled(
                nl.to_string(),
                Style::default().fg(theme.text.to_color()),
            )));
        }
    }

    let count = lines.len();
    let body = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll as u16, 0));
    frame.render_widget(body, inner);
    count
}

/// A compact `[label]` chip in the given accent color, with a leading space so
/// chips read as a spaced row.
fn chip(label: &str, color: ratatui::style::Color) -> Span<'static> {
    Span::styled(format!(" [{label}]"), Style::default().fg(color))
}

/// A full-width horizontal divider line in a muted color.
fn divider(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.text_muted.to_color()),
    ))
}

/// A small muted section label inside the detail pane.
fn section_label(label: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        label.to_string(),
        Style::default()
            .fg(theme.text_muted.to_color())
            .add_modifier(Modifier::BOLD),
    ))
}

/// Render a comment's `diff_hunk` as colored lines. The leading marker keeps
/// its diff color (added `+` green, removed `-` red, `@@` headers, muted
/// context), and the code after the marker is syntax-highlighted via the shared
/// tree-sitter highlighter, keyed off the comment's file path for language
/// detection.
///
/// The added/removed lines are highlighted against reconstructed "new" and
/// "old" sides (markers stripped) so the parser sees real multi-line source for
/// context, then each hunk line is matched back to its highlighted line. When
/// no language is detected (e.g. a comment with no file path) or a parser isn't
/// available, this degrades to the plain marker coloring.
fn diff_hunk_lines(hunk: &str, path: Option<&str>, theme: &Theme) -> Vec<Line<'static>> {
    // Reconstruct the two sides so the highlighter parses contiguous source.
    let mut new_src = String::new();
    let mut old_src = String::new();
    for raw in hunk.lines() {
        if raw.starts_with("@@") {
            continue;
        }
        match raw.as_bytes().first() {
            Some(b'+') => {
                new_src.push_str(&raw[1..]);
                new_src.push('\n');
            }
            Some(b'-') => {
                old_src.push_str(&raw[1..]);
                old_src.push('\n');
            }
            _ => {
                let content = raw.strip_prefix(' ').unwrap_or(raw);
                new_src.push_str(content);
                new_src.push('\n');
                old_src.push_str(content);
                old_src.push('\n');
            }
        }
    }

    let p = path.map(std::path::Path::new);
    let new_hl = crate::highlight::highlight_source(crate::highlight::HighlightRequest {
        path: p,
        language_hint: None,
        source: &new_src,
    });
    let old_hl = crate::highlight::highlight_source(crate::highlight::HighlightRequest {
        path: p,
        language_hint: None,
        source: &old_src,
    });

    let mut lines = Vec::new();
    let mut new_idx = 0usize;
    let mut old_idx = 0usize;
    for raw in hunk.lines() {
        if raw.starts_with("@@") {
            lines.push(Line::from(Span::styled(
                raw.to_string(),
                Style::default().fg(theme.secondary.to_color()),
            )));
            continue;
        }

        let (marker, color, content, hl_line) = match raw.as_bytes().first() {
            Some(b'+') => {
                let hl = new_hl.lines.get(new_idx);
                new_idx += 1;
                ("+", theme.success.to_color(), &raw[1..], hl)
            }
            Some(b'-') => {
                let hl = old_hl.lines.get(old_idx);
                old_idx += 1;
                ("-", theme.danger.to_color(), &raw[1..], hl)
            }
            _ => {
                let hl = new_hl.lines.get(new_idx);
                new_idx += 1;
                old_idx += 1;
                let marker = if raw.starts_with(' ') { " " } else { "" };
                let content = raw.strip_prefix(' ').unwrap_or(raw);
                (marker, theme.text_muted.to_color(), content, hl)
            }
        };

        let mut spans = Vec::new();
        if !marker.is_empty() {
            spans.push(Span::styled(marker.to_string(), Style::default().fg(color)));
        }
        spans.extend(highlight_content_spans(
            content,
            hl_line,
            Style::default().fg(color),
            theme,
        ));
        lines.push(Line::from(spans));
    }
    lines
}

/// Map highlighted spans for one code line onto `content`, producing styled
/// spans. `base` is the diff color for the line; real syntax tokens override
/// the foreground while `Plain` tokens (and any uncovered remainder) keep the
/// diff color, so the add/remove signal survives even when highlighting is
/// sparse or unavailable.
fn highlight_content_spans(
    content: &str,
    hl: Option<&crate::highlight::HighlightedLine>,
    base: Style,
    theme: &Theme,
) -> Vec<Span<'static>> {
    let Some(hl) = hl.filter(|h| !h.spans.is_empty()) else {
        return vec![Span::styled(content.to_string(), base)];
    };

    let mut spans = Vec::new();
    let mut remaining = content;
    let mut rendered_any = false;
    for sp in &hl.spans {
        if remaining.is_empty() {
            break;
        }
        if sp.text.is_empty() {
            continue;
        }
        let n = shared_prefix_len(remaining, &sp.text);
        if n == 0 {
            continue;
        }
        let (head, tail) = remaining.split_at(n);
        let style = if sp.class == crate::highlight::SyntaxClass::Plain {
            base
        } else {
            base.patch(crate::highlight::style_for_class(sp.class, theme))
        };
        spans.push(Span::styled(head.to_string(), style));
        remaining = tail;
        rendered_any = true;
    }

    if !remaining.is_empty() {
        spans.push(Span::styled(remaining.to_string(), base));
    } else if !rendered_any {
        spans.push(Span::styled(content.to_string(), base));
    }
    spans
}

/// Length (in bytes) of the shared leading run of characters between two
/// strings, used to align rendered content with highlighter span text.
fn shared_prefix_len(content: &str, other: &str) -> usize {
    let mut end = 0;
    for (a, b) in content.chars().zip(other.chars()) {
        if a != b {
            break;
        }
        end += a.len_utf8();
    }
    end
}

/// Footer legend spelling out the list/detail markers.
fn marker_legend(theme: &Theme) -> Line<'static> {
    let muted = Style::default().fg(theme.text_muted.to_color());
    Line::from(vec![
        Span::styled(" ✓ resolved", Style::default().fg(theme.success.to_color())),
        Span::styled("   [outdated] line moved", Style::default().fg(theme.warning.to_color())),
        Span::styled("   bot/human", muted),
        Span::styled("   triage: ", muted),
        Span::styled("[ ] untriaged ", muted),
        Span::styled("[~] fixing ", Style::default().fg(theme.warning.to_color())),
        Span::styled("[x] done ", Style::default().fg(theme.success.to_color())),
        Span::styled("[-] skip", muted),
    ])
}

/// Accent color for a triage state's checkbox/chip.
fn triage_color(state: crate::app::pr_review::TriageState, theme: &Theme) -> ratatui::style::Color {
    use crate::app::pr_review::TriageState;
    match state {
        TriageState::Untriaged => theme.text_muted.to_color(),
        TriageState::Fixing => theme.warning.to_color(),
        TriageState::Done | TriageState::Replied => theme.success.to_color(),
        TriageState::Skipped => theme.text_muted.to_color(),
    }
}

/// A muted hint nudging the user toward `i` when the comment's file maps to a
/// known highlight language whose parser isn't installed yet. Returns `None` for
/// comments with no path, unrecognized languages, or already-installed parsers.
fn syntax_install_hint(path: Option<&str>) -> Option<String> {
    let path = path?;
    let (language, status) =
        crate::highlight::language_install_state_for_path(std::path::Path::new(path))?;
    matches!(status, crate::highlight::HighlightInstallState::Available).then(|| {
        format!(
            "{} highlighting not installed — press i",
            language.picker_title()
        )
    })
}

fn kind_label(kind: &CommentKind) -> &'static str {
    match kind {
        CommentKind::Inline => "inline comment",
        CommentKind::ReviewSummary { .. } => "review summary",
        CommentKind::Conversation => "conversation",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line_text(line: &Line) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn diff_hunk_lines_preserve_text_and_markers() {
        let theme = Theme::default();
        let hunk = "@@ -1,3 +1,3 @@\n fn main() {\n-    let x = 1;\n+    let x = 2;\n }";
        let lines = diff_hunk_lines(hunk, Some("src/main.rs"), &theme);

        // One rendered line per hunk line, with markers and (indented) content
        // preserved verbatim — regardless of whether highlighting is available.
        assert_eq!(lines.len(), 5);
        assert_eq!(line_text(&lines[0]), "@@ -1,3 +1,3 @@");
        assert_eq!(line_text(&lines[1]), " fn main() {");
        assert_eq!(line_text(&lines[2]), "-    let x = 1;");
        assert_eq!(line_text(&lines[3]), "+    let x = 2;");
        assert_eq!(line_text(&lines[4]), " }");
    }

    #[test]
    fn diff_hunk_lines_without_language_still_preserve_text() {
        let theme = Theme::default();
        // No path → no language detection → plain marker coloring, text intact.
        let hunk = "-old line\n+new line";
        let lines = diff_hunk_lines(hunk, None, &theme);
        assert_eq!(lines.len(), 2);
        assert_eq!(line_text(&lines[0]), "-old line");
        assert_eq!(line_text(&lines[1]), "+new line");
    }
}

fn pane_block(theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()))
}
