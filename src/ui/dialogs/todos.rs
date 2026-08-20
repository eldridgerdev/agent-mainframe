use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

use super::super::dashboard::centered_rect;
use crate::app::{
    TodoEditTarget, TodoEditor, TodoLaunchAction, TodoLaunchStep, TodoQuickCaptureState,
    TodoViewState, TodosHostReassignState,
};
use crate::db::todos::{Todo, TodoPriority};
use crate::theme::Theme;

const CURSOR: &str = "\u{2588}";

/// One-line quick-capture dialog overlaid on a session view. Collects a TODO
/// title to append to the current project's list.
pub fn draw_todo_quick_capture_dialog(
    frame: &mut Frame,
    state: &TodoQuickCaptureState,
    theme: &Theme,
) {
    let area = centered_rect(60, 22, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(format!(" New TODO · {} ", state.project_name))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(inner);

    let input = Paragraph::new(Line::from(vec![
        Span::styled(" Title: ", Style::default().fg(theme.primary.to_color())),
        Span::styled(&state.input, Style::default().fg(theme.text.to_color())),
        Span::styled(CURSOR, Style::default().fg(theme.primary.to_color())),
    ]));
    frame.render_widget(input, chunks[0]);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(" add  ", Style::default().fg(theme.text.to_color())),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text.to_color())),
    ]));
    frame.render_widget(hint, chunks[2]);
}

/// Prompt shown when the feature hosting a project's TODO list is deleted while
/// the project survives: pick a surviving feature to re-home the list onto, or
/// delete the list. The trailing option (index == `candidates.len()`) deletes.
pub fn draw_todos_host_reassign_dialog(
    frame: &mut Frame,
    state: &TodosHostReassignState,
    theme: &Theme,
) {
    // Size to the option list, within sensible bounds.
    let option_count = state.candidates.len() + 1;
    let rows = (option_count as u16).min(8);
    let height_pct = (40 + rows * 4).min(80);
    let area = centered_rect(60, height_pct, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(format!(" TODO list needs a home · {} ", state.project_name))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.warning.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items = if state.todo_count == 1 {
        "item"
    } else {
        "items"
    };
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(" Feature "),
            Span::styled(
                state.deleted_feature_name.as_str(),
                Style::default()
                    .fg(theme.danger.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" hosted this project's TODO list"),
        ]),
        Line::from(vec![
            Span::raw(" ("),
            Span::styled(
                format!("{} {}", state.todo_count, items),
                Style::default().fg(theme.text.to_color()),
            ),
            Span::raw("). Re-home it onto another feature, or delete it:"),
        ]),
        Line::from(""),
    ];

    for (i, (name, _id)) in state.candidates.iter().enumerate() {
        let selected = i == state.selected;
        lines.push(option_line(
            format!("Re-home to {name}"),
            selected,
            theme.primary.to_color(),
            theme,
        ));
    }
    // Trailing "Delete" option.
    let delete_selected = state.selected == state.candidates.len();
    lines.push(option_line(
        "Delete the list and its TODOs".to_string(),
        delete_selected,
        theme.danger.to_color(),
        theme,
    ));

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled(" j/k", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " choose  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " confirm  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " keep list",
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]));

    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, inner);
}

/// One selectable row in the re-home prompt: a `>` cursor + label, highlighted
/// when selected.
fn option_line(
    label: String,
    selected: bool,
    accent: ratatui::style::Color,
    theme: &Theme,
) -> Line<'static> {
    let (marker, style) = if selected {
        (
            " > ",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )
    } else {
        ("   ", Style::default().fg(theme.text.to_color()))
    };
    Line::from(vec![
        Span::styled(marker, style),
        Span::styled(label, style),
    ])
}

/// Full-screen native TODOs overlay: a free-form scratchpad banner on top,
/// then the project's TODO items.
pub fn draw_todos_view(frame: &mut Frame, state: &TodoViewState, theme: &Theme, nerd_font: bool) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }

    frame.render_widget(
        Block::default().style(Style::default().bg(theme.effective_bg())),
        area,
    );

    let scratchpad = state
        .list
        .as_ref()
        .and_then(|l| l.carry_over.as_deref())
        .filter(|s| !s.trim().is_empty());

    // Header (1) + optional scratchpad banner (3) + list (min) + hint (1).
    let mut constraints = vec![Constraint::Length(1)];
    if scratchpad.is_some() {
        constraints.push(Constraint::Length(3));
    }
    constraints.push(Constraint::Min(1));
    constraints.push(Constraint::Length(1));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let mut idx = 0;
    draw_header(frame, chunks[idx], state, theme);
    idx += 1;
    if let Some(note) = scratchpad {
        draw_scratchpad(frame, chunks[idx], note, theme);
        idx += 1;
    }
    let list_area = chunks[idx];
    idx += 1;
    let hint_area = chunks[idx];

    draw_list(frame, list_area, state, theme, nerd_font);
    draw_hint(frame, hint_area, theme);

    // Overlays on top of the list, in the same precedence the key handler
    // uses: delete confirmation, then the launch step, then an inline edit.
    if state.pending_delete {
        draw_delete_confirm(frame, state, theme);
    } else if let Some(step) = &state.launch {
        draw_launch_step(frame, state, step, theme);
    } else if let Some(editor) = &state.editor {
        draw_editor(frame, editor, theme);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, state: &TodoViewState, theme: &Theme) {
    let open = state.todos.iter().filter(|t| !t.done).count();
    let done = state.todos.len().saturating_sub(open);
    let line = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            "TODOs",
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  {} / {}", state.project_name, state.feature_name),
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            format!("   {open} open, {done} done"),
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_scratchpad(frame: &mut Frame, area: Rect, note: &str, theme: &Theme) {
    let block = Block::default()
        .title(" Scratchpad ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning.to_color()));
    let paragraph = Paragraph::new(Span::styled(
        note,
        Style::default().fg(theme.text.to_color()),
    ))
    .block(block)
    .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn draw_list(frame: &mut Frame, area: Rect, state: &TodoViewState, theme: &Theme, nerd_font: bool) {
    if state.todos.is_empty() {
        let empty = Paragraph::new(Span::styled(
            "  No TODOs yet.",
            Style::default().fg(theme.text_muted.to_color()),
        ));
        frame.render_widget(empty, area);
        return;
    }

    let visible = area.height as usize;
    // Keep the selected row in view.
    let scroll = if state.selected >= state.scroll_offset + visible {
        state.selected + 1 - visible
    } else if state.selected < state.scroll_offset {
        state.selected
    } else {
        state.scroll_offset
    };

    let lines: Vec<Line> = state
        .todos
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, todo)| todo_line(todo, i == state.selected, theme, nerd_font))
        .collect();

    frame.render_widget(Paragraph::new(lines), area);

    if state.todos.len() > visible {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(state.todos.len())
            .position(scroll)
            .viewport_content_length(visible);
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

/// Wrap `detail` into pre-indented lines that hang under an option's label.
///
/// `Paragraph`'s own wrapping cannot hang-indent: a continuation line starts at
/// column zero, so a wrapped explanation reads as if it belonged to the dialog
/// rather than to the option above it. Wrapping here, against the width the
/// dialog actually has, keeps the indent on every line.
fn detail_lines(detail: &str, width: u16, theme: &Theme) -> Vec<Line<'static>> {
    const INDENT: &str = "     ";
    let avail = (width as usize).saturating_sub(INDENT.len() + 1);
    if avail == 0 {
        return Vec::new();
    }

    let style = Style::default().fg(theme.text_muted.to_color());
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in detail.split_whitespace() {
        // A word longer than the line gets its own line rather than forcing a
        // break mid-word; the terminal truncates it, which is legible.
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if candidate.chars().count() > avail && !current.is_empty() {
            lines.push(Line::from(vec![
                Span::raw(INDENT),
                Span::styled(std::mem::take(&mut current), style),
            ]));
            current = word.to_string();
        } else {
            current = candidate;
        }
    }
    if !current.is_empty() {
        lines.push(Line::from(vec![
            Span::raw(INDENT),
            Span::styled(current, style),
        ]));
    }
    lines
}

/// The launch step layered over the list: the chooser, then the destination.
///
/// Drawn as a modal over the list rather than replacing it, matching the delete
/// confirmation, so the TODO being acted on stays visible behind the prompt.
fn draw_launch_step(frame: &mut Frame, state: &TodoViewState, step: &TodoLaunchStep, theme: &Theme) {
    let area = centered_rect(64, 46, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = match step {
        TodoLaunchStep::Choice { .. } => " Start work on this TODO ",
        TodoLaunchStep::Destination { .. } => " Where should this plan land? ",
    };
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                step.origin().todo_title.clone(),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    match step {
        TodoLaunchStep::Choice { selected, .. } => {
            for (i, action) in TodoLaunchAction::ALL.iter().enumerate() {
                lines.push(option_line(
                    action.label().to_string(),
                    i == *selected,
                    theme.primary.to_color(),
                    theme,
                ));
                lines.extend(detail_lines(action.detail(), inner.width, theme));
            }
        }
        TodoLaunchStep::Destination {
            host_feature_name,
            can_create_worktree,
            selected,
            ..
        } => {
            lines.push(option_line(
                format!("Here, in \"{host_feature_name}\""),
                *selected == 0,
                theme.primary.to_color(),
                theme,
            ));
            lines.extend(detail_lines(
                "Plans into this feature and starts an agent on it. Nothing new is checked out.",
                inner.width,
                theme,
            ));

            let new_accent = if *can_create_worktree {
                theme.primary.to_color()
            } else {
                theme.text_muted.to_color()
            };
            lines.push(option_line(
                "In a new feature and worktree".to_string(),
                *selected == 1,
                new_accent,
                theme,
            ));
            // A blocked option says why here rather than only on Enter: the
            // reason is a property of the project, not of the keypress.
            let detail = if *can_create_worktree {
                "Creates the branch first, then plans into it and starts its agent."
            } else {
                "Unavailable: this project is not a git repository."
            };
            let mut detail_rows = detail_lines(detail, inner.width, theme);
            if !*can_create_worktree {
                // A blocked option's reason is a warning, not an aside.
                for line in &mut detail_rows {
                    for span in &mut line.spans {
                        span.style = span.style.fg(theme.warning.to_color());
                    }
                }
            }
            lines.extend(detail_rows);
        }
    }

    lines.push(Line::from(""));
    let back = match step {
        TodoLaunchStep::Choice { .. } => " back to list",
        TodoLaunchStep::Destination { .. } => " back",
    };
    lines.push(Line::from(vec![
        Span::styled(" j/k", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " choose  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Enter", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " confirm  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(back, Style::default().fg(theme.text_muted.to_color())),
    ]));

    let _ = state;
    let para = Paragraph::new(lines).wrap(Wrap { trim: false });
    frame.render_widget(para, inner);
}

fn todo_line<'a>(todo: &'a Todo, selected: bool, theme: &Theme, nerd_font: bool) -> Line<'a> {
    let (prio_marker, prio_color) = match todo.priority {
        TodoPriority::High => ("!", theme.danger.to_color()),
        TodoPriority::Med => ("·", theme.warning.to_color()),
        TodoPriority::Low => (" ", theme.text_muted.to_color()),
    };

    let checkbox = if todo.done { "[x]" } else { "[ ]" };
    let cursor = if selected { "› " } else { "  " };

    let title_style = if todo.done {
        Style::default()
            .fg(theme.text_muted.to_color())
            .add_modifier(Modifier::CROSSED_OUT)
    } else if selected {
        Style::default()
            .fg(theme.text.to_color())
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.text.to_color())
    };

    let notes_indicator = match &todo.body {
        Some(body) if !body.trim().is_empty() => {
            if nerd_font {
                "  \u{f036}"
            } else {
                "  ≡"
            }
        }
        _ => "",
    };

    // Marker for a TODO that has launched an agent session.
    let launched_indicator = if todo.spawned_session_id.is_some() {
        if nerd_font { "  \u{f135}" } else { "  ▸" }
    } else {
        ""
    };

    // A TODO planned into its own feature. Distinct from the session marker:
    // `g` goes to the feature, not to a session in this one.
    let planned_indicator = if todo.linked_feature_id.is_some() {
        if nerd_font { "  \u{e0a0}" } else { "  ⑂" }
    } else {
        ""
    };

    let mut spans = vec![
        Span::styled(cursor, Style::default().fg(theme.primary.to_color())),
        Span::styled(
            format!("{prio_marker} "),
            Style::default().fg(prio_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{checkbox} "),
            Style::default().fg(if todo.done {
                theme.success.to_color()
            } else {
                theme.text_muted.to_color()
            }),
        ),
        Span::styled(todo.title.clone(), title_style),
    ];
    if !notes_indicator.is_empty() {
        spans.push(Span::styled(
            notes_indicator,
            Style::default().fg(theme.text_muted.to_color()),
        ));
    }
    if !launched_indicator.is_empty() {
        spans.push(Span::styled(
            launched_indicator,
            Style::default().fg(theme.success.to_color()),
        ));
    }
    if !planned_indicator.is_empty() {
        spans.push(Span::styled(
            planned_indicator,
            Style::default().fg(theme.primary.to_color()),
        ));
    }
    Line::from(spans)
}

fn draw_hint(frame: &mut Frame, area: Rect, theme: &Theme) {
    let hint = Line::from(vec![Span::styled(
        "  j/k move  a add  e title  o notes  space done  p prio  J/K reorder  g start/plan  b scratch  d del  Esc/q close",
        Style::default().fg(theme.text_muted.to_color()),
    )]);
    frame.render_widget(Paragraph::new(hint), area);
}

/// Title and a multi-line hint for each edit target.
fn editor_chrome(target: &TodoEditTarget) -> (&'static str, &'static str) {
    match target {
        TodoEditTarget::New => (" New TODO ", "Enter: add   Esc: cancel"),
        TodoEditTarget::Title => (" Edit title ", "Enter: save   Esc: cancel"),
        TodoEditTarget::Notes => (
            " Edit notes ",
            "Enter: save   Alt+Enter: newline   Esc: cancel",
        ),
        TodoEditTarget::Scratchpad => (
            " Scratchpad ",
            "Enter: save   Alt+Enter: newline   Esc: cancel",
        ),
    }
}

fn draw_editor(frame: &mut Frame, editor: &TodoEditor, theme: &Theme) {
    let multiline = matches!(
        editor.target,
        TodoEditTarget::Notes | TodoEditTarget::Scratchpad
    );
    let area = if multiline {
        centered_rect(70, 40, frame.area())
    } else {
        centered_rect(60, 20, frame.area())
    };
    crate::ui::draw_modal_overlay(frame, area, theme);

    let (title, hint) = editor_chrome(&editor.target);
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);

    // Render the buffer with a block cursor at the editor's cursor position.
    let text = editor.editor.text();
    let cursor = editor.editor.cursor().min(text.len());
    let mut shown = String::with_capacity(text.len() + CURSOR.len());
    shown.push_str(&text[..cursor]);
    shown.push_str(CURSOR);
    shown.push_str(&text[cursor..]);
    let body: Vec<Line> = shown
        .split('\n')
        .map(|l| {
            Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(theme.text.to_color()),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(body).wrap(Wrap { trim: false }), chunks[0]);

    frame.render_widget(
        Paragraph::new(Span::styled(
            hint,
            Style::default().fg(theme.text_muted.to_color()),
        )),
        chunks[1],
    );
}

fn draw_delete_confirm(frame: &mut Frame, state: &TodoViewState, theme: &Theme) {
    let area = centered_rect(50, 18, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Delete TODO ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.danger.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let title = state
        .todos
        .get(state.selected)
        .map(|t| t.title.as_str())
        .unwrap_or("");
    let lines = vec![
        Line::from(Span::styled(
            format!("Delete \"{title}\"?"),
            Style::default().fg(theme.text.to_color()),
        )),
        Line::from(Span::styled(
            "y: confirm   n/Esc: cancel",
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}
