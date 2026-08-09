//! Rendering for the Learning Mode overlay
//! (`docs/backlog/learning-mode-plan.md`).
//!
//! Three panes in the Final Review idiom — file list, file/diff content, Q&A
//! history — under a header that always says what the mode is doing and that
//! it is read-only, over a footer that spells its keys out in words rather
//! than a glyph legend. The user this is drawn for has never seen this
//! codebase, so nothing here is left to be inferred from an icon.
//!
//! Modal layers are drawn last, in the same precedence the key handler uses
//! (`crate::handlers::learning`): answer pane, question prompt, the two
//! pickers, then help on top.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};
use std::path::Path;

use super::super::dashboard::centered_rect;
use super::editor_view::{count_wrapped_editor_lines, editor_lines, sync_editor_scroll};
use crate::app::learning::STARTER_QUESTIONS;
use crate::app::{
    BrowseScope, LearningAnchor, LearningFocus, LearningHarnessPicker, LearningListEntry,
    LearningListGroup, LearningQa, LearningQaIntent, LearningQaStatus, LearningQuestionEditor,
    LearningStarterPicker, LearningViewState,
};
use crate::highlight;
use crate::theme::Theme;

/// Above this many lines a file is rendered without syntax highlighting.
/// Highlighting a file re-clones its whole span list every frame, which is a
/// bad trade on something too big to read anyway.
const MAX_HIGHLIGHT_LINES: usize = 4_000;

/// Rows a single Q&A entry occupies in the history pane (headline + question).
const QA_ROW_HEIGHT: usize = 2;

/// Full-screen Learning Mode overlay.
pub fn draw_learning_view(frame: &mut Frame, state: &mut LearningViewState, theme: &Theme) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }

    frame.render_widget(
        Block::default().style(Style::default().bg(theme.effective_bg())),
        area,
    );

    let has_error = state.error.is_some();
    let mut constraints = vec![Constraint::Length(2), Constraint::Min(3)];
    if has_error {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(2));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    draw_header(frame, chunks[0], state, theme);

    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(24),
            Constraint::Min(20),
            Constraint::Percentage(30),
        ])
        .split(chunks[1]);
    draw_file_list(frame, panes[0], state, theme);
    draw_content(frame, panes[1], state, theme);
    draw_qa_list(frame, panes[2], state, theme);

    let mut idx = 2;
    if has_error {
        draw_error(frame, chunks[idx], state, theme);
        idx += 1;
    }
    draw_footer(frame, chunks[idx], state, theme);

    // Modal layers, bottom of the stack first.
    if state.answer_open {
        draw_answer(frame, state, theme);
    }
    if state.question.is_some() {
        draw_question(frame, state, theme);
    }
    if let Some(picker) = &state.starter_picker {
        draw_starter_picker(frame, picker, theme);
    }
    if let Some(picker) = &state.harness_picker {
        draw_harness_picker(frame, picker, theme);
    }
    if state.help_open {
        draw_help(frame, state, theme);
    }
}

// ── header / footer ──────────────────────────────────────────

fn draw_header(frame: &mut Frame, area: Rect, state: &LearningViewState, theme: &Theme) {
    let title = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "Learning Mode",
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} / {}", state.project_name, state.feature_name),
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::raw("   "),
        Span::styled(
            "read-only",
            Style::default()
                .fg(theme.success.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " — asking questions here never changes your files",
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]);

    let mut settings = vec![
        Span::raw("  "),
        Span::styled(
            "Showing: ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            state.scope.description(),
            Style::default().fg(theme.info.to_color()),
        ),
        Span::styled(" (s)   ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled(
            "Explaining for: ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            state.level.as_str(),
            Style::default().fg(theme.info.to_color()),
        ),
        Span::styled(" (L)   ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled("Agent: ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled(
            state.harness.display_name().to_string(),
            Style::default().fg(theme.info.to_color()),
        ),
        Span::styled(" (m)", Style::default().fg(theme.text_muted.to_color())),
    ];
    let in_flight = state.in_flight_count();
    if in_flight > 0 {
        settings.push(Span::styled(
            format!(
                "   {} answer{} still generating",
                in_flight,
                if in_flight == 1 { "" } else { "s" }
            ),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ));
    }

    frame.render_widget(Paragraph::new(vec![title, Line::from(settings)]), area);
}

fn draw_error(frame: &mut Frame, area: Rect, state: &LearningViewState, theme: &Theme) {
    let Some(message) = state.error.as_deref() else {
        return;
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {message}"),
            Style::default().fg(theme.danger.to_color()),
        ))),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, state: &LearningViewState, theme: &Theme) {
    let key = Style::default().fg(theme.warning.to_color());
    let word = Style::default().fg(theme.text_muted.to_color());

    let mut first = vec![
        Span::raw("  "),
        Span::styled("Tab", key),
        Span::styled(" next pane  ", word),
        Span::styled("j/k", key),
        Span::styled(" move  ", word),
        Span::styled("Enter", key),
        Span::styled(
            match state.focus {
                LearningFocus::Qa => " read the answer  ",
                _ => " open  ",
            },
            word,
        ),
        Span::styled("e", key),
        Span::styled(" explain this to me  ", word),
        Span::styled("c", key),
        Span::styled(" ask for a change  ", word),
        Span::styled("t", key),
        Span::styled(" starter questions", word),
    ];
    // Only offered once there is an answer to continue from, so the key isn't
    // advertised before it can do anything.
    if state.qa.iter().any(|qa| qa.answer.is_some()) {
        first.push(Span::styled("  F", key));
        first.push(Span::styled(" ask a follow-up", word));
    }
    if state.question.is_none() && !state.qa.is_empty() {
        first.push(Span::styled(
            format!("   ({} asked)", state.qa.len()),
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }

    let second = vec![
        Span::raw("  "),
        Span::styled("v", key),
        Span::styled(" start a line range  ", word),
        Span::styled("V", key),
        Span::styled(" clear it  ", word),
        Span::styled("f", key),
        Span::styled(" whole file  ", word),
        Span::styled("P", key),
        Span::styled(" whole project  ", word),
        Span::styled("x", key),
        Span::styled(" this change  ", word),
        Span::styled("z", key),
        Span::styled(" fold Start here  ", word),
        Span::styled("?", key),
        Span::styled(" help  ", word),
        Span::styled("q", key),
        Span::styled(" close", word),
    ];

    frame.render_widget(
        Paragraph::new(vec![Line::from(first), Line::from(second)]),
        area,
    );
}

// ── file list ────────────────────────────────────────────────

fn draw_file_list(frame: &mut Frame, area: Rect, state: &mut LearningViewState, theme: &Theme) {
    let focused = state.focus == LearningFocus::FileList;
    let block = pane_block(" Files ", focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    if state.entries.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                " Nothing to browse here.",
                Style::default().fg(theme.text_muted.to_color()),
            )),
            inner,
        );
        return;
    }

    let visible = inner.height as usize;
    state.list_scroll = keep_in_view(state.list_scroll, state.selected_entry, visible);

    let width = inner.width as usize;
    let lines: Vec<Line> = state
        .entries
        .iter()
        .enumerate()
        .skip(state.list_scroll)
        .take(visible)
        .map(|(i, entry)| {
            file_row(
                entry,
                i == state.selected_entry,
                state.start_here_collapsed,
                width,
                theme,
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);

    draw_scrollbar(
        frame,
        inner,
        state.entries.len(),
        state.list_scroll,
        visible,
    );
}

fn file_row(
    entry: &LearningListEntry,
    selected: bool,
    collapsed: bool,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let (text, color) = match entry {
        LearningListEntry::StartHereHeader => (
            format!("{} Start here", if collapsed { "▸" } else { "▾" }),
            theme.warning.to_color(),
        ),
        LearningListEntry::ProjectTour => (
            "  Tour this whole project".to_string(),
            theme.info.to_color(),
        ),
        LearningListEntry::File {
            path,
            group: LearningListGroup::StartHere,
            ..
        } => (format!("  {path}"), theme.text.to_color()),
        LearningListEntry::File { path, .. } => (path.clone(), theme.text.to_color()),
    };

    let cursor = if selected { "› " } else { "  " };
    let body = truncate_left(&text, width.saturating_sub(cursor.len()));
    let style = if selected {
        Style::default()
            .fg(color)
            .bg(theme.effective_selection_bg())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(color)
    };
    Line::from(vec![
        Span::styled(cursor, Style::default().fg(theme.primary.to_color())),
        Span::styled(body, style),
    ])
}

// ── content pane ─────────────────────────────────────────────

fn draw_content(frame: &mut Frame, area: Rect, state: &mut LearningViewState, theme: &Theme) {
    let focused = state.focus == LearningFocus::Content;
    let title = match (&state.content_path, state.anchor) {
        (_, LearningAnchor::Project) => " This whole project ".to_string(),
        (Some(path), _) => format!(" {path} "),
        (None, _) => " No file selected ".to_string(),
    };
    let block = pane_block(&title, focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // The anchor, spelled out, so the user always knows what a question would
    // be about without reading the highlight.
    let anchor_area = Rect::new(inner.x, inner.y, inner.width, 1);
    let body_area = Rect::new(
        inner.x,
        inner.y.saturating_add(1),
        inner.width,
        inner.height.saturating_sub(1),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " Asking about: ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled(
                state.anchor.describe(state.content_path.as_deref()),
                Style::default()
                    .fg(theme.info.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        anchor_area,
    );
    if body_area.height == 0 {
        return;
    }

    if let Some(reason) = state.content_error.clone() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {reason}"),
                Style::default().fg(theme.text_muted.to_color()),
            ))
            .wrap(Wrap { trim: true }),
            body_area,
        );
        return;
    }

    let rows = build_content_rows(state, theme);
    if rows.is_empty() {
        let hint = match state.anchor {
            LearningAnchor::Project => {
                "Nothing to show for the project as a whole — press e to ask about it, or t for a starter question."
            }
            _ => "This file has nothing to show.",
        };
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {hint}"),
                Style::default().fg(theme.text_muted.to_color()),
            ))
            .wrap(Wrap { trim: true }),
            body_area,
        );
        return;
    }

    let visible = body_area.height as usize;
    state.content_scroll = keep_in_view(state.content_scroll, state.cursor_line, visible);
    let shown: Vec<Line> = rows
        .into_iter()
        .skip(state.content_scroll)
        .take(visible)
        .collect();
    frame.render_widget(Paragraph::new(shown), body_area);

    draw_scrollbar(
        frame,
        body_area,
        state.selectable_line_count(),
        state.content_scroll,
        visible,
    );
}

/// One rendered row per selectable content line, with its line number, the
/// selection highlight, and syntax colours where they're available.
fn build_content_rows(state: &LearningViewState, theme: &Theme) -> Vec<Line<'static>> {
    let (start, end) = state.selected_span();
    let selected_range = !matches!(state.anchor, LearningAnchor::Project | LearningAnchor::File);

    match state.scope {
        BrowseScope::RepoTree => {
            let highlighted = highlight_content(state);
            state
                .content
                .iter()
                .enumerate()
                .map(|(i, raw)| {
                    let spans = highlighted
                        .as_ref()
                        .and_then(|text| text.lines.get(i))
                        .map(|line| {
                            line.spans
                                .iter()
                                .map(|span| {
                                    Span::styled(
                                        span.text.clone(),
                                        highlight::style_for_class(span.class, theme),
                                    )
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_else(|| {
                            vec![Span::styled(
                                raw.clone(),
                                Style::default().fg(theme.text.to_color()),
                            )]
                        });
                    content_row(
                        format!("{:>5} ", i + 1),
                        spans,
                        selected_range && i >= start && i <= end,
                        i == state.cursor_line,
                        theme,
                    )
                })
                .collect()
        }
        BrowseScope::BranchChanges => {
            let Some(file) = state.selected_diff_file() else {
                return Vec::new();
            };
            let locations = file.addressable_lines();
            let texts = file.addressable_line_texts();
            locations
                .iter()
                .enumerate()
                .map(|(i, location)| {
                    let text = texts.get(i).cloned().unwrap_or_default();
                    let (marker, color) = match (location.old_line, location.new_line) {
                        (None, Some(_)) => ("+", theme.success.to_color()),
                        (Some(_), None) => ("-", theme.danger.to_color()),
                        _ => (" ", theme.text.to_color()),
                    };
                    let number = location.new_line.or(location.old_line).unwrap_or(0);
                    content_row(
                        format!("{number:>5} {marker}"),
                        vec![Span::styled(text, Style::default().fg(color))],
                        selected_range && i >= start && i <= end,
                        i == state.cursor_line,
                        theme,
                    )
                })
                .collect()
        }
    }
}

fn content_row(
    gutter: String,
    body: Vec<Span<'static>>,
    in_selection: bool,
    is_cursor: bool,
    theme: &Theme,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        gutter,
        Style::default().fg(if is_cursor {
            theme.primary.to_color()
        } else {
            theme.text_muted.to_color()
        }),
    )];
    if in_selection {
        // Repaint the row onto the selection background, keeping the syntax
        // foregrounds so highlighted code stays readable inside a range.
        spans.extend(body.into_iter().map(|span| {
            let style = span.style.bg(theme.effective_selection_bg());
            Span::styled(span.content, style)
        }));
    } else {
        spans.extend(body);
    }
    Line::from(spans)
}

fn highlight_content(state: &LearningViewState) -> Option<highlight::HighlightedText> {
    let path = state.content_path.as_deref()?;
    if state.content.len() > MAX_HIGHLIGHT_LINES {
        return None;
    }
    let source = state.content.join("\n");
    Some(highlight::highlight_source(highlight::HighlightRequest {
        path: Some(Path::new(path)),
        language_hint: None,
        source: &source,
    }))
}

// ── Q&A history ──────────────────────────────────────────────

fn draw_qa_list(frame: &mut Frame, area: Rect, state: &mut LearningViewState, theme: &Theme) {
    let focused = state.focus == LearningFocus::Qa;
    let block = pane_block(" Questions ", focused, theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    if state.qa.is_empty() {
        let lines = vec![
            Line::from(Span::styled(
                " No questions yet.",
                Style::default().fg(theme.text_muted.to_color()),
            )),
            Line::from(""),
            Line::from(Span::styled(
                " Press e to have something explained,",
                Style::default().fg(theme.text_muted.to_color()),
            )),
            Line::from(Span::styled(
                " or t to pick a starter question.",
                Style::default().fg(theme.text_muted.to_color()),
            )),
            Line::from(""),
            Line::from(Span::styled(
                " No question is too basic.",
                Style::default().fg(theme.text_muted.to_color()),
            )),
        ];
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    }

    let visible_entries = (inner.height as usize) / QA_ROW_HEIGHT;
    if visible_entries == 0 {
        return;
    }
    state.qa_scroll = keep_in_view(state.qa_scroll, state.selected_qa, visible_entries);

    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();
    for (i, qa) in state
        .qa
        .iter()
        .enumerate()
        .skip(state.qa_scroll)
        .take(visible_entries)
    {
        let selected = i == state.selected_qa;
        let indented = qa.parent_qa_id.is_some();
        lines.push(qa_headline(qa, selected, indented, theme));
        lines.push(qa_question_line(qa, selected, indented, width, theme));
    }
    frame.render_widget(Paragraph::new(lines), inner);

    draw_scrollbar(
        frame,
        inner,
        state.qa.len(),
        state.qa_scroll,
        visible_entries,
    );
}

fn qa_headline(qa: &LearningQa, selected: bool, indented: bool, theme: &Theme) -> Line<'static> {
    let intent_color = match qa.intent {
        LearningQaIntent::Explain => theme.info.to_color(),
        LearningQaIntent::Action => theme.warning.to_color(),
    };
    let status_color = match qa.status {
        LearningQaStatus::Answered => theme.success.to_color(),
        LearningQaStatus::Failed => theme.danger.to_color(),
        _ => theme.warning.to_color(),
    };

    let mut spans = vec![
        Span::styled(
            if selected { "› " } else { "  " },
            Style::default().fg(theme.primary.to_color()),
        ),
        Span::styled(
            if indented { "└ " } else { "" },
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            qa.intent.marker(),
            Style::default()
                .fg(intent_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  ", Style::default()),
        Span::styled(qa.status.word(), Style::default().fg(status_color)),
    ];
    // Markers before provenance: this pane is narrow enough that the headline
    // overflows, and the renderer truncates from the right. "You already acted
    // on this" is worth more than which harness answered, so the harness is
    // what gets dropped.
    if qa.todo_id.is_some() {
        spans.push(Span::styled(
            "  → TODO",
            Style::default().fg(theme.success.to_color()),
        ));
    }
    if qa.spawned_session_id.is_some() {
        spans.push(Span::styled(
            "  → session",
            Style::default().fg(theme.success.to_color()),
        ));
    }
    spans.push(Span::styled(
        format!(
            "  {} · {}",
            qa.harness.display_name(),
            qa.run_mode.description()
        ),
        Style::default().fg(theme.text_muted.to_color()),
    ));
    Line::from(spans)
}

fn qa_question_line(
    qa: &LearningQa,
    selected: bool,
    indented: bool,
    width: usize,
    theme: &Theme,
) -> Line<'static> {
    let indent = if indented { "      " } else { "    " };
    let text = first_line(&qa.question);
    let style = if selected {
        Style::default()
            .fg(theme.text.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text.to_color())
    };
    Line::from(vec![
        Span::raw(indent),
        Span::styled(
            truncate_right(&text, width.saturating_sub(indent.len())),
            style,
        ),
    ])
}

// ── answer pane ──────────────────────────────────────────────

fn draw_answer(frame: &mut Frame, state: &mut LearningViewState, theme: &Theme) {
    let Some(qa) = state.qa.get(state.selected_qa).cloned() else {
        return;
    };
    let area = centered_rect(86, 86, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(format!(
            " {} · {} ",
            qa.intent.label(),
            qa.anchor.describe(qa.file_path.as_deref())
        ))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_header_bg()))
        .border_style(
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 5 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let header = Paragraph::new(vec![
        Line::from(Span::styled(
            qa.question.clone(),
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!(
                "answered by {} · {} · written for a {} reader · {}",
                qa.harness.display_name(),
                qa.run_mode.description(),
                qa.level.as_str(),
                qa.status.word()
            ),
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ])
    .style(Style::default().bg(theme.effective_header_bg()))
    .wrap(Wrap { trim: true });
    frame.render_widget(header, chunks[0]);

    match (&qa.answer, &qa.error) {
        (Some(answer), _) => {
            let source = qa
                .file_path
                .clone()
                .unwrap_or_else(|| "answer.md".to_string());
            super::markdown::draw_markdown_document(
                frame,
                chunks[1],
                answer,
                Path::new(&source),
                &mut state.answer_scroll,
                &mut state.answer_rendered_width,
                &mut state.answer_rendered_lines,
                theme,
            );
        }
        (None, Some(error)) => {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    error.clone(),
                    Style::default().fg(theme.danger.to_color()),
                ))
                .style(Style::default().bg(theme.effective_header_bg()))
                .wrap(Wrap { trim: true }),
                chunks[1],
            );
        }
        (None, None) => {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    "Still generating — this stays interactive, so you can keep browsing.",
                    Style::default().fg(theme.text_muted.to_color()),
                ))
                .style(Style::default().bg(theme.effective_header_bg()))
                .wrap(Wrap { trim: true }),
                chunks[1],
            );
        }
    }

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("j/k", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " scroll  ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("PgUp/PgDn", Style::default().fg(theme.warning.to_color())),
            Span::styled(" page  ", Style::default().fg(theme.text_muted.to_color())),
            Span::styled("g/G", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " top/bottom  ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("F", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " ask a follow-up  ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " back to browsing",
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]))
        .style(Style::default().bg(theme.effective_header_bg())),
        chunks[2],
    );
}

// ── question prompt ──────────────────────────────────────────

fn draw_question(frame: &mut Frame, state: &mut LearningViewState, theme: &Theme) {
    let area = centered_rect(72, 46, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let Some(question) = &mut state.question else {
        return;
    };
    let block = Block::default()
        .title(format!(" {} ", question.intent.label()))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 5 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(" About: ", Style::default().fg(theme.text_muted.to_color())),
                Span::styled(
                    question.anchor.describe(question.file_path.as_deref()),
                    Style::default()
                        .fg(theme.info.to_color())
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(Span::styled(
                match question.parent_qa_id {
                    Some(_) => " Following up on the answer you were reading.",
                    None => " The agent sees this selection and the file around it.",
                },
                Style::default().fg(theme.text_muted.to_color()),
            )),
        ]),
        chunks[0],
    );

    draw_question_editor(frame, chunks[1], question, theme);

    let key = Style::default().fg(theme.warning.to_color());
    let word = Style::default().fg(theme.text_muted.to_color());
    let flip_to = question.intent.toggled().label();
    let vim = question.editor.vim_mode();
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(vec![
                Span::styled(" Tab", key),
                Span::styled(" ask this  ", word),
                Span::styled("Ctrl+E", key),
                Span::styled(format!(" switch to \"{flip_to}\"  "), word),
                Span::styled("Ctrl+P", key),
                Span::styled(" starter questions  ", word),
                Span::styled("Esc", key),
                Span::styled(" cancel", word),
            ]),
            Line::from(Span::styled(
                match vim {
                    Some(mode) => format!(" vim: {mode:?} (Ctrl+T to leave vim mode)"),
                    None => " Answers are generated by the agent CLI, so each question costs whatever that agent costs.".to_string(),
                },
                Style::default().fg(theme.text_muted.to_color()),
            )),
        ]),
        chunks[2],
    );
}

fn draw_question_editor(
    frame: &mut Frame,
    area: Rect,
    question: &mut LearningQuestionEditor,
    theme: &Theme,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let lines = editor_lines(
        &question.editor,
        theme,
        "Type your question — plain English is fine.",
    );
    let wrap_width = area.width.saturating_sub(1).max(1) as usize;
    let total = count_wrapped_editor_lines(&lines, wrap_width);
    sync_editor_scroll(
        &question.editor,
        &mut question.scroll,
        &mut question.sync_to_cursor,
        area.height as usize,
        wrap_width,
        total,
    );
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((question.scroll as u16, 0)),
        area,
    );
}

// ── pickers ──────────────────────────────────────────────────

fn draw_starter_picker(frame: &mut Frame, picker: &LearningStarterPicker, theme: &Theme) {
    let area = centered_rect(72, 52, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Starter questions ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        Line::from(Span::styled(
            " Pick one to fill the prompt — it stays editable, and nothing is asked yet.",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(""),
    ];
    for (row, index) in picker.indices.iter().enumerate() {
        let Some(preset) = STARTER_QUESTIONS.get(*index) else {
            continue;
        };
        let selected = row == picker.selected;
        let style = if selected {
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text.to_color())
        };
        lines.push(Line::from(vec![
            Span::styled(if selected { " › " } else { "   " }, style),
            Span::styled(preset.text.to_string(), style),
        ]));
        lines.push(Line::from(Span::styled(
            format!("     {}", preset.intent.label()),
            Style::default().fg(theme.text_muted.to_color()),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" j/k", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " choose  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " put it in the prompt  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn draw_harness_picker(frame: &mut Frame, picker: &LearningHarnessPicker, theme: &Theme) {
    let area = centered_rect(52, 40, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Which agent answers your questions? ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        Line::from(Span::styled(
            " Already set to a working default — you never have to change this.",
            Style::default().fg(theme.text_muted.to_color()),
        )),
        Line::from(""),
    ];
    for (index, harness) in picker.harnesses.iter().enumerate() {
        let style = if index == picker.selected {
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text.to_color())
        };
        lines.push(Line::from(Span::styled(
            format!("  {}", harness.display_name()),
            style,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " use this one  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " keep what I had",
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

// ── help ─────────────────────────────────────────────────────

fn draw_help(frame: &mut Frame, state: &mut LearningViewState, theme: &Theme) {
    let area = centered_rect(78, 84, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Learning Mode ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 3 {
        return;
    }

    let body_area = Rect::new(inner.x, inner.y, inner.width, inner.height - 1);
    let hint_area = Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1);

    let lines = help_lines(theme);
    let visible = body_area.height as usize;
    let max_scroll = lines.len().saturating_sub(visible);
    state.help_scroll = state.help_scroll.min(max_scroll);
    let shown: Vec<Line> = lines
        .iter()
        .skip(state.help_scroll)
        .take(visible)
        .cloned()
        .collect();
    frame.render_widget(Paragraph::new(shown), body_area);

    draw_scrollbar(frame, body_area, lines.len(), state.help_scroll, visible);

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" j/k", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " scroll  ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled("Esc/?", Style::default().fg(theme.warning.to_color())),
            Span::styled(
                " start browsing",
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ])),
        hint_area,
    );
}

/// The first thing a newcomer sees, so it leads with what the mode is for and
/// what it can't do to their code, and only then lists keys.
fn help_lines(theme: &Theme) -> Vec<Line<'static>> {
    let body = Style::default().fg(theme.text.to_color());
    let muted = Style::default().fg(theme.text_muted.to_color());
    let heading = Style::default()
        .fg(theme.primary.to_color())
        .add_modifier(Modifier::BOLD);
    let key = Style::default().fg(theme.warning.to_color());

    let mut lines = vec![
        Line::from(Span::styled(
            " Read code you didn't write, and ask an agent about it.",
            body,
        )),
        Line::from(""),
        Line::from(Span::styled(
            " Nothing here changes your files. The viewer is read-only: you can",
            Style::default().fg(theme.success.to_color()),
        )),
        Line::from(Span::styled(
            " browse and ask anything without risking the project.",
            Style::default().fg(theme.success.to_color()),
        )),
        Line::from(""),
        Line::from(Span::styled(" No question is too basic.", body)),
        Line::from(""),
        Line::from(Span::styled(" Two ways to ask", heading)),
    ];

    for (k, text) in [
        (
            "e",
            "explain this to me — a teaching answer, no changes proposed",
        ),
        (
            "c",
            "ask for a change — a concrete proposal you can act on later",
        ),
        (
            "t",
            "starter questions — presets to edit, for when you're not sure",
        ),
        (
            "F",
            "ask a follow-up — the agent keeps the answer you just read",
        ),
    ] {
        lines.push(key_row(k, text, key, body));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " What a question is about",
        heading,
    )));
    for (k, text) in [
        ("j/k", "move the cursor in whichever pane has focus"),
        (
            "Tab",
            "move between the file list, the code, and your questions",
        ),
        ("Enter", "open a file, or read the selected answer"),
        ("v / V", "start a line range / drop back to one line"),
        ("f", "the whole file"),
        ("P", "the whole project"),
        ("x", "the change under the cursor (branch changes only)"),
        ("z", "fold or unfold the Start here group"),
    ] {
        lines.push(key_row(k, text, key, body));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" Settings", heading)));
    for (k, text) in [
        (
            "s",
            "switch between all files and just this branch's changes",
        ),
        (
            "L",
            "newcomer answers (terms defined, what to read next) or familiar",
        ),
        (
            "m",
            "which agent answers — already set to something that works",
        ),
        ("q / Esc", "close Learning Mode"),
    ] {
        lines.push(key_row(k, text, key, body));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(" Worth knowing", heading)));
    for note in [
        " Explaining and asking for a change are separate, and both optional.",
        " Answers are generated by the agent CLI you picked, so each question",
        " costs whatever that agent costs.",
        " Asking doesn't block anything — keep browsing while an answer arrives.",
    ] {
        lines.push(Line::from(Span::styled(note, muted)));
    }

    lines
}

// ── small helpers ────────────────────────────────────────────

/// One `key — what it does` row in the help overlay.
fn key_row(k: &str, text: &str, key: Style, body: Style) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("   {k:<10}"), key),
        Span::styled(text.to_string(), body),
    ])
}

fn pane_block(title: &str, focused: bool, theme: &Theme) -> Block<'static> {
    Block::default()
        .title(title.to_string())
        .borders(Borders::ALL)
        .border_style(if focused {
            Style::default()
                .fg(theme.border_focus.to_color())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.border.to_color())
        })
}

/// Smallest scroll offset that keeps `selected` inside a `visible`-row window.
fn keep_in_view(scroll: usize, selected: usize, visible: usize) -> usize {
    if visible == 0 {
        return scroll;
    }
    if selected < scroll {
        selected
    } else if selected >= scroll + visible {
        selected + 1 - visible
    } else {
        scroll
    }
}

fn draw_scrollbar(frame: &mut Frame, area: Rect, total: usize, position: usize, visible: usize) {
    if total <= visible || area.height == 0 {
        return;
    }
    let scrollbar = Scrollbar::default()
        .orientation(ScrollbarOrientation::VerticalRight)
        .begin_symbol(Some("↑"))
        .end_symbol(Some("↓"));
    let mut scrollbar_state = ScrollbarState::new(total)
        .position(position)
        .viewport_content_length(visible);
    frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
}

/// Keep the tail of a path: the filename matters more than the repo root it
/// sits under when the pane is narrow.
fn truncate_left(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let keep = width.saturating_sub(1);
    let mut out = String::from("…");
    out.extend(chars[chars.len() - keep..].iter());
    out
}

fn truncate_right(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width {
        return text.to_string();
    }
    let mut out: String = chars[..width.saturating_sub(1)].iter().collect();
    out.push('…');
    out
}

/// A question's first line, for the one-line history row.
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeping_a_row_in_view_scrolls_only_when_it_has_to() {
        assert_eq!(keep_in_view(0, 3, 10), 0, "already visible");
        assert_eq!(keep_in_view(5, 2, 10), 2, "scrolled past it going up");
        assert_eq!(keep_in_view(0, 12, 10), 3, "just far enough down");
        assert_eq!(keep_in_view(4, 4, 0), 4, "no room to decide anything");
    }

    #[test]
    fn truncation_keeps_the_end_of_a_path_and_the_start_of_a_question() {
        assert_eq!(
            truncate_left("src/app/learning.rs", 40),
            "src/app/learning.rs"
        );
        assert_eq!(truncate_left("src/app/learning.rs", 10), "…arning.rs");
        assert_eq!(
            truncate_right("what does this do?", 40),
            "what does this do?"
        );
        assert_eq!(truncate_right("what does this do?", 10), "what does…");
    }

    #[test]
    fn a_multi_line_question_renders_as_its_first_line() {
        assert_eq!(first_line("  why this?\nand also that\n"), "why this?");
        assert_eq!(first_line(""), "");
    }

    // ── whole-overlay rendering ──────────────────────────────

    use crate::app::{LearningLevel, LearningRunMode};
    use crate::project::AgentKind;
    use std::path::PathBuf;

    const ANSWER: &str = "\
## What this does

It walks the tree once, then:

- collects the paths
- drops anything ignored
- sorts what is left

```rust
let files = list_repo_files(workdir)?;
```

### Where to look next

`src/diff.rs` has the git plumbing.
";

    fn state() -> LearningViewState {
        let mut state = LearningViewState::new(
            "proj-1".to_string(),
            0,
            0,
            "amf".to_string(),
            "learning-mode".to_string(),
            PathBuf::from("/tmp/does-not-matter"),
            true,
            AgentKind::Claude,
            LearningLevel::Newcomer,
            "sess-1".to_string(),
        );
        state.content = vec!["fn main() {}".to_string(), "// second".to_string()];
        state.content_path = Some("src/main.rs".to_string());
        state
    }

    fn answered_qa() -> LearningQa {
        LearningQa {
            id: "qa-1".to_string(),
            session_id: "sess-1".to_string(),
            parent_qa_id: None,
            file_path: Some("src/main.rs".to_string()),
            anchor: LearningAnchor::Lines { start: 1, end: 2 },
            selection_text: "fn main() {}".to_string(),
            selection_is_diff: false,
            question: "What does this do?".to_string(),
            intent: LearningQaIntent::Explain,
            level: LearningLevel::Newcomer,
            answer: Some(ANSWER.to_string()),
            harness: AgentKind::Claude,
            run_mode: LearningRunMode::NoTools,
            status: LearningQaStatus::Answered,
            error: None,
            todo_id: None,
            spawned_session_id: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn render(state: &mut LearningViewState) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let backend = TestBackend::new(140, 44);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| draw_learning_view(frame, state, &Theme::default()))
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
    fn the_header_says_what_the_mode_is_doing_and_that_it_is_read_only() {
        let mut state = state();
        let rendered = render(&mut state);

        assert!(rendered.contains("Learning Mode"));
        assert!(
            rendered.contains("read-only"),
            "the promise has to be visible"
        );
        assert!(rendered.contains("Showing:"), "browse scope");
        assert!(rendered.contains("Explaining for:"), "level");
        assert!(rendered.contains("Agent:"), "harness");
    }

    #[test]
    fn an_in_flight_count_appears_only_while_something_is_generating() {
        let mut state = state();
        assert!(!render(&mut state).contains("still generating"));

        let mut pending = answered_qa();
        pending.status = LearningQaStatus::Running;
        pending.answer = None;
        state.qa.push(pending);

        assert!(render(&mut state).contains("still generating"));
    }

    /// The plan's renderer check: a markdown answer with headings, a list, and
    /// a fenced code block comes out formatted rather than as raw source.
    #[test]
    fn a_markdown_answer_renders_formatted() {
        let mut state = state();
        state.qa.push(answered_qa());
        state.answer_open = true;

        let rendered = render(&mut state);

        assert!(rendered.contains("What this does"), "heading text");
        assert!(
            !rendered.contains("## What this does"),
            "the heading markers should be consumed by the renderer, not printed"
        );
        assert!(
            !rendered.contains("```"),
            "the fence markers should not survive into the output"
        );
        assert!(
            rendered.contains("list_repo_files"),
            "the code block's contents still render"
        );
        assert!(rendered.contains("collects the paths"), "list item");
        // The question and its provenance head the pane.
        assert!(rendered.contains("What does this do?"));
        assert!(rendered.contains("answered by"));
    }

    #[test]
    fn a_long_answer_scrolls_to_its_end() {
        let mut state = state();
        let mut qa = answered_qa();
        // Long enough that the end is well past one screen.
        qa.answer = Some((1..=200).fold(String::new(), |mut acc, i| {
            acc.push_str(&format!("line {i} of the explanation\n\n"));
            acc
        }));
        state.qa.push(qa);
        state.answer_open = true;

        // First render measures the document; `usize::MAX / 2` is what the
        // scroll-to-bottom action stores before the real height is known.
        assert!(render(&mut state).contains("line 1 of the explanation"));

        state.answer_scroll = usize::MAX / 2;
        let rendered = render(&mut state);

        assert!(
            rendered.contains("line 200 of the explanation"),
            "scroll-to-bottom has to land on the last line, not past it"
        );
        assert!(
            state.answer_scroll < usize::MAX / 2,
            "the renderer clamps the stored scroll once it knows the height"
        );
    }

    #[test]
    fn an_actioned_answer_is_marked_in_the_history() {
        let mut state = state();
        let mut qa = answered_qa();
        qa.todo_id = Some("todo-1".to_string());
        qa.spawned_session_id = Some("sess-9".to_string());
        state.qa.push(qa);

        let rendered = render(&mut state);
        assert!(rendered.contains("TODO"), "actioned marker");
        assert!(rendered.contains("session"), "escalated marker");
    }

    #[test]
    fn a_follow_up_renders_indented_under_its_parent() {
        let mut state = state();
        state.qa.push(answered_qa());
        let mut child = answered_qa();
        child.id = "qa-2".to_string();
        child.parent_qa_id = Some("qa-1".to_string());
        child.question = "And why the sort?".to_string();
        state.qa.push(child);

        let rendered = render(&mut state);
        assert!(rendered.contains("And why the sort?"));
        assert!(
            rendered.contains("└"),
            "the thread marker shows the nesting"
        );
    }

    #[test]
    fn the_empty_history_pane_says_how_to_start() {
        let mut state = state();
        let rendered = render(&mut state);
        assert!(rendered.contains("No questions yet"));
        assert!(
            rendered.contains("No question is too basic"),
            "the pane is written for someone who doesn't know what to ask"
        );
    }
}
