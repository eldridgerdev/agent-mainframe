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
    LearningRunMode, LearningStarterPicker, LearningViewState,
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

    // Refusals and confirmations share one line: only the most recent of the
    // two is ever set, since each clears the other.
    let has_error = state.error.is_some() || state.notice.is_some();
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
    if state.action_editor.is_some() {
        draw_action_editor(frame, state, theme);
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

/// The banner line: a refusal in the failure colour, or — when there is none —
/// a confirmation of what the last key actually did, in a colour that doesn't
/// read as something having gone wrong.
fn draw_error(frame: &mut Frame, area: Rect, state: &LearningViewState, theme: &Theme) {
    let (message, color) = match (state.error.as_deref(), state.notice.as_deref()) {
        (Some(error), _) => (error, theme.danger.to_color()),
        (None, Some(notice)) => (notice, theme.info.to_color()),
        (None, None) => return,
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {message}"),
            Style::default().fg(color),
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
    let has_answer = state.qa.iter().any(|qa| qa.answer.is_some());
    if has_answer {
        first.push(Span::styled("  F", key));
        first.push(Span::styled(" ask a follow-up", word));
    }
    if state.question.is_none() && !state.qa.is_empty() {
        first.push(Span::styled(
            format!("   ({} asked)", state.qa.len()),
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }

    let mut second = vec![
        Span::raw("  "),
        // The two range keys share a hint: spelling both out cost more of the
        // line than the pair is worth, and what the second one does is legible
        // from what the first one does.
        Span::styled("v/V", key),
        Span::styled(" line range on/off  ", word),
        Span::styled("f", key),
        Span::styled(" whole file  ", word),
        Span::styled("P", key),
        Span::styled(" whole project  ", word),
        Span::styled("x", key),
        Span::styled(" this change  ", word),
    ];
    // The footer truncates from the right, so what goes here is rationed
    // against `q close` surviving at 140 columns. The two answer keys travel
    // together — both act on the selected entry, and offering one without the
    // other reads as the other not existing. `z` takes the slot only before
    // there is an answer: the Start here group is built for a project with no
    // history, so by then it is a leftover the next reload drops. It stays in
    // the `?` overlay either way.
    if has_answer {
        second.push(Span::styled("D", key));
        second.push(Span::styled(" ask again, reading the repo  ", word));
        second.push(Span::styled("a", key));
        second.push(Span::styled(" keep as a to-do  ", word));
    } else if state
        .entries
        .iter()
        .any(|e| matches!(e, LearningListEntry::StartHereHeader))
    {
        second.push(Span::styled("z", key));
        second.push(Span::styled(" fold Start here  ", word));
    }
    second.extend([
        Span::styled("?", key),
        Span::styled(" help  ", word),
        Span::styled("q", key),
        Span::styled(" close", word),
    ]);

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

    // The overlay's banner sits behind this pane, so a refusal raised by a key
    // pressed *in here* — `D` on an answer that already read the repo — would
    // otherwise be invisible until the pane is closed, which is precisely the
    // silently-swallowed keypress the mode is meant not to have.
    let banner = match (&state.error, &state.notice) {
        (Some(error), _) => Some((error.clone(), theme.danger.to_color())),
        (None, Some(notice)) => Some((notice.clone(), theme.info.to_color())),
        (None, None) => None,
    };
    let mut constraints = vec![Constraint::Length(3), Constraint::Min(1)];
    if banner.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(2));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);
    let footer_chunk = chunks[chunks.len() - 1];
    if let Some((message, color)) = &banner {
        frame.render_widget(
            Paragraph::new(Span::styled(message.clone(), Style::default().fg(*color)))
                .style(Style::default().bg(theme.effective_header_bg()))
                .wrap(Wrap { trim: true }),
            chunks[2],
        );
    }

    let header = Paragraph::new(vec![
        Line::from(Span::styled(
            qa.question.clone(),
            Style::default()
                .fg(theme.text.to_color())
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            answer_provenance(&qa),
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
        Paragraph::new(answer_footer(&qa, footer_chunk.width as usize, theme))
            .style(Style::default().bg(theme.effective_header_bg())),
        footer_chunk,
    );
}

/// One key hint in the answer pane's footer: the key and what it does.
struct AnswerHint {
    key: &'static str,
    label: &'static str,
    /// Order this hint is given up in when the line doesn't fit — highest
    /// first. `None` never drops.
    drop_rank: Option<u8>,
}

/// The answer pane's key footer, fitted to `width`.
///
/// Two lines, split by what they are for: what you can *do* with this answer on
/// top, how to move around it underneath. One line stopped being enough once
/// keeping an answer joined following it up, sending it deeper, and re-filing
/// it — the pane is a percentage of the terminal, so at 110 columns it has
/// about 92 inner columns, and a single line of all eight hints wants over 150.
/// Splitting them costs one row of answer text and buys every action being
/// visible where the answer is read, which is where they are reached for.
///
/// Each line is still fitted independently, because a narrow terminal can
/// overrun either: the widget truncates from the right, which would take `Esc
/// back to browsing` off a modal with no other way out. Hints are dropped
/// instead, least useful first — the scrolling keys that duplicate `j/k`, then
/// re-filing, a bookkeeping gesture rather than a way to learn anything, then
/// handing the answer to a live agent. `F`, `D`, `a`, `j/k` and `Esc` are never
/// dropped.
///
/// Hints for keys the selected row would refuse are not shown at all — a
/// deep-dive row cannot be sent deeper, and an unanswered one cannot be
/// followed up or kept — so the footer never advertises a keypress that answers
/// with a banner.
///
/// The two asking keys swap order by intent. On a change request the answer
/// was proposed without the repository open, so checking it against the real
/// code earns its place ahead of continuing the conversation; on an
/// explanation, the next question is the likelier move.
fn answer_footer(qa: &LearningQa, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let follow_up = qa.answer.is_some().then_some(AnswerHint {
        key: "F",
        label: "ask a follow-up",
        drop_rank: None,
    });
    let deep_dive = (!qa.status.is_in_flight() && qa.run_mode == LearningRunMode::NoTools)
        .then_some(AnswerHint {
            key: "D",
            label: "ask again, reading the repo",
            drop_rank: None,
        });
    let asking = match qa.intent {
        LearningQaIntent::Action => [deep_dive, follow_up],
        LearningQaIntent::Explain => [follow_up, deep_dive],
    };
    let mut actions: Vec<AnswerHint> = asking.into_iter().flatten().collect();
    // Offered on an answered row either way: on a row that already produced an
    // item the key opens it rather than making a second, and saying which of
    // the two will happen is the point of the label.
    if qa.answer.is_some() {
        actions.push(AnswerHint {
            key: "a",
            label: match qa.todo_id {
                Some(_) => "open its TODO item",
                None => "keep this as a to-do",
            },
            drop_rank: None,
        });
    }
    // The one key here that leaves the read-only overlay. Offered last because
    // it is the least likely next move while reading, but dropped *after*
    // re-filing: an answer you cannot act on is worth more than a label.
    if !qa.status.is_in_flight() {
        actions.push(AnswerHint {
            key: "S",
            // Terse for a reason: with five hints on the line, the pane's ~118
            // inner columns at a 140-column terminal leave 22 for this one, and
            // going over costs the whole `i` hint.
            label: match qa.spawned_session_id {
                Some(_) => "back to its session",
                None => "hand to a live agent",
            },
            drop_rank: Some(1),
        });
    }
    actions.push(AnswerHint {
        key: "i",
        label: match qa.intent {
            LearningQaIntent::Explain => "file as a change",
            LearningQaIntent::Action => "file as a note",
        },
        drop_rank: Some(2),
    });

    let moving = vec![
        AnswerHint {
            key: "j/k",
            label: "scroll",
            drop_rank: None,
        },
        AnswerHint {
            key: "PgUp/PgDn",
            label: "page",
            drop_rank: Some(1),
        },
        AnswerHint {
            key: "g/G",
            label: "top/bottom",
            drop_rank: Some(2),
        },
        AnswerHint {
            key: "Esc",
            label: "back to browsing",
            drop_rank: None,
        },
    ];

    vec![
        hint_line(fit_hints(actions, width), theme),
        hint_line(fit_hints(moving, width), theme),
    ]
}

/// Drop hints, highest `drop_rank` first, until the line fits `width`.
fn fit_hints(mut hints: Vec<AnswerHint>, width: usize) -> Vec<AnswerHint> {
    // Widths include the two-space gap every hint but the last carries.
    let cost = |hint: &AnswerHint| hint.key.chars().count() + 1 + hint.label.chars().count() + 2;
    let mut total: usize = hints.iter().map(cost).sum::<usize>().saturating_sub(2);
    while total > width {
        let Some(victim) = hints
            .iter()
            .enumerate()
            .filter_map(|(i, hint)| hint.drop_rank.map(|rank| (rank, i)))
            .max()
            .map(|(_, i)| i)
        else {
            break;
        };
        total -= cost(&hints[victim]);
        hints.remove(victim);
    }
    hints
}

fn hint_line(hints: Vec<AnswerHint>, theme: &Theme) -> Line<'static> {
    let key_style = Style::default().fg(theme.warning.to_color());
    let word_style = Style::default().fg(theme.text_muted.to_color());
    let last = hints.len().saturating_sub(1);
    let mut spans = Vec::new();
    for (i, hint) in hints.iter().enumerate() {
        spans.push(Span::styled(hint.key, key_style));
        spans.push(Span::styled(
            if i == last {
                format!(" {}", hint.label)
            } else {
                format!(" {}  ", hint.label)
            },
            word_style,
        ));
    }
    Line::from(spans)
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

// ── keep this as a to-do ─────────────────────────────────────

/// The confirmation for turning an answer into a TODO item.
///
/// It leads with what pressing Enter will and won't do, in those words: this is
/// the one place in Learning Mode where a keypress writes something, and the
/// user has been told all along that the mode changes nothing. The distinction
/// — a note about the code, not a change to it — has to survive being read
/// quickly.
fn draw_action_editor(frame: &mut Frame, state: &mut LearningViewState, theme: &Theme) {
    let area = centered_rect(72, 60, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let Some(editor) = &mut state.action_editor else {
        return;
    };
    let block = Block::default()
        .title(" Keep this as a to-do ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height < 8 {
        return;
    }

    let mut constraints = vec![
        // A blank row under the promise, so the two lines that draw the line
        // between a note and an edit don't run into the title prompt.
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Length(1),
        Constraint::Min(1),
    ];
    if editor.error.is_some() {
        constraints.push(Constraint::Length(1));
    }
    constraints.push(Constraint::Length(1));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                " This adds a note to this project's TODO list.",
                Style::default().fg(theme.text.to_color()),
            )),
            Line::from(Span::styled(
                " It writes a note about your code, not a change to it.",
                Style::default().fg(theme.success.to_color()),
            )),
        ]),
        chunks[0],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Title — edit it, the suggestion is only the answer's first line:",
            Style::default().fg(theme.text_muted.to_color()),
        ))),
        chunks[1],
    );

    let lines = editor_lines(&editor.title, theme, "Type a title for this note.");
    let wrap_width = chunks[2].width.saturating_sub(1).max(1) as usize;
    let total = count_wrapped_editor_lines(&lines, wrap_width);
    sync_editor_scroll(
        &editor.title,
        &mut editor.scroll,
        &mut editor.sync_to_cursor,
        chunks[2].height as usize,
        wrap_width,
        total,
    );
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((editor.scroll as u16, 0)),
        chunks[2],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " The note will say:",
            Style::default().fg(theme.text_muted.to_color()),
        ))),
        chunks[3],
    );
    // Truncated rather than wrapped, and marked where it was cut: the preview
    // is here so nothing about the note is a surprise, and a line clipped
    // silently at the pane edge would read as a note that stops mid-sentence.
    let body_width = chunks[4].width.saturating_sub(1) as usize;
    frame.render_widget(
        Paragraph::new(
            editor
                .body
                .lines()
                .map(|line| {
                    Line::from(Span::styled(
                        format!(" {}", truncate_right(line, body_width)),
                        Style::default().fg(theme.text_muted.to_color()),
                    ))
                })
                .collect::<Vec<_>>(),
        ),
        chunks[4],
    );

    let mut idx = 5;
    if let Some(error) = &editor.error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {error}"),
                Style::default().fg(theme.danger.to_color()),
            ))),
            chunks[idx],
        );
        idx += 1;
    }

    let key = Style::default().fg(theme.warning.to_color());
    let word = Style::default().fg(theme.text_muted.to_color());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" Enter", key),
            Span::styled(" add it to the list  ", word),
            Span::styled("Esc", key),
            Span::styled(" cancel — nothing is written", word),
        ])),
        chunks[idx],
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
        (
            "D",
            "ask again, letting the agent read the repo — slower, but it checks",
        ),
        (
            "i",
            "re-file an entry as the other kind — the answer itself is kept",
        ),
        (
            "a",
            "keep an answer as a to-do — adds a note to the list, not a change",
        ),
        (
            "S",
            "hand an answer to a live agent — that one can change files",
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
        " Most answers only see the code on screen, so they can name files or",
        " lines that don't exist. D re-asks with the repo open, so the agent",
        " can go and check. The first answer is kept either way.",
        " S is the one way out of read-only: it opens a normal agent session,",
        " which can change files. The question is filled in for you, and",
        " nothing is sent until you press Enter on it.",
    ] {
        lines.push(Line::from(Span::styled(note, muted)));
    }

    lines
}

// ── small helpers ────────────────────────────────────────────

/// Where an answer came from, in one line: who produced it, how much it was
/// allowed to read, and who it was written for.
///
/// The status is carried by the opening verb rather than repeated at the end —
/// "answered by Claude … · answered" said it twice, and said "answered by" of
/// a row that hadn't answered.
fn answer_provenance(qa: &LearningQa) -> String {
    let who = qa.harness.display_name();
    let lead = match qa.status {
        LearningQaStatus::Answered => format!("answered by {who}"),
        LearningQaStatus::Running => format!("{who} is answering"),
        LearningQaStatus::Pending => format!("queued for {who}"),
        LearningQaStatus::Failed => format!("{who} couldn't answer"),
    };
    format!(
        "{lead} · {} · written for a {} reader",
        qa.run_mode.description(),
        qa.level.as_str()
    )
}

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
            deep_dive_of: None,
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
        render_at(state, 140, 44)
    }

    fn render_at(state: &mut LearningViewState, width: u16, height: u16) -> String {
        use ratatui::{Terminal, backend::TestBackend};

        let backend = TestBackend::new(width, height);
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
    fn the_answer_pane_states_its_provenance_once() {
        let qa = answered_qa();
        let line = answer_provenance(&qa);
        assert_eq!(
            line,
            "answered by Claude · this file only · written for a newcomer reader"
        );
        assert_eq!(
            line.matches("answered").count(),
            1,
            "the status is the opening verb, not also a trailing word: {line}"
        );

        // A row that hasn't answered must not claim it was "answered by".
        for (status, expected) in [
            (LearningQaStatus::Running, "Claude is answering"),
            (LearningQaStatus::Pending, "queued for Claude"),
            (LearningQaStatus::Failed, "Claude couldn't answer"),
        ] {
            let mut pending = answered_qa();
            pending.status = status;
            let line = answer_provenance(&pending);
            assert!(line.starts_with(expected), "{line}");
            assert!(line.ends_with("written for a newcomer reader"), "{line}");
        }
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

    /// The footer truncates from the right, so a key added to an already-full
    /// line is a key nobody sees — and the one it pushes off is `q close`.
    /// Asked with the Start here group present, which is how the real app looks
    /// right after a first question: the group is only dropped on the next file
    /// list reload, so the two do compete for this line.
    #[test]
    fn the_deep_dive_key_survives_the_footer_at_a_real_width() {
        let mut state = state();
        state.entries = vec![LearningListEntry::StartHereHeader];

        let rendered = render(&mut state);
        assert!(
            !rendered.contains("reading the repo"),
            "not offered before there is an answer to send deeper"
        );
        assert!(rendered.contains("z fold Start here"), "{rendered}");

        state.qa.push(answered_qa());
        let rendered = render(&mut state);
        assert!(
            rendered.contains("D ask again, reading the repo"),
            "{rendered}"
        );
        assert!(
            rendered.contains("F ask a follow-up"),
            "and it didn't push the follow-up key off the first line"
        );
        assert!(
            rendered.contains("q close"),
            "nor the quit key off the second: {rendered}"
        );
    }

    /// The pane is a percentage of the terminal, so its footer has to fit a
    /// narrow one. Truncation would take `Esc` — the only way out of a modal —
    /// off the end, so hints are dropped from the middle instead.
    #[test]
    fn the_answer_footer_keeps_the_way_out_and_the_actions_when_narrow() {
        let mut state = state();
        state.qa.push(answered_qa());
        state.answer_open = true;

        // Splitting actions from navigation is what buys room for all of them
        // at an ordinary terminal size.
        let wide = render(&mut state);
        for hint in [
            "F ask a follow-up",
            "D ask again, reading the repo",
            "a keep this as a to-do",
            "S hand to a live agent",
            "i file as a change",
            "j/k scroll",
            "PgUp/PgDn page",
            "g/G top/bottom",
            "Esc back to browsing",
        ] {
            assert!(wide.contains(hint), "missing {hint:?}: {wide}");
        }

        let narrow = render_at(&mut state, 100, 44);
        assert!(
            narrow.contains("Esc back to browsing"),
            "the way out survives first: {narrow}"
        );
        assert!(
            narrow.contains("D ask again, reading the repo"),
            "and so do the actions: {narrow}"
        );
        assert!(narrow.contains("F ask a follow-up"), "{narrow}");
        assert!(narrow.contains("a keep this as a to-do"), "{narrow}");
        assert!(
            narrow.contains("j/k scroll"),
            "the common scroll keys outlive the rarer ones: {narrow}"
        );
        assert!(
            !narrow.contains("i file as a change"),
            "bookkeeping is what gives way: {narrow}"
        );
        assert!(
            !narrow.contains("hand to a live agent"),
            "and then the one action you can still reach from the dashboard: {narrow}"
        );
    }

    /// A footer that offers a key the row will refuse is the same dead keypress
    /// as one that is clipped off the end.
    #[test]
    fn the_answer_footer_only_offers_keys_the_row_can_act_on() {
        let mut dived = state();
        let mut deep = answered_qa();
        deep.run_mode = LearningRunMode::DeepDive;
        dived.qa.push(deep);
        dived.answer_open = true;

        let rendered = render(&mut dived);
        assert!(
            !rendered.contains("ask again, reading the repo"),
            "this one already read it: {rendered}"
        );
        assert!(rendered.contains("F ask a follow-up"), "{rendered}");

        let mut waiting = state();
        let mut running = answered_qa();
        running.answer = None;
        running.status = LearningQaStatus::Running;
        waiting.qa.push(running);
        waiting.answer_open = true;

        let rendered = render(&mut waiting);
        assert!(
            !rendered.contains("ask a follow-up"),
            "nothing to follow up on yet: {rendered}"
        );
        assert!(
            !rendered.contains("ask again, reading the repo"),
            "nor to send deeper: {rendered}"
        );
        assert!(rendered.contains("Esc back to browsing"), "{rendered}");
    }

    /// The answer pane covers the overlay's banner, so a key refused from
    /// inside it — `D` on an answer that already read the repo — has to say so
    /// here or it reads as a dead key.
    #[test]
    fn a_refusal_raised_inside_the_answer_pane_is_visible_there() {
        let mut state = state();
        state.qa.push(answered_qa());
        state.answer_open = true;
        state.error = Some("That answer already read the repository.".to_string());

        let rendered = render(&mut state);
        assert!(
            rendered.contains("That answer already read the repository."),
            "{rendered}"
        );
        assert!(
            rendered.contains("Esc back to browsing"),
            "and the key footer is still there, not displaced: {rendered}"
        );
    }

    /// Re-filing has to be visible in the list, or the key did nothing a user
    /// can see.
    #[test]
    fn a_re_filed_entry_carries_the_other_marker() {
        let mut state = state();
        state.qa.push(answered_qa());
        let explained = render(&mut state);
        assert!(explained.contains("explain"), "{explained}");

        state.qa[0].intent = LearningQaIntent::Action;
        let refiled = render(&mut state);
        assert!(refiled.contains("change"), "{refiled}");
        assert!(
            refiled.contains("What does this do?"),
            "the question is untouched: {refiled}"
        );
    }

    /// A change request was proposed without the repository open, so checking
    /// it comes before continuing the conversation; on an explanation the next
    /// question is the likelier move. Ordering is the only thing intent
    /// changes about the actions — both keys work on both.
    #[test]
    fn the_answer_footer_leads_with_what_the_entry_kind_makes_likely() {
        let mut state = state();
        state.qa.push(answered_qa());
        state.answer_open = true;

        let explaining = render(&mut state);
        let follow = explaining.find("F ask a follow-up").unwrap();
        let deeper = explaining.find("D ask again").unwrap();
        assert!(follow < deeper, "an explanation leads with F: {explaining}");
        assert!(
            explaining.contains("i file as a change"),
            "and offers the other filing: {explaining}"
        );

        state.qa[0].intent = LearningQaIntent::Action;
        let changing = render(&mut state);
        let follow = changing.find("F ask a follow-up").unwrap();
        let deeper = changing.find("D ask again").unwrap();
        assert!(deeper < follow, "a change request leads with D: {changing}");
        assert!(
            changing.contains("i file as a note"),
            "and offers the way back: {changing}"
        );
    }

    /// An entry that already produced an item can still be acted on — the key
    /// opens that item instead of making a second — so the hint has to say
    /// which of the two will happen.
    #[test]
    fn the_keep_hint_says_whether_it_would_add_or_open() {
        let mut state = state();
        state.qa.push(answered_qa());
        state.answer_open = true;
        assert!(render(&mut state).contains("a keep this as a to-do"));

        state.qa[0].todo_id = Some("todo-1".to_string());
        let kept = render(&mut state);
        assert!(kept.contains("a open its TODO item"), "{kept}");
        assert!(
            !kept.contains("a keep this as a to-do"),
            "it would not add a second: {kept}"
        );
    }

    /// Same reasoning as the keep hint: a row that already opened a session
    /// jumps back to it rather than starting a second, so the footer says which
    /// of the two the key will do.
    #[test]
    fn the_escalation_hint_says_whether_it_would_start_or_return() {
        let mut state = state();
        state.qa.push(answered_qa());
        state.answer_open = true;
        assert!(render(&mut state).contains("S hand to a live agent"));

        state.qa[0].spawned_session_id = Some("sess-9".to_string());
        let linked = render(&mut state);
        assert!(linked.contains("S back to its session"), "{linked}");
        assert!(
            !linked.contains("S hand to a live agent"),
            "it would not start a second: {linked}"
        );
    }

    /// This is the one keypress in Learning Mode that writes anything, and the
    /// mode has spent every other screen promising that it doesn't. The dialog
    /// has to draw that line itself.
    #[test]
    fn the_keep_confirmation_says_what_it_will_and_will_not_do() {
        let mut state = state();
        state.qa.push(answered_qa());
        state.action_editor = Some(crate::app::LearningActionEditor {
            qa_id: "qa-1".to_string(),
            title: crate::editor::TextEditor::new("Work out why main is empty".to_string()),
            body: "From Learning Mode — src/main.rs:1-2\n\nAsked: What does this do?".to_string(),
            error: None,
            scroll: 0,
            sync_to_cursor: false,
        });

        let rendered = render(&mut state);
        assert!(rendered.contains("TODO list"), "{rendered}");
        assert!(
            rendered.contains("not a change to it"),
            "the line between a note and an edit: {rendered}"
        );
        assert!(
            rendered.contains("Work out why main is empty"),
            "the title is there to be edited: {rendered}"
        );
        assert!(
            rendered.contains("src/main.rs:1-2"),
            "and what gets written is shown, not implied: {rendered}"
        );
        assert!(
            rendered.contains("nothing is written"),
            "walking away has to be the obvious option: {rendered}"
        );
    }

    /// The dialog covers the overlay's banner line, so a refusal raised from
    /// inside it has nowhere else to appear.
    #[test]
    fn a_refusal_raised_inside_the_keep_confirmation_is_visible_there() {
        let mut state = state();
        state.qa.push(answered_qa());
        state.action_editor = Some(crate::app::LearningActionEditor {
            qa_id: "qa-1".to_string(),
            title: crate::editor::TextEditor::new(String::new()),
            body: "From Learning Mode — src/main.rs:1-2".to_string(),
            error: Some("Give it a title first — this is what you'll see later.".to_string()),
            scroll: 0,
            sync_to_cursor: false,
        });

        let rendered = render(&mut state);
        assert!(rendered.contains("Give it a title first"), "{rendered}");
        assert!(
            rendered.contains("Esc cancel"),
            "and the way out is still there: {rendered}"
        );
    }

    /// Index of the first cell of `needle`, counted in cells rather than bytes
    /// so the pane borders don't shift it.
    fn cell_index(buffer: &ratatui::buffer::Buffer, needle: &str) -> Option<usize> {
        let symbols: Vec<&str> = buffer.content().iter().map(|cell| cell.symbol()).collect();
        let want: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
        symbols
            .windows(want.len())
            .position(|window| window.iter().zip(&want).all(|(a, b)| *a == b.as_str()))
    }

    /// The banner line carries both refusals and confirmations. Painting "this
    /// worked" in the failure colour is a small lie, and this mode's reader is
    /// the least equipped to discount it.
    #[test]
    fn a_confirmation_is_not_painted_as_a_failure() {
        use ratatui::{Terminal, backend::TestBackend};

        let theme = Theme::default();
        let mut state = state();
        state.qa.push(answered_qa());
        state.notice = Some("Re-filed as a change request.".to_string());

        let mut terminal = Terminal::new(TestBackend::new(140, 44)).unwrap();
        terminal
            .draw(|frame| draw_learning_view(frame, &mut state, &theme))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();

        let at = cell_index(&buffer, "Re-filed").expect("the confirmation is on screen");
        assert_eq!(buffer.content()[at].fg, theme.info.to_color());

        // An actual refusal still reads as one, and wins the line.
        state.error = Some("That answer already read the repository.".to_string());
        terminal
            .draw(|frame| draw_learning_view(frame, &mut state, &theme))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        assert!(
            cell_index(&buffer, "Re-filed").is_none(),
            "the refusal takes the line"
        );
        let at = cell_index(&buffer, "That answer already").unwrap();
        assert_eq!(buffer.content()[at].fg, theme.danger.to_color());
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
