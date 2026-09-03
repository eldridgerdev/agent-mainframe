use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use unicode_width::UnicodeWidthStr;

use std::path::Path;

use crate::{
    app::ai_review::AiReviewTriageStatus,
    app::pr_review::{
        BootstrapDepth, BootstrapStage, CommentKind, CompactStage, MarkAction, PrComment, ReplyKind,
    },
    app::{
        BootstrapPickState, BootstrapRunState, CompactConfirmState, CompactReviewState,
        CompactRunState, InvestigationAction, InvestigationActionPick, InvestigationFollowUpDraft,
        InvestigationHarnessPick, MarkPickState, PrInvestigationLoadState, PrNumberPromptState,
        PrPickerState, PrReviewLoadState, PrReviewState, ReplyKindPickState,
    },
    editor::VimMode,
    theme::Theme,
    token_tracking::{
        SessionTokenUsage, TokenPricingConfig, format_feature_token_usage,
        format_token_usage_summary,
    },
};

/// Modal prompt for a manual PR number, shown when the branch has no
/// auto-detectable open PR. Collects digits and surfaces resolve errors inline.
pub fn draw_pr_number_prompt(frame: &mut Frame, state: &PrNumberPromptState, theme: &Theme) {
    let area = super::super::dashboard::centered_rect(50, 25, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" PR Triage by number (experimental) ")
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
/// triage. `⏎` opens the highlighted one, `a` toggles closed/merged, `#` drops
/// to the manual number prompt, `b` opens the review-memory lookback
/// bootstrap. `memory_paths` holds both resolved review-memory doc paths; the
/// bootstrap depth picker and the compact overlay each show whichever their own
/// `g` scope toggle points at.
pub fn draw_pr_picker(
    frame: &mut Frame,
    state: &PrPickerState,
    theme: &Theme,
    memory_paths: &crate::app::review_memory::ReviewMemoryPaths,
) {
    let area = frame.area();
    let block = pane_block(theme).title(" Pick a PR to triage (experimental) ");
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
            .map(|entry| ListItem::new(pr_picker_row(entry, state.current_user.as_deref(), theme)))
            .collect();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .bg(theme.effective_selection_bg())
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("> ");
        let mut list_state = ListState::default();
        list_state.select(Some(
            state.selected.min(state.entries.len().saturating_sub(1)),
        ));
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
        format!(
            " j/k move   \u{23ce} open   {toggle}   # number   b bootstrap memory   c compact memory   esc close"
        ),
        Style::default().fg(theme.text_muted.to_color()),
    )));
    frame.render_widget(footer, layout[3]);

    if let Some(pick) = &state.bootstrap_pick {
        draw_bootstrap_pick(frame, pick, memory_paths.for_scope(pick.scope), theme);
    }
    if let Some(confirm) = &state.compact_confirm {
        draw_compact_confirm(frame, confirm, memory_paths.for_scope(confirm.scope), theme);
    }
}

/// Depth picker for the review-memory lookback bootstrap (`b` in the PR
/// picker): a radio list of how far back to look, overlaid on the picker.
fn draw_bootstrap_pick(
    frame: &mut Frame,
    pick: &BootstrapPickState,
    memory_path: &Path,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(60, 45, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Bootstrap review memory ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // header
            Constraint::Length(1), // "look back over"
            Constraint::Min(1),    // depth list
            Constraint::Length(2), // token/cost note
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    // Two explicit lines rather than one wrapped sentence: the destination is
    // now a choice (`g`), and the scope word plus a real path is more than a
    // 60%-wide overlay fits on one row without splitting mid-word.
    let muted = Style::default().fg(theme.text_muted.to_color());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                "  Distill common findings from merged/closed PRs into:",
                muted,
            )),
            Line::from(Span::styled(
                format!("  {} doc · {}", pick.scope.label(), memory_path.display()),
                muted,
            )),
        ]),
        chunks[0],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  Look back over:",
            Style::default().fg(theme.text.to_color()),
        ))),
        chunks[1],
    );

    let mut spans: Vec<Span> = vec![Span::raw("  ")];
    for (i, depth) in BootstrapDepth::ALL.iter().enumerate() {
        let is_selected = i == pick.selected;
        let marker = if is_selected { "(\u{2022})" } else { "( )" };
        let style = if is_selected {
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_muted.to_color())
        };
        spans.push(Span::styled(format!("{marker} {}", depth.label()), style));
        spans.push(Span::raw("    "));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[2]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  Comment fetch is free (gh); one agent pass distills the findings.",
            Style::default().fg(theme.text_muted.to_color()),
        )))
        .wrap(Wrap { trim: false }),
        chunks[3],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[⏎] run   [j/k] move   [g] project/global   [esc] cancel",
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[4],
    );
}

/// Confirm overlay for the review-memory compact pass (`c` in the PR picker):
/// shows how many findings are in the doc today before spending an agent pass
/// to merge near-duplicates and prune stale ones.
fn draw_compact_confirm(
    frame: &mut Frame,
    confirm: &CompactConfirmState,
    memory_path: &Path,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(60, 35, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Compact review memory ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // description
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    // Scope word and path on their own line, same as the bootstrap picker: the
    // destination is a choice (`g`) now, and a real path plus the count doesn't
    // fit one row of a 60%-wide overlay without splitting mid-word.
    let n = confirm.existing_findings;
    let note = if n == 0 {
        "  Nothing to compact here — press g to switch docs, or esc to cancel."
    } else {
        "  One agent pass merges near-duplicate findings and prunes stale ones. \
         You'll review the proposed doc before anything is written."
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                format!(
                    "  {n} finding{} in the {} doc:",
                    if n == 1 { "" } else { "s" },
                    confirm.scope.label()
                ),
                Style::default().fg(theme.text.to_color()),
            )),
            Line::from(Span::styled(
                format!("  {}", memory_path.display()),
                Style::default().fg(theme.text_muted.to_color()),
            )),
            Line::from(""),
            Line::from(Span::styled(
                note,
                Style::default().fg(theme.text_muted.to_color()),
            )),
        ])
        .wrap(Wrap { trim: false }),
        chunks[0],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[⏎] run   [g] project/global   [esc] cancel",
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[1],
    );
}

/// Full-screen progress view for the lookback bootstrap's background fetch +
/// distill pass.
pub fn draw_review_memory_bootstrap_running(
    frame: &mut Frame,
    state: &BootstrapRunState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = frame.area();
    let block = pane_block(theme).title(format!(
        " Bootstrap {} review memory (experimental) ",
        state.scope.label()
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);

    let status_line = match state.stage {
        BootstrapStage::FetchingComments => Line::from(vec![
            spinner,
            Span::styled(
                format!(" Fetching comments from {}...", state.depth.label()),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        BootstrapStage::Distilling {
            pr_count,
            token_estimate,
        } => Line::from(vec![
            spinner,
            Span::styled(
                format!(" Distilling findings from {pr_count} PRs (~{token_estimate} tokens)..."),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    };

    let body = Paragraph::new(vec![
        Line::from(""),
        status_line,
        Line::from(""),
        Line::from(Span::styled(
            "esc to return to the PR picker (the run keeps going in the background)",
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ])
    .wrap(Wrap { trim: false });
    frame.render_widget(body, inner);
}

/// Full-screen progress view for the review-memory compact pass's background
/// read + rewrite. Mirrors [`draw_review_memory_bootstrap_running`].
pub fn draw_review_memory_compact_running(
    frame: &mut Frame,
    state: &CompactRunState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = frame.area();
    let block = pane_block(theme).title(format!(
        " Compact {} review memory (experimental) ",
        state.scope.label()
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);

    let status_line = match state.stage {
        CompactStage::ReadingDoc => Line::from(vec![
            spinner,
            Span::styled(
                " Reading review memory doc...",
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        CompactStage::Compacting { token_estimate } => Line::from(vec![
            spinner,
            Span::styled(
                format!(" Compacting findings (~{token_estimate} tokens)..."),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
    };

    let body = Paragraph::new(vec![
        Line::from(""),
        status_line,
        Line::from(""),
        Line::from(Span::styled(
            "esc to return to the PR picker (the run keeps going in the background)",
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ])
    .wrap(Wrap { trim: false });
    frame.render_widget(body, inner);
}

/// Full-screen review of the compact pass's proposed replacement doc: an
/// editable preview of what will be written, shown before anything actually
/// is (mirrors [`draw_fix_confirm`]'s edit/confirm split and scroll handling,
/// full-screen rather than a centered dialog since a whole doc can run long).
pub fn draw_review_memory_compact_review(
    frame: &mut Frame,
    state: &mut CompactReviewState,
    theme: &Theme,
) {
    let area = frame.area();
    let block = pane_block(theme).title(format!(
        " Review compacted memory: {} \u{2192} {} findings (experimental) ",
        state.original_findings, state.proposed_findings
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut constraints = vec![Constraint::Length(1)]; // path line
    if state.error.is_some() {
        constraints.push(Constraint::Length(3)); // error / conflict message
    }
    constraints.push(Constraint::Min(1)); // doc body / editor
    constraints.push(Constraint::Length(1)); // key hints
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);
    let mut row = 0;

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  Will overwrite {}", state.path.display()),
            Style::default().fg(theme.text_muted.to_color()),
        ))),
        chunks[row],
    );
    row += 1;

    if let Some(error) = &state.error {
        // Already a full sentence at the call site — an io failure or a
        // "the doc changed under you" conflict, which read differently.
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                error.clone(),
                Style::default().fg(theme.danger.to_color()),
            )))
            .wrap(Wrap { trim: false }),
            chunks[row],
        );
        row += 1;
    }

    let doc_area = chunks[row];
    let doc_lines = super::editor_view::editor_lines(&state.editor, theme, "(empty doc)");
    let visible_lines = doc_area.height as usize;
    let mut wrap_width = doc_area.width as usize;
    let mut total_visual_lines =
        super::editor_view::count_wrapped_editor_lines(&doc_lines, wrap_width);
    if total_visual_lines > visible_lines && wrap_width > 1 {
        wrap_width -= 1;
        total_visual_lines = super::editor_view::count_wrapped_editor_lines(&doc_lines, wrap_width);
    }
    super::editor_view::sync_editor_scroll(
        &state.editor,
        &mut state.scroll,
        &mut state.sync_to_cursor,
        visible_lines,
        wrap_width,
        total_visual_lines,
    );
    frame.render_widget(
        Paragraph::new(doc_lines)
            .wrap(Wrap { trim: false })
            .scroll((state.scroll.min(u16::MAX as usize) as u16, 0)),
        doc_area,
    );
    if total_visual_lines > visible_lines {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(total_visual_lines)
            .position(state.scroll)
            .viewport_content_length(visible_lines);
        frame.render_stateful_widget(scrollbar, doc_area, &mut scrollbar_state);
    }
    row += 1;

    let hints = if state.editing {
        "[esc] done editing"
    } else {
        "[⏎/w] write   [e] edit   [esc] discard"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[row],
    );
}

fn pr_picker_row(
    entry: &crate::github::PrListEntry,
    current_user: Option<&str>,
    theme: &Theme,
) -> Line<'static> {
    let is_mine = current_user.is_some_and(|me| entry.author.eq_ignore_ascii_case(me));
    let author_style = if is_mine {
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text_muted.to_color())
    };
    let mut spans = vec![
        Span::styled(
            format!("#{} ", entry.number),
            Style::default().fg(theme.primary.to_color()),
        ),
        Span::styled(
            entry.title.clone(),
            Style::default().fg(theme.text.to_color()),
        ),
        Span::styled(format!("  · @{}", entry.author), author_style),
        Span::styled(
            format!(" · {}", entry.head_ref),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ];
    if is_mine {
        spans.push(chip("you", theme.primary.to_color()));
    }
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
    let block =
        pane_block(theme).title(format!(" PR Triage · #{} (experimental) ", state.pr.number));
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
                " Loading PR Triage comments (experimental)...",
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

/// Modal loading frame shown while a strictly read-only investigation of one
/// review comment runs off the UI thread. Deliberately blocking: the triage
/// overlay is stashed and comes back only when the run returns (or is
/// cancelled).
pub fn draw_pr_investigation_loading(
    frame: &mut Frame,
    state: &PrInvestigationLoadState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = frame.area();
    let block = pane_block(theme).title(format!(
        " PR Triage · #{} · Investigate (read-only) ",
        state.pr_number
    ));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);

    let elapsed = state.started_at.elapsed().as_secs();
    let body = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            spinner,
            Span::styled(
                format!(
                    " Investigating this review comment with {}… ({elapsed}s)",
                    state.harness.display_name()
                ),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Read-only: the agent inspects the repo but changes nothing.",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            format!("PR #{}  ·  {}", state.pr_number, state.pr_url),
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(Span::styled(
            "esc to stop waiting (the run finishes in the background)",
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ])
    .wrap(Wrap { trim: false });
    frame.render_widget(body, inner);
}

/// Full-screen PR-triage pane: comment list on the left, detail on the
/// right.
pub struct PrReviewUsage<'a> {
    pub cumulative: Option<&'a SessionTokenUsage>,
    pub visit: Option<&'a SessionTokenUsage>,
    pub pricing: &'a TokenPricingConfig,
}

#[allow(clippy::too_many_arguments)]
pub fn draw_pr_review(
    frame: &mut Frame,
    state: &mut PrReviewState,
    theme: &Theme,
    usage: PrReviewUsage<'_>,
    dedicated_session_working: Option<bool>,
    ai_review_status: &AiReviewTriageStatus,
    // `"<feature> · <harness> · <mode>"` for the companion triage feature when
    // that's the fix target — so the fix confirm dialog names exactly which
    // feature (and which mode) a fix will run in. `None` for the in-feature
    // targets, which run in the feature already on screen.
    triage_feature_summary: Option<&str>,
    // Both review-memory doc paths, so the "add to memory" dialog can name the
    // exact file its `g` scope toggle currently points at. `None` when that
    // dialog is closed — resolving them shells out to git, so the caller only
    // pays for it on the frames that actually need it.
    memory_paths: Option<&crate::app::review_memory::ReviewMemoryPaths>,
) {
    let area = frame.area();
    let review = &state.review;
    // `f`/`B` fix injection reads files from `state.workdir` regardless of
    // which PR is loaded (the picker lets the user open *any* PR in the
    // repo), so a mismatch here means a fix would silently land on the wrong
    // branch. This banner is the always-visible half of the guard; the fix
    // confirm dialog repeats it as the explicit-acknowledge half.
    let mismatch = state.branch_mismatch().map(|s| s.to_string());

    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                                      // header
            Constraint::Length(if mismatch.is_some() { 1 } else { 0 }), // branch-mismatch banner
            Constraint::Min(1),                                         // body
            Constraint::Length(2), // footer (keys + marker legend)
        ])
        .split(area);

    // Header.
    let mut header_spans = vec![
        Span::styled(
            format!(" PR Triage · #{} (experimental) ", review.pr.number),
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
    ];
    if let Some(working) = dedicated_session_working {
        let session_name = if state.fix_target == crate::app::pr_review::FixTarget::DedicatedReview
            && state.dedicated_session_label != crate::app::pr_review::TRIAGE_SESSION_LABEL
        {
            state.dedicated_session_label.as_str()
        } else {
            "dedicated"
        };
        let (label, color) = if working {
            (
                format!("  [{session_name} ● working]"),
                theme.warning.to_color(),
            )
        } else {
            (
                format!("  [{session_name} idle]"),
                theme.status_detail.to_color(),
            )
        };
        header_spans.push(Span::styled(label, Style::default().fg(color)));
    }
    // `A` opens a separate AI Review pane now, so a background pass kicked
    // off there and left running isn't visible from inside this pane unless
    // called out explicitly — this is that ambient "yes, it's still going"
    // signal (mirrors the dashboard/session ambient badge's own
    // `ai_review_running_for_workdir` check).
    // Name the companion feature in the header too, not only in the fix
    // confirm dialog: when fixes land in a *different* feature than the one on
    // screen, that's the single most important thing about the pane's state.
    if let Some(summary) = triage_feature_summary {
        header_spans.push(Span::styled(
            format!("  [{summary}]"),
            Style::default().fg(theme.secondary.to_color()),
        ));
    }
    match ai_review_status {
        AiReviewTriageStatus::Running => header_spans.push(Span::styled(
            "  [AI review running]",
            Style::default().fg(theme.warning.to_color()),
        )),
        AiReviewTriageStatus::Pending(count) => header_spans.push(Span::styled(
            format!("  [AI review pending: {count}]"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        )),
        AiReviewTriageStatus::NoFindings(run) => {
            let (text, _) = super::ai_review::ai_review_run_badge_text(run, false);
            header_spans.push(Span::styled(
                format!("  [AI review: {text}]"),
                Style::default().fg(theme.status_detail.to_color()),
            ));
        }
        AiReviewTriageStatus::Failed(run) => {
            let (text, _) = super::ai_review::ai_review_run_badge_text(run, false);
            header_spans.push(Span::styled(
                format!("  [AI review {text}]"),
                Style::default().fg(theme.danger.to_color()),
            ));
        }
        AiReviewTriageStatus::NotRun | AiReviewTriageStatus::CompletedWithFindings => {}
    }
    // Once the fix-target session exists, show what triage has spent on it —
    // the "only pay for what you asked for" constraint made visible in-pane.
    // The header row is a single unwrapped line, so on a narrow terminal this
    // span is dropped rather than left to be silently clipped mid-text.
    if let Some(cumulative) = usage.cumulative {
        let cumulative_text = format!(
            "  · {} {}",
            state.fix_target.tag(),
            format_feature_token_usage(cumulative, usage.pricing)
        );
        let used_width: usize = header_spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum();
        let visit_text = usage.visit.map(|visit_usage| {
            format!(
                "  · this visit {}",
                format_token_usage_summary(visit_usage, usage.pricing)
            )
        });
        // Prefer both totals. If the terminal is too narrow, keep the new
        // visit-specific tally visible before falling back to the cumulative
        // target-session total.
        let usage_text = visit_text
            .as_ref()
            .map(|visit| format!("{cumulative_text} {}", visit.trim_start()))
            .into_iter()
            .chain(visit_text)
            .chain(std::iter::once(cumulative_text))
            .find(|text| {
                used_width + UnicodeWidthStr::width(text.as_str()) <= outer[0].width as usize
            });
        if let Some(usage_text) = usage_text {
            header_spans.push(Span::styled(
                usage_text,
                Style::default().fg(theme.status_detail.to_color()),
            ));
        }
    }
    let header = Line::from(header_spans);
    frame.render_widget(Paragraph::new(header), outer[0]);

    if let Some(checked_out) = &mismatch {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(
                    " ⚠ reviewing PR for branch `{}`, but this worktree is on `{checked_out}` — fixes will be applied to `{checked_out}`, not `{}`",
                    review.pr.head_ref, review.pr.head_ref
                ),
                Style::default()
                    .fg(theme.danger.to_color())
                    .add_modifier(Modifier::BOLD),
            ))),
            outer[1],
        );
    }

    // Body: list | detail.
    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(outer[2]);

    draw_comment_list(frame, body[0], state, theme);
    let selected_investigation = state.selected_comment().and_then(|c| {
        state
            .investigations
            .iter()
            .find(|inv| inv.comment_id == c.id)
    });
    let detail_lines = draw_comment_detail(
        frame,
        body[1],
        state.selected_comment(),
        &state.review.comments,
        selected_investigation,
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
    // The batch hint shows the marked count so the user knows what `B` combines.
    let mut batch_hint = match state.marked.len() {
        0 => "space mark".to_string(),
        n => format!("space mark · B combine({n})"),
    };
    // Only offer the sibling-jump keys when the selected comment was actually
    // fixed as part of a combined batch — otherwise they are a no-op.
    if state
        .selected_comment()
        .is_some_and(|c| c.batch_id.is_some())
    {
        batch_hint.push_str(" · [/] siblings");
    }
    // `v` runs a read-only investigation; `a` acts on one once it has finished.
    let investigate_hint = {
        use crate::app::pr_review::PrInvestigationStatus;
        let finished = state.selected_comment().is_some_and(|c| {
            state.investigations.iter().any(|r| {
                r.comment_id == c.id
                    && matches!(
                        r.status,
                        PrInvestigationStatus::Complete
                            | PrInvestigationStatus::Failed
                            | PrInvestigationStatus::Dismissed
                    )
            })
        });
        if finished {
            "v investigate · a act"
        } else {
            "v investigate"
        }
    };
    let key_text = if state
        .selected_comment()
        .is_some_and(PrComment::is_amf_followup_reply)
    {
        format!(
            " AMF follow-up · context only   j/k move   {toggle_hint}   o sort→{}   i syntax   r refresh   g other-PR   A ai-review   esc/q close",
            state.sort_mode.label()
        )
    } else {
        // `I` only means something for the companion-feature target — the
        // other two commit straight onto the PR branch.
        let integrate_hint = if state.fix_target.is_companion_feature() {
            "   I integrate"
        } else {
            ""
        };
        format!(
            " j/k move   f fix→{}   {investigate_hint}   {batch_hint}   R reply   m mark   M memory   {toggle_hint}   o sort→{}   P session{integrate_hint}   i syntax   r refresh   g other-PR   A ai-review   esc/q close",
            state.fix_target.tag(),
            state.sort_mode.label()
        )
    };
    let keys = Paragraph::new(Line::from(Span::styled(
        key_text,
        Style::default().fg(theme.text_muted.to_color()),
    )));
    let footer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(outer[3]);
    frame.render_widget(keys, footer[0]);
    frame.render_widget(Paragraph::new(marker_legend(theme)), footer[1]);

    if let Some(pick) = &state.harness_pick {
        draw_harness_pick(frame, pick, theme);
    }
    // Per-run investigation harness picker (`v` then `f`), overlays the pane
    // before the blocking investigation starts.
    if let Some(pick) = &state.investigation_harness_pick {
        draw_investigation_harness_pick(frame, pick, theme);
    }
    // Completed-investigation action menu (`a`).
    if let Some(pick) = &state.investigation_action_pick {
        draw_investigation_action_pick(frame, pick, theme);
    }
    // Follow-up question editor.
    if let Some(draft) = &state.investigation_follow_up {
        draw_investigation_follow_up(frame, draft, theme);
    }
    // Triage-feature setup (`New feature…`) overlays the pane before the fix
    // confirm dialog it continues into.
    if let Some(setup) = &state.new_feature_setup {
        draw_triage_feature_setup(frame, setup, theme);
    }
    // Integration overlay (`I`): land the companion feature's commits on the PR.
    if let Some(integrate) = &state.integrate {
        draw_triage_integrate(frame, integrate, theme);
    }
    // Reply-kind picker (`R`) overlays the pane before the reply dialog itself.
    if let Some(pick) = &state.reply_kind_pick {
        draw_reply_kind_pick(frame, pick, theme);
    }
    // "Mark" picker (`m`) overlays the pane: Done (local) / Skip (local) /
    // Resolve on GitHub for the selected comment.
    if let Some(pick) = &state.mark_pick {
        draw_mark_pick(frame, pick, state.selected_comment(), theme);
    }
    // Fix confirm/edit dialog overlays the pane when open — but not while the
    // fix-target picker or triage-feature setup is stacked on top of it (via
    // `t`). Those capture all keyboard input and are drawn earlier, so the
    // opaque fix-confirm block would completely occlude the dialog the user is
    // actually driving. It stays in `state` and repaints once they close.
    let fix_target = state.fix_target;
    if state.harness_pick.is_none()
        && state.new_feature_setup.is_none()
        && let Some(confirm) = &mut state.fix_confirm
    {
        draw_fix_confirm(
            frame,
            confirm,
            fix_target,
            &state.dedicated_session_label,
            triage_feature_summary,
            mismatch.as_deref(),
            theme,
        );
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
    // "Add to memory" dialog overlays the pane when open.
    if let Some(memory_add) = &state.memory_add {
        let author = state
            .review
            .comments
            .iter()
            .find(|c| c.id == memory_add.comment_id)
            .map(|c| c.author.as_str())
            .unwrap_or("reviewer");
        draw_memory_add_dialog(
            frame,
            memory_add,
            author,
            memory_paths.map(|paths| paths.for_scope(memory_add.scope)),
            theme,
        );
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

    let agent_drafted = crate::app::pr_review::reply_effective_agent_drafted(reply);

    // Attribution is appended at post time, not part of the editable buffer
    // below (an empty "not needed" reason would otherwise start with a footer
    // already sitting in it) — this block previews what will actually be sent.
    // Re-evaluated against the live editor text (not the stored flag alone) so
    // editing a captured draft away from the agent's own words drops the AI
    // attribution in the preview too.
    let attribution = if agent_drafted {
        crate::app::pr_review::AI_ATTRIBUTION_FOOTER
    } else {
        crate::app::pr_review::AMF_ATTRIBUTION_FOOTER
    };
    let mut disclosure = Vec::new();
    if agent_drafted && let Some(metadata) = &reply.generation_metadata {
        disclosure.push(Line::from(Span::styled(
            metadata.source_disclosure(),
            Style::default().fg(theme.text_muted.to_color()),
        )));
        disclosure.push(Line::from(Span::styled(
            metadata.usage_disclosure(),
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }
    disclosure.push(Line::from(Span::styled(
        format!("will post with a \"{attribution}\" footer"),
        Style::default().fg(theme.text_muted.to_color()),
    )));
    // The point of this block is to let the user read the disclosure *before*
    // it is posted, so it wraps rather than clipping: a provider-qualified
    // model name ("anthropic/claude-opus-4-5-20251101") runs past a narrow
    // dialog on its own. `line_count` runs the same wrapper the renderer does,
    // so the rows reserved always match the rows drawn. Capped so a freak-long
    // model name can't squeeze the reply body and key hints off the dialog.
    let disclosure = Paragraph::new(disclosure).wrap(Wrap { trim: false });
    let max_disclosure_rows = inner.height.saturating_sub(2).max(1);
    let disclosure_rows = (disclosure.line_count(inner.width) as u16).clamp(1, max_disclosure_rows);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),                  // reply body
            Constraint::Length(disclosure_rows), // generation + attribution disclosure
            Constraint::Length(1),               // key hints
        ])
        .split(inner);

    let body_lines = super::editor_view::editor_lines(&reply.editor, theme, "(type a reply)");
    frame.render_widget(
        Paragraph::new(body_lines).wrap(Wrap { trim: false }),
        chunks[0],
    );

    frame.render_widget(disclosure, chunks[1]);

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
        chunks[2],
    );
}

/// "Add to memory" dialog: the selected comment's distilled finding, editable,
/// awaiting approval before it's appended to the review-memory doc. `Tab`
/// cycles the category in the confirm view and `g` toggles which doc it lands
/// in — this repo's, or the user's cross-project one.
fn draw_memory_add_dialog(
    frame: &mut Frame,
    memory_add: &crate::app::MemoryAddState,
    author: &str,
    memory_path: Option<&Path>,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(70, 50, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let category = crate::app::pr_review::MEMORY_CATEGORIES[memory_add.category];
    let scope = memory_add.scope.label();
    let block = Block::default()
        .title(format!(" Add to {scope} memory · {category} · @{author} "))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // finding text
            Constraint::Length(1), // destination doc
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    let body_lines =
        super::editor_view::editor_lines(&memory_add.editor, theme, "(describe the finding)");
    frame.render_widget(
        Paragraph::new(body_lines).wrap(Wrap { trim: false }),
        chunks[0],
    );

    // The title names the scope; this names the exact file, so "global" is
    // never a guess about where the finding actually went.
    if let Some(memory_path) = memory_path {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("→ {}", memory_path.display()),
                Style::default().fg(theme.text_muted.to_color()),
            ))),
            chunks[1],
        );
    }

    let hints = if memory_add.editing {
        "[esc] done editing"
    } else {
        "[⏎] add   [e] edit   [Tab] category   [g] project/global   [esc] cancel"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[2],
    );
}

/// Confirm/edit dialog: shows the exact prompt that will be injected (no file
/// contents — token principle #3) with a `~N tokens` preview, and lets the user
/// edit it before it reaches the agent.
fn draw_fix_confirm(
    frame: &mut Frame,
    confirm: &mut crate::app::FixConfirmState,
    target: crate::app::pr_review::FixTarget,
    dedicated_session_label: &str,
    triage_feature_summary: Option<&str>,
    branch_mismatch: Option<&str>,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(70, 70, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    // Surface the active keymap in the title so the user knows whether `Esc`
    // exits the dialog or just leaves vim insert mode.
    let mode_label = match confirm.editor.vim_mode() {
        Some(VimMode::Insert) => " · vim insert",
        Some(VimMode::Normal) => " · vim normal",
        None => "",
    };
    let title = match &confirm.batch {
        Some(ids) => format!(
            " Inject combined fix for {} comments{mode_label} ",
            ids.len()
        ),
        None => format!(" Inject fix into agent session{mode_label} "),
    };
    let block = Block::default()
        .title(title)
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

    // For the companion target, name the feature and the mode it runs in —
    // the whole point of that target is that they differ from the feature the
    // PR was built in, so "dedicated triage session" alone wouldn't say where
    // this lands.
    let target_text = match triage_feature_summary {
        Some(summary) => summary.to_string(),
        None if target == crate::app::pr_review::FixTarget::DedicatedReview => {
            format!("dedicated session '{dedicated_session_label}'")
        }
        None => target.label().to_string(),
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "Will inject into the ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled(
                target_text,
                Style::default()
                    .fg(theme.secondary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(":", Style::default().fg(theme.text_muted.to_color())),
        ])),
        chunks[0],
    );

    // Repurpose the spacer line as an explicit-acknowledge warning when the
    // workdir's checked-out branch doesn't match the PR being triaged — `⏎`
    // injecting from here is the confirm gate the mismatch requires (paired
    // with the always-visible pane-header banner).
    if let Some(checked_out) = branch_mismatch {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("⚠ this worktree is on `{checked_out}`, not the PR's branch — fix will land on `{checked_out}`"),
                Style::default()
                    .fg(theme.danger.to_color())
                    .add_modifier(Modifier::BOLD),
            ))),
            chunks[1],
        );
    }

    // Prompt body: a wrapped, scrollable editor view that follows the cursor
    // when editing and shows a scrollbar once the prompt overflows the dialog.
    let editor_area = chunks[2];
    let prompt_lines = super::editor_view::editor_lines(&confirm.editor, theme, "(empty prompt)");
    let visible_lines = editor_area.height as usize;
    let mut wrap_width = editor_area.width as usize;
    let mut total_visual_lines =
        super::editor_view::count_wrapped_editor_lines(&prompt_lines, wrap_width);
    // Leave room for the scrollbar column when the content overflows.
    if total_visual_lines > visible_lines && wrap_width > 1 {
        wrap_width -= 1;
        total_visual_lines =
            super::editor_view::count_wrapped_editor_lines(&prompt_lines, wrap_width);
    }
    super::editor_view::sync_editor_scroll(
        &confirm.editor,
        &mut confirm.scroll,
        &mut confirm.sync_to_cursor,
        visible_lines,
        wrap_width,
        total_visual_lines,
    );
    frame.render_widget(
        Paragraph::new(prompt_lines)
            .wrap(Wrap { trim: false })
            .scroll((confirm.scroll.min(u16::MAX as usize) as u16, 0)),
        editor_area,
    );
    if total_visual_lines > visible_lines {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(total_visual_lines)
            .position(confirm.scroll)
            .viewport_content_length(visible_lines);
        frame.render_stateful_widget(scrollbar, editor_area, &mut scrollbar_state);
    }

    let tokens = crate::app::pr_review::estimate_tokens(confirm.editor.text());
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("~{tokens} tokens · no file contents included"),
            Style::default().fg(theme.text_muted.to_color()),
        ))),
        chunks[3],
    );

    let hints = if confirm.editing {
        // Under vim, Esc is consumed by the editor (Insert→Normal), so the way
        // back to the confirm view is Ctrl+Q; in plain mode Esc does it.
        if confirm.editor.vim_mode().is_some() {
            "[tab] inject   [^t] vim   [^q] done editing"
        } else {
            "[tab] inject   [^t] vim   [esc] done editing"
        }
    } else {
        "[⏎] inject   [e] edit   [t] target   [^j/k] scroll   [esc] cancel"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[4],
    );
}

/// Single-select harness picker for the dedicated triage session, shown on the
/// first fix of a PR. The chosen harness is remembered for the rest of the PR
/// (the session is created once and reused).
fn draw_harness_pick(frame: &mut Frame, pick: &crate::app::HarnessPickState, theme: &Theme) {
    let area = super::super::dashboard::centered_rect(60, 40, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let naming = pick.session_name.as_ref();
    let block = Block::default()
        .title(if naming.is_some() {
            " Dedicated session name "
        } else {
            " Fix target "
        })
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // header
            Constraint::Min(1),    // row list
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    if let Some(name) = naming {
        let harness = pick
            .rows
            .get(pick.selected)
            .map(crate::app::pr_review::FixTargetPickRow::label)
            .unwrap_or_else(|| "Dedicated triage session".to_string());
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    format!("  {harness}"),
                    Style::default().fg(theme.text_muted.to_color()),
                )),
                Line::from(vec![
                    Span::styled("  > ", Style::default().fg(theme.warning.to_color())),
                    if name.is_empty() {
                        Span::styled(
                            "PR Triage (default)",
                            Style::default().fg(theme.text_muted.to_color()),
                        )
                    } else {
                        Span::styled(name.clone(), Style::default().fg(theme.text.to_color()))
                    },
                ]),
            ])
            .wrap(Wrap { trim: false }),
            chunks[0].union(chunks[1]),
        );
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "[⏎] use name   [Ctrl+U] clear   [esc] back   [Ctrl+Q] cancel",
                Style::default().fg(theme.primary.to_color()),
            ))),
            chunks[2],
        );
        return;
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  Run this PR's fixes on:",
            Style::default().fg(theme.text_muted.to_color()),
        )))
        .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let mut lines: Vec<Line> = Vec::new();
    for (i, row) in pick.rows.iter().enumerate() {
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
            Span::styled(row.label(), name_style),
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

/// Flat per-run harness picker for a pending read-only investigation.
fn draw_investigation_harness_pick(
    frame: &mut Frame,
    pick: &InvestigationHarnessPick,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(56, 40, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Investigate with ")
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
            "  Run this read-only investigation on:",
            Style::default().fg(theme.text_muted.to_color()),
        )))
        .wrap(Wrap { trim: false }),
        chunks[0],
    );

    let mut lines: Vec<Line> = Vec::new();
    for (i, harness) in pick.harnesses.iter().enumerate() {
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
            Span::styled(harness.display_name().to_string(), name_style),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), chunks[1]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[⏎] start   [j/k] move   [esc] cancel",
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[2],
    );
}

/// The completed-investigation action menu (`a`): a small modal list.
fn draw_investigation_action_pick(
    frame: &mut Frame,
    pick: &InvestigationActionPick,
    theme: &Theme,
) {
    // Sized for the full six rows plus the key hint even on a short terminal.
    let area = super::super::dashboard::centered_rect(56, 55, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Investigation → ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    let mut lines: Vec<Line> = Vec::new();
    for (i, action) in InvestigationAction::ALL.iter().enumerate() {
        let is_selected = i == pick.selected;
        let marker = if is_selected { ">" } else { " " };
        let style = if is_selected {
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
            Span::styled(action.label(), style),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), chunks[0]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[⏎] choose   [j/k] move   [esc] cancel",
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[1],
    );
}

/// Inline editor for a follow-up question on an investigation.
fn draw_investigation_follow_up(
    frame: &mut Frame,
    draft: &InvestigationFollowUpDraft,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(66, 40, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Ask a follow-up ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // hint
            Constraint::Min(1),    // editor
            Constraint::Length(1), // keys
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "  Re-runs read-only with the prior answer as context.",
            Style::default().fg(theme.text_muted.to_color()),
        )))
        .wrap(Wrap { trim: false }),
        chunks[0],
    );
    let body_lines =
        super::editor_view::editor_lines(&draft.editor, theme, "(type a follow-up question)");
    frame.render_widget(
        Paragraph::new(body_lines).wrap(Wrap { trim: false }),
        chunks[1],
    );
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[Tab] ask (pick a harness)   [esc] cancel",
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[2],
    );
}

/// Compact setup overlay for the `New feature…` fix target: one settings list
/// (preset / harness / vibe mode / review / chrome / branch) rather than a
/// re-run of the multi-step feature wizard, because the user is mid-triage.
fn draw_triage_feature_setup(
    frame: &mut Frame,
    setup: &crate::app::TriageFeatureSetupState,
    theme: &Theme,
) {
    use crate::app::TriageSetupRow;

    let area = super::super::dashboard::centered_rect(64, 60, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" New triage feature ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // explanation
            Constraint::Min(1),    // settings rows
            Constraint::Length(1), // error
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(vec![Line::from(Span::styled(
            "Fixes run in their own worktree on a companion branch, with the settings below — \
             independent of the feature the PR was built in. Landing them on the PR is an \
             explicit step (I).",
            Style::default().fg(theme.text_muted.to_color()),
        ))])
        .wrap(Wrap { trim: true }),
        chunks[0],
    );

    let value_for = |row: TriageSetupRow| -> String {
        match row {
            TriageSetupRow::Preset => setup.preset_label(),
            TriageSetupRow::Harness => setup.agent().display_name().to_string(),
            TriageSetupRow::Mode => format!(
                "{} — {}",
                setup.mode.display_name(),
                setup.mode.description()
            ),
            TriageSetupRow::Review => if setup.review { "on" } else { "off" }.to_string(),
            TriageSetupRow::Chrome => if setup.enable_chrome { "on" } else { "off" }.to_string(),
            TriageSetupRow::Branch => setup.branch.clone(),
        }
    };

    let lines: Vec<Line> = TriageSetupRow::ALL
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let is_selected = i == setup.row;
            let marker = if is_selected { ">" } else { " " };
            let value_style = if is_selected {
                Style::default()
                    .fg(theme.text.to_color())
                    .bg(theme.effective_selection_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };
            let mut value = value_for(*row);
            // A visible caret makes the branch row read as a text field rather
            // than one more cyclable value.
            if is_selected && *row == TriageSetupRow::Branch {
                value.push('▏');
            }
            Line::from(vec![
                Span::styled(
                    format!("  {marker} "),
                    Style::default().fg(theme.warning.to_color()),
                ),
                Span::styled(
                    format!("{:<13}", row.label()),
                    Style::default().fg(theme.text_muted.to_color()),
                ),
                Span::styled(value, value_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), chunks[1]);

    if let Some(error) = &setup.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {error}"),
                Style::default()
                    .fg(theme.danger.to_color())
                    .add_modifier(Modifier::BOLD),
            ))),
            chunks[2],
        );
    }

    let hints = if setup.focused_row() == TriageSetupRow::Branch {
        "[⏎] create   [↑/↓] move   [type] edit branch   [esc] cancel"
    } else {
        "[⏎] create   [j/k] move   [h/l] change   [esc] cancel"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hints,
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[3],
    );
}

/// Integration overlay (`I`): what the companion triage feature has committed
/// since it branched, and the two non-destructive ways to land it on the PR.
fn draw_triage_integrate(
    frame: &mut Frame,
    integrate: &crate::app::TriageIntegrateState,
    theme: &Theme,
) {
    use crate::app::TriageIntegration;

    let area = super::super::dashboard::centered_rect(70, 66, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Land triage commits on the PR ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // branches
            Constraint::Min(1),    // commit preview
            Constraint::Length(3), // option rows
            Constraint::Length(2), // status / error
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(vec![Line::from(vec![
            Span::styled("  ", Style::default()),
            Span::styled(
                integrate.triage_branch.clone(),
                Style::default()
                    .fg(theme.secondary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" → ", Style::default().fg(theme.text_muted.to_color())),
            Span::styled(
                integrate.pr_branch.clone(),
                Style::default()
                    .fg(theme.secondary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ])])
        .wrap(Wrap { trim: true }),
        chunks[0],
    );

    let mut commit_lines: Vec<Line> = Vec::new();
    if integrate.commits.is_empty() {
        commit_lines.push(Line::from(Span::styled(
            "  No commits on the triage branch yet — nothing to land.",
            Style::default().fg(theme.text_muted.to_color()),
        )));
    } else {
        commit_lines.push(Line::from(Span::styled(
            format!("  {} commit(s) to land:", integrate.commits.len()),
            Style::default().fg(theme.text_muted.to_color()),
        )));
        for commit in &integrate.commits {
            commit_lines.push(Line::from(Span::styled(
                format!("    {commit}"),
                Style::default().fg(theme.text.to_color()),
            )));
        }
    }
    if integrate.triage_dirty {
        commit_lines.push(Line::from(Span::styled(
            "  ⚠ the triage worktree has uncommitted changes — they will not be included",
            Style::default().fg(theme.warning.to_color()),
        )));
    }
    frame.render_widget(
        Paragraph::new(commit_lines).wrap(Wrap { trim: false }),
        chunks[1],
    );

    let option_lines: Vec<Line> = TriageIntegration::ALL
        .iter()
        .enumerate()
        .map(|(i, option)| {
            let disabled =
                *option == TriageIntegration::CherryPick && integrate.source_dirty.is_some();
            let is_selected = i == integrate.selected;
            let marker = if is_selected { ">" } else { " " };
            let style = if disabled {
                Style::default().fg(theme.text_muted.to_color())
            } else if is_selected {
                Style::default()
                    .fg(theme.text.to_color())
                    .bg(theme.effective_selection_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };
            let mut spans = vec![
                Span::styled(
                    format!("  {marker} "),
                    Style::default().fg(theme.warning.to_color()),
                ),
                Span::styled(option.label(), style),
            ];
            if disabled {
                spans.push(Span::styled(
                    "  [unavailable]",
                    Style::default().fg(theme.danger.to_color()),
                ));
            }
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(option_lines), chunks[2]);

    let mut status: Vec<Line> = Vec::new();
    if let Some(done) = &integrate.done {
        status.push(Line::from(Span::styled(
            format!("  ✓ {done}"),
            Style::default()
                .fg(theme.success.to_color())
                .add_modifier(Modifier::BOLD),
        )));
    }
    if let Some(error) = &integrate.error {
        status.push(Line::from(Span::styled(
            format!("  {error}"),
            Style::default()
                .fg(theme.danger.to_color())
                .add_modifier(Modifier::BOLD),
        )));
    }
    if let Some(reason) = &integrate.source_dirty {
        status.push(Line::from(Span::styled(
            format!("  Cherry-pick unavailable: {reason}"),
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }
    frame.render_widget(Paragraph::new(status).wrap(Wrap { trim: true }), chunks[3]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[⏎] run   [j/k] move   [esc] close",
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[4],
    );
}

/// Reply-kind picker (`R`): two rows, "Done" and "Not needed", shown before
/// the actual reply dialog opens.
fn draw_reply_kind_pick(frame: &mut Frame, pick: &ReplyKindPickState, theme: &Theme) {
    let area = super::super::dashboard::centered_rect(50, 30, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Reply ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // row list
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    let lines: Vec<Line> = ReplyKind::ALL
        .iter()
        .enumerate()
        .map(|(i, kind)| {
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
            Line::from(vec![
                Span::styled(
                    format!("  {marker} "),
                    Style::default().fg(theme.warning.to_color()),
                ),
                Span::styled(kind.menu_label().to_string(), name_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), chunks[0]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[⏎] choose   [j/k] move   [esc] cancel",
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[1],
    );
}

/// "Mark" picker (`m`): three rows — Done (local), Skip (local), Resolve on
/// GitHub — reflecting the selected comment's current state. The GitHub row
/// is deliberately styled apart from the two local ones (a distinct color)
/// since it's the only one of the three that writes anywhere outside AMF.
fn draw_mark_pick(
    frame: &mut Frame,
    pick: &MarkPickState,
    comment: Option<&PrComment>,
    theme: &Theme,
) {
    let area = super::super::dashboard::centered_rect(56, 34, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Mark ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // row list
            Constraint::Length(1), // key hints
        ])
        .split(inner);

    let lines: Vec<Line> = MarkAction::ALL
        .iter()
        .enumerate()
        .map(|(i, action)| {
            let is_selected = i == pick.selected;
            let marker = if is_selected { ">" } else { " " };
            let is_github = matches!(action, MarkAction::ResolveOnGitHub);
            let base_color = if is_github {
                theme.warning.to_color()
            } else {
                theme.text.to_color()
            };
            let name_style = if is_selected {
                Style::default()
                    .fg(base_color)
                    .bg(theme.effective_selection_bg())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(base_color)
            };
            Line::from(vec![
                Span::styled(
                    format!("  {marker} "),
                    Style::default().fg(theme.warning.to_color()),
                ),
                Span::styled(action.menu_label(comment), name_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), chunks[0]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "[⏎] choose   [j/k] move   [esc] cancel",
            Style::default().fg(theme.primary.to_color()),
        ))),
        chunks[1],
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

    // The selected comment's combined-batch id, so its sibling rows can be
    // marked more brightly than other (unrelated) batch members.
    let selected_batch_id = state.selected_comment().and_then(|c| c.batch_id.clone());

    // Inner width available for row text (block borders take one column each side).
    let inner_width = area.width.saturating_sub(2) as usize;
    // Under `PrSortMode::Conversations`, a divider row is inserted ahead of
    // the conversation-comment group so it reads as a real section rather
    // than a silent reorder — this shifts every row from that point on by
    // one `items` slot relative to its position in `visible`, which the
    // highlight lookup below accounts for.
    let divider_at = state.conversation_section_start();
    let mut items: Vec<ListItem> = Vec::with_capacity(visible.len() + 1);
    for (pos, &i) in visible.iter().enumerate() {
        if divider_at == Some(pos) {
            items.push(ListItem::new(Line::from(Span::styled(
                "  ─ Conversation ─",
                Style::default().fg(theme.text_muted.to_color()),
            ))));
        }
        let comment = &state.review.comments[i];
        let is_marked = state.marked.contains(&comment.id);
        items.push(ListItem::new(comment_list_line(
            comment,
            is_marked,
            batch_rel(comment, selected_batch_id.as_deref(), i == state.selected),
            theme,
            inner_width,
        )));
    }

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

    // The list renders only visible comments (plus the divider), so translate
    // the absolute selection index into its position within `items`.
    let highlight = visible
        .iter()
        .position(|&i| i == state.selected)
        .map(|pos| {
            if divider_at.is_some_and(|d| d <= pos) {
                pos + 1
            } else {
                pos
            }
        });
    let mut list_state = ListState::default();
    list_state.select(highlight);
    frame.render_stateful_widget(list, area, &mut list_state);
}

/// One row in the comment list: a batch-mark dot, a local-triage checkbox, a
/// resolution marker, author, location, snippet. Long paths are truncated from
/// the left so the filename and line number stay visible when the row is narrow.
/// How a list row relates to the *currently selected* comment's combined
/// batch. Drives the `⧉` marker: absent when the row isn't part of any batch,
/// dim for an unrelated batch, bright when it's a sibling of the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BatchRel {
    None,
    OtherBatch,
    Sibling,
}

fn batch_rel(c: &PrComment, selected_batch_id: Option<&str>, is_selected_row: bool) -> BatchRel {
    match c.batch_id.as_deref() {
        None => BatchRel::None,
        Some(id) if !is_selected_row && selected_batch_id == Some(id) => BatchRel::Sibling,
        Some(_) => BatchRel::OtherBatch,
    }
}

fn comment_list_line<'a>(
    c: &'a PrComment,
    is_marked: bool,
    batch_rel: BatchRel,
    theme: &Theme,
    width: usize,
) -> Line<'a> {
    let amf_authored = c.is_amf_authored();
    let context_only = !c.is_actionable();
    let marker = if c.is_resolved { "✓" } else { " " };
    // A leading `●` flags comments marked (space) for the `F` batch fix.
    let mark = if is_marked { "●" } else { " " };
    // `⧉` flags a comment that was fixed as part of a combined batch (`B`);
    // rendered brightly on the selected comment's own siblings so `[`/`]` has
    // a visible target.
    let batch_span = match batch_rel {
        BatchRel::None => String::new(),
        _ => "⧉ ".to_string(),
    };
    let location = match (&c.path, c.line) {
        (Some(path), Some(line)) => format!("{path}:{line}"),
        (Some(path), None) => path.clone(),
        (None, _) => kind_label(&c.kind).to_string(),
    };

    let location_style = if c.is_resolved || context_only {
        Style::default().fg(theme.text_muted.to_color())
    } else {
        Style::default().fg(theme.text.to_color())
    };

    let mark_span = format!("{mark} ");
    let triage_span = format!("[{}] ", c.triage.marker());
    let marker_span = format!("{marker} ");
    let author_span = format!("@{}  ", c.author);
    let attribution_span = if amf_authored { "[via AMF] " } else { "" };

    // Everything before the location is fixed-width; give the rest to the
    // location and left-ellipsize so the tail (filename:line) survives.
    let prefix_width = mark_span.chars().count()
        + triage_span.chars().count()
        + batch_span.chars().count()
        + marker_span.chars().count()
        + author_span.chars().count()
        + attribution_span.chars().count();
    let location = truncate_left(&location, width.saturating_sub(prefix_width));

    Line::from(vec![
        Span::styled(
            mark_span,
            Style::default().fg(if context_only {
                theme.text_muted.to_color()
            } else {
                theme.warning.to_color()
            }),
        ),
        Span::styled(
            triage_span,
            Style::default().fg(if context_only {
                theme.text_muted.to_color()
            } else {
                triage_color(c.triage, theme)
            }),
        ),
        Span::styled(
            batch_span,
            match batch_rel {
                BatchRel::Sibling => Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
                _ => Style::default().fg(theme.text_muted.to_color()),
            },
        ),
        Span::styled(
            marker_span,
            Style::default().fg(if context_only {
                theme.text_muted.to_color()
            } else {
                theme.success.to_color()
            }),
        ),
        Span::styled(
            author_span,
            if context_only {
                Style::default().fg(theme.text_muted.to_color())
            } else {
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD)
            },
        ),
        Span::styled(
            attribution_span,
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(location, location_style),
        Span::styled(
            format!("  {}", c.snippet),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ])
}

/// Truncate `s` to at most `max` display columns, keeping the tail and marking
/// the elision with a leading `…`. Used for file paths so the filename (and its
/// line number) remain visible when the row is too narrow for the full path.
pub(crate) fn truncate_left(s: &str, max: usize) -> String {
    let len = s.chars().count();
    if len <= max {
        return s.to_string();
    }
    if max <= 1 {
        return "…".to_string();
    }
    let tail: String = s.chars().skip(len - (max - 1)).collect();
    format!("…{tail}")
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
    all_comments: &[PrComment],
    investigation: Option<&crate::db::pr_investigations::PrInvestigation>,
    scroll: usize,
    theme: &Theme,
) -> usize {
    // The detail pane is the unfocused side (the list takes key input), so give
    // it a muted border to keep focus visually on the list.
    let block = pane_block(theme)
        .border_style(Style::default().fg(theme.text_muted.to_color()))
        .title(" Detail ");
    let inner = block.inner(area);
    // This pane replaces content from comments whose rendered shapes can be
    // completely different (a long highlighted hunk followed by a short
    // review summary, for example). Clear the pane's cells explicitly before
    // drawing the replacement so fragments from an earlier/underlying render
    // cannot survive in rows the new Paragraph leaves blank.
    frame.render_widget(Clear, area);
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
    if c.is_amf_authored()
        && let Some(line) = lines.last_mut()
    {
        let label = if c.is_actionable() {
            "via AMF"
        } else {
            "via AMF · context only"
        };
        line.spans.push(chip(label, theme.text_muted.to_color()));
    }

    // Diff hunk, colored like a diff (add/remove/context/hunk-header) — unless
    // it's whole-file-sized, in which case show the file reference the prompt
    // sends instead of a wall of diff.
    if let Some(hunk) = c.prompt_hunk() {
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
        lines.extend(diff_hunk_lines(&hunk, c.path.as_deref(), theme));
    } else if c.hunk_suppressed() {
        lines.push(divider(width, theme));
        lines.push(Line::from(Span::styled(
            match (c.file_level, &c.path) {
                (true, Some(path)) => format!("comment on file {path}"),
                (true, None) => "comment on the whole file".to_string(),
                // Line-anchored, but its hunk is whole-file-sized: say it was
                // withheld rather than implying the comment is file-level.
                (false, _) => "diff hunk omitted (covers the whole file)".to_string(),
            },
            Style::default().fg(theme.text_muted.to_color()),
        )));
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

    // Read-only investigation of this comment (`v` → `f`), when one exists.
    if let Some(inv) = investigation {
        lines.extend(investigation_detail_lines(inv, width, theme));
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

    // Replies already posted to this thread — however they got there. AMF's
    // own `R`/`n` flow marks the comment `Done`/`Skipped` locally, but a reply
    // posted some other way (a headless agent shelling out to `gh` directly)
    // leaves no local triage record at all — the reply is just another fetched
    // comment in the list. Surfacing it here means confirming a thread already
    // got an answer is a glance at the original comment, not a hunt through the
    // flat list for a same-thread entry.
    let replies = c.replies_in(all_comments);
    if !replies.is_empty() {
        lines.push(divider(width, theme));
        lines.push(section_label("Replies", theme));
        for reply in &replies {
            let mut spans = vec![Span::styled(
                format!("↳ @{}", reply.author),
                Style::default().fg(theme.secondary.to_color()),
            )];
            if reply.is_amf_authored() {
                spans.push(chip("via AMF", theme.text_muted.to_color()));
            }
            if reply.outdated {
                spans.push(chip("outdated", theme.warning.to_color()));
            }
            if reply.is_resolved {
                spans.push(chip("✓ resolved", theme.success.to_color()));
            }
            lines.push(Line::from(spans));
            lines.push(Line::from(Span::styled(
                format!("   {}", reply.snippet),
                Style::default().fg(theme.text.to_color()),
            )));
        }
    }

    let body = Paragraph::new(lines).wrap(Wrap { trim: false });
    // `scroll.y` is measured in rows after wrapping, not in the source
    // `Vec<Line>` entries. `line_count` runs the same `WordWrapper` the
    // renderer does, so the clamp can never disagree with what `Paragraph`
    // actually draws — no matter how long diff lines or Markdown wrap.
    let count = body.line_count(inner.width);
    frame.render_widget(body.scroll((scroll as u16, 0)), inner);
    count
}

/// The detail-pane "Investigation" section for a comment that has one: a status
/// header (state · harness · time), then the answer markdown (or the running /
/// failure state), then any follow-up turns.
fn investigation_detail_lines(
    inv: &crate::db::pr_investigations::PrInvestigation,
    width: usize,
    theme: &Theme,
) -> Vec<Line<'static>> {
    use crate::app::pr_review::PrInvestigationStatus;

    let mut lines: Vec<Line<'static>> = Vec::new();
    lines.push(divider(width, theme));
    lines.push(section_label("Investigation (read-only)", theme));

    let (status_label, status_color) = match inv.status {
        PrInvestigationStatus::Running => ("running", theme.warning.to_color()),
        PrInvestigationStatus::Complete => ("complete", theme.success.to_color()),
        PrInvestigationStatus::Failed => ("failed", theme.danger.to_color()),
        PrInvestigationStatus::Dismissed => ("dismissed", theme.text_muted.to_color()),
    };
    // Trim the sub-second precision from the stored `YYYY-MM-DD HH:MM:SS.mmm`.
    let when: String = inv.updated_at.chars().take(19).collect();
    lines.push(Line::from(vec![
        chip(status_label, status_color),
        chip(inv.harness.display_name(), theme.secondary.to_color()),
        chip(&when, theme.text_muted.to_color()),
    ]));

    match inv.status {
        PrInvestigationStatus::Running => {
            lines.push(Line::from(Span::styled(
                "thinking… (blocking run in progress)",
                Style::default().fg(theme.text_muted.to_color()),
            )));
        }
        PrInvestigationStatus::Failed => {
            lines.push(Line::from(Span::styled(
                inv.error
                    .clone()
                    .unwrap_or_else(|| "the investigation failed".to_string()),
                Style::default().fg(theme.danger.to_color()),
            )));
        }
        PrInvestigationStatus::Complete | PrInvestigationStatus::Dismissed => {
            match inv.answer.as_deref().filter(|a| !a.trim().is_empty()) {
                Some(answer) => {
                    lines
                        .extend(crate::markdown::render_markdown(answer, theme, width, None).lines);
                }
                None => lines.push(Line::from(Span::styled(
                    "(no answer recorded)",
                    Style::default().fg(theme.text_muted.to_color()),
                ))),
            }
        }
    }

    for (i, turn) in inv.follow_ups.iter().enumerate() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!("Follow-up {}: {}", i + 1, turn.question.trim()),
            Style::default()
                .fg(theme.secondary.to_color())
                .add_modifier(Modifier::BOLD),
        )));
        if !turn.answer.trim().is_empty() {
            lines.extend(crate::markdown::render_markdown(&turn.answer, theme, width, None).lines);
        }
    }

    lines
}

/// A compact `[label]` chip in the given accent color, with a leading space so
/// chips read as a spaced row.
pub(crate) fn chip(label: &str, color: ratatui::style::Color) -> Span<'static> {
    Span::styled(format!(" [{label}]"), Style::default().fg(color))
}

/// A full-width horizontal divider line in a muted color.
pub(crate) fn divider(width: usize, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        "─".repeat(width),
        Style::default().fg(theme.text_muted.to_color()),
    ))
}

/// A small muted section label inside the detail pane.
pub(crate) fn section_label(label: &str, theme: &Theme) -> Line<'static> {
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
pub(crate) fn diff_hunk_lines(hunk: &str, path: Option<&str>, theme: &Theme) -> Vec<Line<'static>> {
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
        Span::styled(" ● marked", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            "   ✓ resolved",
            Style::default().fg(theme.success.to_color()),
        ),
        Span::styled(
            "   [outdated] line moved",
            Style::default().fg(theme.warning.to_color()),
        ),
        Span::styled("   bot/human/ai", muted),
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
pub(crate) fn syntax_install_hint(path: Option<&str>) -> Option<String> {
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

pub(crate) fn pane_block(theme: &Theme) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()))
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

    fn sample_entry(author: &str) -> crate::github::PrListEntry {
        crate::github::PrListEntry {
            number: 1,
            title: "Sample PR".to_string(),
            author: author.to_string(),
            head_ref: "feature-branch".to_string(),
            updated_at: String::new(),
            is_draft: false,
            state: "OPEN".to_string(),
        }
    }

    #[test]
    fn pr_picker_row_tags_the_current_users_own_pr() {
        let theme = Theme::default();
        let entry = sample_entry("alice");
        let line = pr_picker_row(&entry, Some("alice"), &theme);
        assert!(line_text(&line).contains("you"));
    }

    #[test]
    fn pr_picker_row_matches_current_user_case_insensitively() {
        let theme = Theme::default();
        let entry = sample_entry("Alice");
        let line = pr_picker_row(&entry, Some("alice"), &theme);
        assert!(line_text(&line).contains("you"));
    }

    #[test]
    fn pr_picker_row_does_not_tag_other_authors() {
        let theme = Theme::default();
        let entry = sample_entry("bob");
        let line = pr_picker_row(&entry, Some("alice"), &theme);
        assert!(!line_text(&line).contains("you"));
    }

    #[test]
    fn pr_picker_row_does_not_tag_when_current_user_unresolved() {
        let theme = Theme::default();
        let entry = sample_entry("alice");
        let line = pr_picker_row(&entry, None, &theme);
        assert!(!line_text(&line).contains("you"));
    }

    fn render_fix_confirm_with_target(
        branch_mismatch: Option<&str>,
        triage_feature_summary: Option<&str>,
    ) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let mut confirm = crate::app::FixConfirmState {
            editor: crate::editor::TextEditor::new("fix this".to_string()),
            editing: false,
            scroll: 0,
            sync_to_cursor: false,
            batch: None,
            reply_draft_requests: Vec::new(),
        };
        let theme = Theme::default();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_fix_confirm(
                    frame,
                    &mut confirm,
                    crate::app::pr_review::FixTarget::NewFeature,
                    crate::app::pr_review::TRIAGE_SESSION_LABEL,
                    triage_feature_summary,
                    branch_mismatch,
                    &theme,
                )
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn fix_confirm_names_the_triage_feature_and_mode_for_the_companion_target() {
        // The whole point of the `New feature…` target is that the feature and
        // mode differ from the one on screen, so the dialog has to say which.
        let rendered = render_fix_confirm_with_target(None, Some("main-triage · Codex · Vibeless"));
        assert!(rendered.contains("main-triage"));
        assert!(rendered.contains("Vibeless"));
    }

    #[test]
    fn fix_confirm_falls_back_to_the_target_label_without_a_feature_summary() {
        let rendered = render_fix_confirm_with_target(None, None);
        assert!(rendered.contains("triage feature"));
    }

    fn render_triage_setup(setup: &crate::app::TriageFeatureSetupState) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let theme = Theme::default();
        let backend = TestBackend::new(110, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_triage_feature_setup(frame, setup, &theme))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn sample_triage_setup() -> crate::app::TriageFeatureSetupState {
        crate::app::TriageFeatureSetupState {
            presets: Vec::new(),
            preset_index: 0,
            agents: vec![crate::project::AgentKind::Codex],
            agent_index: 0,
            mode: crate::project::VibeMode::Vibeless,
            review: false,
            enable_chrome: false,
            branch: "main-triage".to_string(),
            row: 0,
            error: None,
            pending_batch: false,
        }
    }

    #[test]
    fn triage_setup_lists_every_setting_and_the_companion_branch() {
        let rendered = render_triage_setup(&sample_triage_setup());
        for label in ["Preset", "Harness", "Vibe mode", "Review mode", "Branch"] {
            assert!(rendered.contains(label), "missing row: {label}");
        }
        assert!(rendered.contains("main-triage"));
        assert!(rendered.contains("Codex"));
        assert!(rendered.contains("Vibeless"));
    }

    #[test]
    fn triage_setup_surfaces_a_creation_error_inline() {
        let mut setup = sample_triage_setup();
        setup.error = Some("Feature 'main-triage' already exists".to_string());
        let rendered = render_triage_setup(&setup);
        assert!(rendered.contains("already exists"));
    }

    fn render_integrate(integrate: &crate::app::TriageIntegrateState) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let theme = Theme::default();
        let backend = TestBackend::new(110, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_triage_integrate(frame, integrate, &theme))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn sample_integrate() -> crate::app::TriageIntegrateState {
        crate::app::TriageIntegrateState {
            triage_branch: "main-triage".to_string(),
            pr_branch: "main".to_string(),
            commits: vec!["abc1234 apply review comment".to_string()],
            source_dirty: None,
            triage_dirty: false,
            selected: 0,
            error: None,
            done: None,
            companion_feature_id: None,
        }
    }

    #[test]
    fn integrate_shows_both_options_and_the_commits_to_land() {
        let rendered = render_integrate(&sample_integrate());
        assert!(rendered.contains("main-triage"));
        assert!(rendered.contains("apply review comment"));
        assert!(rendered.contains("Push to the PR branch"));
        assert!(rendered.contains("Cherry-pick"));
        assert!(!rendered.contains("unavailable"));
    }

    #[test]
    fn integrate_marks_cherry_pick_unavailable_against_a_dirty_source() {
        let mut integrate = sample_integrate();
        integrate.source_dirty = Some("the source worktree has uncommitted changes".to_string());
        let rendered = render_integrate(&integrate);
        assert!(rendered.contains("[unavailable]"));
        assert!(rendered.contains("uncommitted changes"));
    }

    #[test]
    fn integrate_warns_when_the_triage_worktree_has_uncommitted_work() {
        let mut integrate = sample_integrate();
        integrate.triage_dirty = true;
        let rendered = render_integrate(&integrate);
        assert!(rendered.contains("will not be included"));
    }

    fn render_fix_confirm(branch_mismatch: Option<&str>) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let mut confirm = crate::app::FixConfirmState {
            editor: crate::editor::TextEditor::new("fix this".to_string()),
            editing: false,
            scroll: 0,
            sync_to_cursor: false,
            batch: None,
            reply_draft_requests: Vec::new(),
        };
        let theme = Theme::default();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_fix_confirm(
                    frame,
                    &mut confirm,
                    crate::app::pr_review::FixTarget::default(),
                    crate::app::pr_review::TRIAGE_SESSION_LABEL,
                    None,
                    branch_mismatch,
                    &theme,
                )
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn fix_confirm_shows_branch_mismatch_warning_when_present() {
        let rendered = render_fix_confirm(Some("other-branch"));
        assert!(rendered.contains("this worktree is on"));
        assert!(rendered.contains("other-branch"));
    }

    #[test]
    fn fix_confirm_hides_branch_mismatch_warning_when_absent() {
        let rendered = render_fix_confirm(None);
        assert!(!rendered.contains("this worktree is on"));
    }

    /// Render the whole triage pane so overlay stacking order is exercised.
    fn render_pr_review(state: &mut PrReviewState) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let theme = Theme::default();
        let pricing = crate::token_tracking::TokenPricingConfig::default();
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_pr_review(
                    frame,
                    state,
                    &theme,
                    super::PrReviewUsage {
                        cumulative: None,
                        visit: None,
                        pricing: &pricing,
                    },
                    None,
                    &crate::app::ai_review::AiReviewTriageStatus::NotRun,
                    None,
                    None,
                );
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn pane_footer_advertises_v_investigate_and_a_after_one_finishes() {
        use crate::app::pr_review::PrInvestigationStatus;
        use crate::db::pr_investigations::PrInvestigation;

        let mut state = pr_review_state_with_comments(
            vec![pr_comment_of_kind(1, CommentKind::Inline)],
            crate::app::pr_review::PrSortMode::FetchOrder,
        );
        let rendered = render_pr_review(&mut state);
        assert!(
            rendered.contains("v investigate"),
            "footer names the `v` key"
        );
        assert!(
            !rendered.contains("a act"),
            "`a` only appears once an investigation has finished"
        );

        let mut inv =
            PrInvestigation::new_running("p", 1, 1, "sha", crate::project::AgentKind::Codex, "");
        inv.status = PrInvestigationStatus::Complete;
        inv.answer = Some("done".to_string());
        state.investigations.push(inv);
        let rendered = render_pr_review(&mut state);
        assert!(rendered.contains("v investigate · a act"));
    }

    #[test]
    fn fix_target_picker_is_not_occluded_by_a_retained_fix_confirm() {
        let mut state = pr_review_state_with_comments(
            vec![pr_comment_of_kind(1, CommentKind::Inline)],
            crate::app::pr_review::PrSortMode::FetchOrder,
        );
        // `t` from the confirm dialog reopens the picker while the dialog
        // stays in state behind it.
        state.fix_confirm = Some(crate::app::FixConfirmState {
            editor: crate::editor::TextEditor::new("fix this".to_string()),
            editing: false,
            scroll: 0,
            sync_to_cursor: false,
            batch: None,
            reply_draft_requests: Vec::new(),
        });
        state.harness_pick = Some(crate::app::HarnessPickState {
            rows: vec![
                crate::app::pr_review::FixTargetPickRow::ExistingLive(None),
                crate::app::pr_review::FixTargetPickRow::Dedicated(
                    crate::project::AgentKind::Claude,
                ),
            ],
            selected: 0,
            session_name: None,
        });

        let rendered = render_pr_review(&mut state);
        assert!(
            rendered.contains("Run this PR's fixes on:"),
            "the reopened fix-target picker must stay visible"
        );
        assert!(
            !rendered.contains("Inject fix into agent session"),
            "the retained fix-confirm dialog must not paint over the picker"
        );
    }

    fn render_memory_add(scope: crate::app::review_memory::MemoryScope) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let memory_add = crate::app::MemoryAddState {
            comment_id: 1,
            category: 0,
            scope,
            editor: crate::editor::TextEditor::new("Guard shared state".to_string()),
            editing: false,
        };
        let paths = crate::app::review_memory::ReviewMemoryPaths {
            project: std::path::PathBuf::from("/repo/.amf/review-memory.md"),
            global: std::path::PathBuf::from("/home/u/.config/amf/review-memory.md"),
        };
        let theme = Theme::default();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_memory_add_dialog(
                    frame,
                    &memory_add,
                    "alice",
                    Some(paths.for_scope(scope)),
                    &theme,
                )
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn memory_add_dialog_names_the_project_doc_by_default() {
        let rendered = render_memory_add(crate::app::review_memory::MemoryScope::Project);
        assert!(rendered.contains("Add to project memory"));
        assert!(rendered.contains("/repo/.amf/review-memory.md"));
        assert!(rendered.contains("[g] project/global"));
    }

    #[test]
    fn memory_add_dialog_names_the_global_doc_under_global_scope() {
        let rendered = render_memory_add(crate::app::review_memory::MemoryScope::Global);
        assert!(rendered.contains("Add to global memory"));
        assert!(rendered.contains("/home/u/.config/amf/review-memory.md"));
        assert!(
            !rendered.contains("/repo/.amf/review-memory.md"),
            "the project path shouldn't linger once the scope is global"
        );
    }

    fn render_bootstrap_pick(scope: crate::app::review_memory::MemoryScope) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let pick = crate::app::BootstrapPickState { selected: 0, scope };
        let path = match scope {
            crate::app::review_memory::MemoryScope::Project => "/repo/.amf/review-memory.md",
            crate::app::review_memory::MemoryScope::Global => {
                "/home/u/.config/amf/review-memory.md"
            }
        };
        let theme = Theme::default();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_bootstrap_pick(frame, &pick, std::path::Path::new(path), &theme))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn bootstrap_pick_names_the_destination_doc_for_each_scope() {
        let project = render_bootstrap_pick(crate::app::review_memory::MemoryScope::Project);
        assert!(project.contains("project doc"));
        assert!(project.contains("/repo/.amf/review-memory.md"));
        assert!(project.contains("[g] project/global"));

        let global = render_bootstrap_pick(crate::app::review_memory::MemoryScope::Global);
        assert!(global.contains("global doc"));
        assert!(global.contains("/home/u/.config/amf/review-memory.md"));
    }

    #[test]
    fn reply_dialog_discloses_the_posted_via_amf_footer() {
        use ratatui::{Terminal, backend::TestBackend};

        let reply = crate::app::ReplyState {
            comment_id: 1,
            kind: crate::app::pr_review::ReplyKind::Done,
            editor: crate::editor::TextEditor::new("Done in `abc123`.".to_string()),
            agent_drafted: false,
            generation_metadata: None,
            original_seed: "Done in `abc123`.".to_string(),
            editing: false,
        };
        let theme = Theme::default();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_reply_dialog(frame, &reply, "alice", &theme))
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(rendered.contains("posted via AMF"));
    }

    #[test]
    fn agent_drafted_reply_dialog_discloses_ai_attribution() {
        use ratatui::{Terminal, backend::TestBackend};

        let reply = crate::app::ReplyState {
            comment_id: 1,
            kind: crate::app::pr_review::ReplyKind::Done,
            editor: crate::editor::TextEditor::new(
                "Fixed the guard.\n\nDone in `abc123`.".to_string(),
            ),
            agent_drafted: true,
            generation_metadata: Some(crate::app::pr_review::ReplyGenerationMetadata {
                harness: Some("Codex".to_string()),
                model: Some("gpt-5.5".to_string()),
                estimated_tokens: Some(1_500),
                estimated_cost: Some("$0.04".to_string()),
                combined_batch: None,
            }),
            original_seed: "Fixed the guard.\n\nDone in `abc123`.".to_string(),
            editing: false,
        };
        let theme = Theme::default();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_reply_dialog(frame, &reply, "alice", &theme))
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(rendered.contains("drafted by AI via AMF"));
        assert!(rendered.contains("AI generation: harness Codex"));
        assert!(rendered.contains("model gpt-5.5"));
        assert!(rendered.contains("estimated tokens ~1.5k"));
        assert!(rendered.contains("Fix cost (est.): $0.04"));
        assert!(!rendered.contains("posted via AMF"));
    }

    /// The disclosure previews what will be posted, so it has to be readable in
    /// full. At a narrow width a provider-qualified model name runs well past
    /// the dialog; it must wrap onto another row rather than being cut off.
    #[test]
    fn agent_drafted_reply_dialog_wraps_a_long_disclosure() {
        use ratatui::{Terminal, backend::TestBackend};

        let reply = crate::app::ReplyState {
            comment_id: 1,
            kind: crate::app::pr_review::ReplyKind::Done,
            editor: crate::editor::TextEditor::new("Fixed the guard.".to_string()),
            agent_drafted: true,
            generation_metadata: Some(crate::app::pr_review::ReplyGenerationMetadata {
                harness: Some("Opencode".to_string()),
                model: Some("anthropic/claude-opus-4-5-20251101".to_string()),
                estimated_tokens: Some(1_500),
                estimated_cost: Some("$0.04".to_string()),
                combined_batch: None,
            }),
            original_seed: "Fixed the guard.".to_string(),
            editing: false,
        };
        let theme = Theme::default();
        let backend = TestBackend::new(60, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_reply_dialog(frame, &reply, "alice", &theme))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let rows: Vec<String> = (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect();
        let has = |needle: &str| rows.iter().any(|row| row.contains(needle));

        // None of the three lines fits on one row at this width, so finding
        // each one's tail proves it wrapped rather than being clipped.
        assert!(has("AI generation: harness Opencode · model"));
        assert!(has("anthropic/claude-opus-4-5-20251101"));
        assert!(has("estimated tokens ~1.5k · Fix cost"));
        assert!(has("(est.): $0.04"));
        assert!(has("will post with a \"— drafted by AI via"));
        assert!(has("AMF\" footer"));
        // The reply body and the key hints keep their rows.
        assert!(has("Fixed the guard."));
        assert!(has("[⏎] post"));
    }

    fn render_harness_pick(existing_live_label: Option<String>) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let pick = crate::app::HarnessPickState {
            rows: vec![
                crate::app::pr_review::FixTargetPickRow::ExistingLive(existing_live_label),
                crate::app::pr_review::FixTargetPickRow::Dedicated(
                    crate::project::AgentKind::Claude,
                ),
            ],
            selected: 0,
            session_name: None,
        };
        let theme = Theme::default();
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_harness_pick(frame, &pick, &theme))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn harness_pick_names_the_existing_session_when_resolved() {
        let rendered = render_harness_pick(Some("Claude 2".to_string()));
        assert!(rendered.contains("Existing live session (Claude 2)"));
    }

    #[test]
    fn investigation_action_menu_lists_every_choice() {
        use ratatui::{Terminal, backend::TestBackend};
        let pick = crate::app::InvestigationActionPick {
            comment_id: 1,
            selected: 0,
        };
        let theme = Theme::default();
        // A deliberately short terminal — the menu must still show all six rows.
        let backend = TestBackend::new(100, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_investigation_action_pick(frame, &pick, &theme))
            .unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        for action in crate::app::InvestigationAction::ALL {
            assert!(
                rendered.contains(action.label()),
                "menu is missing {:?}",
                action
            );
        }
    }

    #[test]
    fn harness_pick_falls_back_to_generic_label_when_unresolved() {
        let rendered = render_harness_pick(None);
        assert!(rendered.contains("Existing live session"));
        assert!(!rendered.contains("Existing live session ("));
    }

    fn pr_comment_of_kind(id: u64, kind: CommentKind) -> PrComment {
        PrComment {
            id,
            kind,
            author: "someone".into(),
            is_bot: false,
            path: None,
            line: None,
            side: None,
            outdated: false,
            file_level: false,
            diff_hunk: None,
            body: format!("comment {id}"),
            snippet: format!("comment {id}"),
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: crate::app::pr_review::TriageState::Untriaged,
            local_note: None,
            batch_id: None,
            github_id: None,
            github_review_id: None,
        }
    }

    fn pr_review_state_with_comments(
        comments: Vec<PrComment>,
        sort_mode: crate::app::pr_review::PrSortMode,
    ) -> PrReviewState {
        let pr = crate::github::PrRef {
            number: 1,
            head_sha: "sha".into(),
            url: "https://github.com/o/r/pull/1".into(),
            owner: "o".into(),
            repo: "r".into(),
            head_ref: "main".into(),
        };
        PrReviewState {
            workdir: std::path::PathBuf::from("/tmp/wd"),
            review: crate::app::pr_review::PrReview {
                pr,
                comments,
                fetched_at: chrono::Local::now(),
            },
            selected: 0,
            detail_scroll: 0,
            detail_content_lines: 0,
            hide_resolved: false,
            sort_mode,
            fix_target: crate::app::pr_review::FixTarget::default(),
            fix_target_picked: false,
            usage_baselines: std::collections::HashMap::new(),
            review_harness: None,
            dedicated_session_label: crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
            harness_pick: None,
            new_feature_setup: None,
            integrate: None,
            fix_confirm: None,
            fix_vim_enabled: false,
            mark_pick: None,
            reply_kind_pick: None,
            reply: None,
            memory_add: None,
            marked: std::collections::HashSet::new(),
            pending_batch: false,
            checked_out_branch: Some("main".to_string()),
            pending_ai_review_findings: 0,
            ai_review_last_run: None,
            investigations: Vec::new(),
            investigation_harness_pick: None,
            investigation_action_pick: None,
            investigation_follow_up: None,
            pending_follow_up: None,
        }
    }

    fn render_comment_list(state: &PrReviewState) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let theme = Theme::default();
        let backend = TestBackend::new(60, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_comment_list(frame, frame.area(), state, &theme))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn comment_list_shows_conversation_divider_under_conversations_sort() {
        let comments = vec![
            pr_comment_of_kind(1, CommentKind::Inline),
            pr_comment_of_kind(2, CommentKind::Conversation),
        ];
        let state = pr_review_state_with_comments(
            comments,
            crate::app::pr_review::PrSortMode::Conversations,
        );
        let rendered = render_comment_list(&state);
        assert!(rendered.contains("Conversation"));
    }

    #[test]
    fn comment_list_hides_conversation_divider_under_other_sort_modes() {
        let comments = vec![
            pr_comment_of_kind(1, CommentKind::Inline),
            pr_comment_of_kind(2, CommentKind::Conversation),
        ];
        let state =
            pr_review_state_with_comments(comments, crate::app::pr_review::PrSortMode::FetchOrder);
        let rendered = render_comment_list(&state);
        assert!(!rendered.contains("Conversation"));
    }

    #[test]
    fn comment_list_collates_amf_reply_and_labels_standalone_finding() {
        let mut root = pr_comment_of_kind(1, CommentKind::Inline);
        root.author = "reviewer".into();

        let mut amf_reply = pr_comment_of_kind(2, CommentKind::Inline);
        amf_reply.author = "amf-reply".into();
        amf_reply.in_reply_to = Some(1);
        amf_reply.body = "Done.\n\n— posted via AMF".into();

        let mut ai_finding = pr_comment_of_kind(3, CommentKind::Inline);
        ai_finding.author = "ai-review".into();
        ai_finding.body = "Finding.\n\n— AI review via AMF".into();

        let mut human_reply = pr_comment_of_kind(4, CommentKind::Inline);
        human_reply.author = "human-reply".into();
        human_reply.in_reply_to = Some(1);
        human_reply.body = "Please also add a test.".into();

        let state = pr_review_state_with_comments(
            vec![root, amf_reply, ai_finding, human_reply],
            crate::app::pr_review::PrSortMode::FetchOrder,
        );
        let rendered = render_comment_list(&state);

        assert!(rendered.contains("@reviewer"));
        assert!(!rendered.contains("@amf-reply"));
        assert!(rendered.contains("@ai-review"));
        assert!(rendered.contains("[via AMF]"));
        assert!(rendered.contains("@human-reply"));
    }

    fn render_comment_detail(comment: &PrComment, all_comments: &[PrComment]) -> String {
        render_comment_detail_with_investigation(comment, all_comments, None)
    }

    fn render_comment_detail_with_investigation(
        comment: &PrComment,
        all_comments: &[PrComment],
        investigation: Option<&crate::db::pr_investigations::PrInvestigation>,
    ) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let theme = Theme::default();
        let backend = TestBackend::new(80, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_comment_detail(
                    frame,
                    frame.area(),
                    Some(comment),
                    all_comments,
                    investigation,
                    0,
                    &theme,
                );
            })
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn sample_investigation(comment_id: u64) -> crate::db::pr_investigations::PrInvestigation {
        use crate::app::pr_review::PrInvestigationStatus;
        let mut inv = crate::db::pr_investigations::PrInvestigation::new_running(
            "proj",
            7,
            comment_id,
            "sha",
            crate::project::AgentKind::Codex,
            "context",
        );
        inv.status = PrInvestigationStatus::Complete;
        inv.answer = Some("The concern is valid: the guard misses x < 0.".to_string());
        inv.created_at = "2026-09-02 11:22:33.456".to_string();
        inv.updated_at = "2026-09-02 11:22:33.456".to_string();
        inv
    }

    #[test]
    fn detail_pane_shows_a_completed_investigation_with_harness_and_time() {
        let comment = pr_comment_of_kind(1, CommentKind::Inline);
        let inv = sample_investigation(1);
        let rendered = render_comment_detail_with_investigation(
            &comment,
            std::slice::from_ref(&comment),
            Some(&inv),
        );
        assert!(rendered.contains("Investigation (read-only)"));
        assert!(rendered.contains("complete"));
        assert!(rendered.contains("Codex"));
        assert!(rendered.contains("2026-09-02 11:22:33"));
        assert!(!rendered.contains(".456"));
        assert!(rendered.contains("The concern is valid"));
    }

    #[test]
    fn detail_pane_shows_a_failed_investigation_error_not_a_frozen_spinner() {
        use crate::app::pr_review::PrInvestigationStatus;
        let comment = pr_comment_of_kind(2, CommentKind::Inline);
        let mut inv = sample_investigation(2);
        inv.status = PrInvestigationStatus::Failed;
        inv.answer = None;
        inv.error = Some("Codex couldn't finish: timed out".to_string());
        let rendered = render_comment_detail_with_investigation(
            &comment,
            std::slice::from_ref(&comment),
            Some(&inv),
        );
        assert!(rendered.contains("failed"));
        assert!(rendered.contains("timed out"));
        assert!(!rendered.contains("thinking"));
    }

    #[test]
    fn detail_pane_has_no_investigation_section_without_one() {
        let comment = pr_comment_of_kind(3, CommentKind::Inline);
        let rendered = render_comment_detail(&comment, std::slice::from_ref(&comment));
        assert!(!rendered.contains("Investigation (read-only)"));
    }

    #[test]
    fn detail_pane_clears_content_already_drawn_in_its_area() {
        use ratatui::{Terminal, backend::TestBackend};

        let theme = Theme::default();
        let summary = pr_comment_of_kind(
            1,
            CommentKind::ReviewSummary {
                state: "COMMENTED".into(),
            },
        );
        let backend = TestBackend::new(80, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                // Model cells left by a previously rendered diff/source pane
                // in the same frame. The replacement summary is deliberately
                // short, so these rows survive unless the detail pane owns and
                // clears its complete area before drawing.
                frame.render_widget(
                    Paragraph::new(vec![
                        Line::from("stale source fragment"),
                        Line::from("doneCursor, oldPid, cfg)"),
                        Line::from("hyperlink(url)"),
                    ]),
                    frame.area(),
                );
                draw_comment_detail(
                    frame,
                    frame.area(),
                    Some(&summary),
                    std::slice::from_ref(&summary),
                    None,
                    0,
                    &theme,
                );
            })
            .unwrap();

        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(rendered.contains("comment 1"));
        assert!(!rendered.contains("stale source fragment"));
        assert!(!rendered.contains("doneCursor"));
        assert!(!rendered.contains("hyperlink(url)"));
    }

    /// Draw the detail pane into a pane far taller than its content and return
    /// `(reported, painted)`: the row count `draw_comment_detail` hands back for
    /// scroll clamping, and the true rendered height read off the terminal
    /// buffer (index of the last row inside the border that got any ink, plus
    /// one). The second value comes from `Paragraph`'s own output, so equality
    /// pins the clamp to the real renderer rather than to a second opinion
    /// about how wrapping works.
    fn detail_rows_reported_and_painted(comment: &PrComment, width: u16) -> (usize, usize) {
        use ratatui::{Terminal, backend::TestBackend};

        const HEIGHT: u16 = 120;
        let theme = Theme::default();
        let backend = TestBackend::new(width, HEIGHT);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut reported = 0;
        terminal
            .draw(|frame| {
                reported = draw_comment_detail(
                    frame,
                    frame.area(),
                    Some(comment),
                    std::slice::from_ref(comment),
                    None,
                    0,
                    &theme,
                );
            })
            .unwrap();

        let buf = terminal.backend().buffer().clone();
        let mut painted = 0;
        // Row/col 0 and the last row/col are the pane border; content is inset.
        for y in 1..HEIGHT - 1 {
            if (1..width - 1).any(|x| !buf[(x, y)].symbol().trim().is_empty()) {
                painted = y as usize;
            }
        }
        assert!(
            painted < HEIGHT as usize - 2,
            "fixture must fit the test pane so `painted` is a real height, not a truncation"
        );
        (reported, painted)
    }

    #[test]
    fn detail_pane_row_count_matches_the_rows_paragraph_paints() {
        // A word far wider than the pane, a whitespace run several pane-widths
        // long, and ordinary prose that wraps on word boundaries — the three
        // shapes where a row count can drift from what `Wrap { trim: false }`
        // actually lays out.
        let cases: [(&str, String); 3] = [
            ("long unbroken word", format!("+{}", "x".repeat(100))),
            ("whitespace run", format!("+{}", " ".repeat(100))),
            (
                "wrapped prose",
                format!("+{}", "lorem ipsum dolor ".repeat(12)),
            ),
        ];
        for (label, hunk) in cases {
            let mut comment = pr_comment_of_kind(1, CommentKind::Inline);
            comment.diff_hunk = Some(hunk);
            for width in [20, 30, 47] {
                let (reported, painted) = detail_rows_reported_and_painted(&comment, width);
                assert_eq!(
                    reported, painted,
                    "{label} at pane width {width}: scroll clamp must match rendered height"
                );
            }
        }
    }

    #[test]
    fn detail_pane_row_count_grows_by_the_hunks_wrapped_height() {
        // Pane width 30 leaves 28 inner columns. A 101-column hunk line
        // ("+" plus 100 x's) occupies ceil(101 / 28) = 4 rows, and the hunk
        // section also adds a divider and a "Diff hunk" label: 6 rows over the
        // same comment with no hunk at all.
        let bare = pr_comment_of_kind(1, CommentKind::Inline);
        let mut with_hunk = bare.clone();
        with_hunk.diff_hunk = Some(format!("+{}", "x".repeat(100)));

        let (bare_rows, _) = detail_rows_reported_and_painted(&bare, 30);
        let (hunk_rows, _) = detail_rows_reported_and_painted(&with_hunk, 30);
        assert_eq!(hunk_rows - bare_rows, 6);
    }

    #[test]
    fn detail_pane_shows_no_replies_section_when_thread_has_no_replies() {
        let root = pr_comment_of_kind(1, CommentKind::Inline);
        let rendered = render_comment_detail(&root, std::slice::from_ref(&root));
        // "↳ @" is the reply marker itself (see draw_comment_detail), so this
        // can't false-pass on a renamed section header or false-fail on a
        // comment body that happens to contain the word "Replies".
        assert!(!rendered.contains("↳ @"));
    }

    #[test]
    fn detail_pane_surfaces_a_reply_posted_via_amf() {
        let root = pr_comment_of_kind(1, CommentKind::Inline);
        let mut reply = pr_comment_of_kind(2, CommentKind::Inline);
        reply.author = "amf-user".into();
        reply.in_reply_to = Some(1);
        reply.body = "Done in `abc123`.\n\n— posted via AMF".into();
        reply.snippet = "Done in `abc123`.".into();
        let all = vec![root.clone(), reply];

        let rendered = render_comment_detail(&root, &all);
        assert!(rendered.contains("↳ @amf-user"));
        assert!(rendered.contains("via AMF"));
        assert!(rendered.contains("Done in `abc123`."));
    }

    #[test]
    fn detail_pane_surfaces_a_reply_posted_outside_amf_without_the_via_amf_chip() {
        let root = pr_comment_of_kind(1, CommentKind::Inline);
        let mut reply = pr_comment_of_kind(2, CommentKind::Inline);
        reply.author = "headless-agent".into();
        reply.in_reply_to = Some(1);
        reply.body = "Done in `abc123`.".into();
        reply.snippet = "Done in `abc123`.".into();
        let all = vec![root.clone(), reply];

        let rendered = render_comment_detail(&root, &all);
        assert!(rendered.contains("↳ @headless-agent"));
        assert!(!rendered.contains("via AMF"));
    }

    #[test]
    fn detail_pane_marks_top_level_amf_comment_as_actionable_attribution() {
        let mut finding = pr_comment_of_kind(1, CommentKind::Inline);
        finding.body = "Finding.\n\n— AI review via AMF".into();

        let rendered = render_comment_detail(&finding, std::slice::from_ref(&finding));
        assert!(rendered.contains("via AMF"));
        assert!(!rendered.contains("context only"));
    }

    #[test]
    fn detail_pane_shows_resolved_and_outdated_chips_next_to_the_reply() {
        let root = pr_comment_of_kind(1, CommentKind::Inline);
        let mut reply = pr_comment_of_kind(2, CommentKind::Inline);
        reply.author = "bob".into();
        reply.in_reply_to = Some(1);
        reply.is_resolved = true;
        reply.outdated = true;
        let all = vec![root.clone(), reply];

        let rendered = render_comment_detail(&root, &all);
        assert!(rendered.contains("resolved"));
        assert!(rendered.contains("outdated"));
    }
}
