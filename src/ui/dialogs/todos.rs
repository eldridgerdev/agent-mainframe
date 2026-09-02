use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

use super::super::dashboard::centered_rect;
use crate::app::{
    TodoDeleteDisposition, TodoDeleteDispositionState, TodoEditTarget, TodoEditor,
    TodoImplementChoice, TodoImplementChoiceState, TodoLaunchAction, TodoLaunchStep, TodoPane,
    TodoPaneKind, TodoQuickCaptureState, TodoReferenceCompletionState, TodoScopeMoveState,
    TodoSpawnTargetState, TodoViewState, TodosHostReassignState,
};
use crate::db::todos::{Todo, TodoPriority, TodoStatus};
use crate::theme::Theme;

const CURSOR: &str = "\u{2588}";

/// Confirm completing the TODO reference attached to the embedded session.
pub fn draw_todo_reference_completion_dialog(
    frame: &mut Frame,
    _state: &TodoReferenceCompletionState,
    theme: &Theme,
) {
    let area = centered_rect(54, 20, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);
    let block = Block::default()
        .title(" Complete referenced TODO ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.warning.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("Mark this session's referenced TODO complete?"),
            Line::from(""),
            Line::from(vec![
                Span::styled("Enter/y", Style::default().fg(theme.success.to_color())),
                Span::raw(" confirm  "),
                Span::styled("Esc/n", Style::default().fg(theme.text_muted.to_color())),
                Span::raw(" cancel"),
            ]),
        ])
        .wrap(Wrap { trim: true }),
        inner,
    );
}

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

    // Name the list this will land in. Quick capture writes to the session
    // feature's own worktree list, which is not the same list the project's
    // other features see — so it says which one rather than leaving it to be
    // inferred from the title bar.
    let input = Paragraph::new(vec![
        Line::from(vec![
            Span::styled(" Title: ", Style::default().fg(theme.primary.to_color())),
            Span::styled(&state.input, Style::default().fg(theme.text.to_color())),
            Span::styled(CURSOR, Style::default().fg(theme.primary.to_color())),
        ]),
        Line::from(vec![
            Span::styled(
                " Adding to: ",
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled(
                &state.list_label,
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]),
    ]);
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
pub fn draw_todos_view_with_visibility(
    frame: &mut Frame,
    state: &TodoViewState,
    theme: &Theme,
    nerd_font: bool,
    project_visible: bool,
    global_visible: bool,
) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }

    frame.render_widget(
        Block::default().style(Style::default().bg(theme.effective_bg())),
        area,
    );

    // Header (1) + panes (min) + hint (1).
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

    draw_header(
        frame,
        chunks[0],
        state,
        project_visible,
        global_visible,
        theme,
    );
    draw_panes(
        frame,
        chunks[1],
        state,
        project_visible,
        global_visible,
        theme,
        nerd_font,
    );
    draw_hint(
        frame,
        chunks[2],
        state,
        project_visible,
        global_visible,
        theme,
    );

    // Overlays on top of the panes, in the same precedence the key handler
    // uses: delete confirmation, the launch step, the scope chooser, then an
    // inline edit.
    if state.pending_delete {
        draw_delete_confirm(frame, state, theme);
    } else if let Some(step) = &state.launch {
        draw_launch_step(frame, state, step, theme);
    } else if let Some(step) = &state.scope_move {
        draw_scope_move(frame, step, theme);
    } else if let Some(editor) = &state.editor {
        draw_editor(frame, editor, theme);
    }
}

/// How many panes fit side by side at this width.
///
/// The thresholds are about legibility, not arithmetic: a pane narrower than
/// roughly forty columns cannot show a priority marker, a checkbox, and a
/// useful amount of title, so a third pane that would push the others under
/// that is not drawn at all — focus cycling reaches it instead.
fn pane_capacity(width: u16) -> usize {
    if width >= 120 {
        3
    } else if width >= 72 {
        2
    } else {
        1
    }
}

/// Which panes get a slot, in draw order.
///
/// Two rules, in this order: the focused pane is *always* drawn — hiding the
/// pane that owns the cursor would leave the user typing into nothing — and
/// the worktree pane keeps its slot whenever there is room for a second, since
/// it is the list this feature's work actually belongs to.
fn pane_slots(
    state: &TodoViewState,
    width: u16,
    project_visible: bool,
    global_visible: bool,
) -> Vec<usize> {
    let visible = state.visible_pane_indices(project_visible, global_visible);
    if visible.is_empty() {
        return Vec::new();
    }
    let slots = pane_capacity(width).min(visible.len());

    let mut chosen = vec![
        state
            .focus
            .filter(|focus| visible.contains(focus))
            .unwrap_or(visible[0]),
    ];
    let has_worktree = state
        .panes
        .first()
        .is_some_and(|p| p.kind == TodoPaneKind::Worktree);
    if has_worktree && chosen.len() < slots && !chosen.contains(&0) {
        chosen.push(0);
    }
    for i in visible {
        if chosen.len() >= slots {
            break;
        }
        if !chosen.contains(&i) {
            chosen.push(i);
        }
    }
    chosen.truncate(slots);
    chosen.sort_unstable();
    chosen
}

fn draw_panes(
    frame: &mut Frame,
    area: Rect,
    state: &TodoViewState,
    project_visible: bool,
    global_visible: bool,
    theme: &Theme,
    nerd_font: bool,
) {
    let hidden: Vec<&TodoPane> = state
        .panes
        .iter()
        .filter(|pane| !TodoViewState::pane_is_visible(pane, project_visible, global_visible))
        .collect();
    let slots = pane_slots(state, area.width, project_visible, global_visible);
    let mut row_constraints = vec![Constraint::Length(3); hidden.len()];
    if !slots.is_empty() {
        row_constraints.push(Constraint::Min(3));
    }
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(row_constraints)
        .split(area);

    for (row, pane) in rows.iter().zip(hidden.iter()) {
        draw_hidden_placeholder(frame, *row, pane.kind, theme);
    }
    if slots.is_empty() {
        return;
    }
    let actionable_area = rows[hidden.len()];
    let constraints: Vec<Constraint> = slots
        .iter()
        .map(|_| Constraint::Ratio(1, slots.len() as u32))
        .collect();
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints(constraints)
        .split(actionable_area);

    for (column, &pane_index) in columns.iter().zip(slots.iter()) {
        if let Some(pane) = state.panes.get(pane_index) {
            draw_pane(
                frame,
                *column,
                pane,
                state.focus == Some(pane_index),
                theme,
                nerd_font,
            );
        }
    }
}

fn draw_hidden_placeholder(frame: &mut Frame, area: Rect, kind: TodoPaneKind, theme: &Theme) {
    let key = match kind {
        TodoPaneKind::Project => 'p',
        TodoPaneKind::Global => 'g',
        TodoPaneKind::Worktree => return,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.text_muted.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("{} TODOs hidden — {key} to show", kind.label()),
            Style::default().fg(theme.text_muted.to_color()),
        ))),
        inner,
    );
}

/// One scope's pane: a bordered block titled with the scope, its scratchpad
/// banner when it has one, and its items.
///
/// The focused pane gets the accent border and a visible cursor; an unfocused
/// pane still shows where its cursor is, dimmed, so switching back lands
/// somewhere predictable.
fn draw_pane(
    frame: &mut Frame,
    area: Rect,
    pane: &TodoPane,
    focused: bool,
    theme: &Theme,
    nerd_font: bool,
) {
    let open = pane
        .todos
        .iter()
        .filter(|t| !t.work.status.is_completed())
        .count();
    let done = pane.todos.len().saturating_sub(open);
    let in_progress = pane
        .todos
        .iter()
        .filter(|t| t.work.status.is_in_progress())
        .count();
    // Only shown when there is something underway: a permanent "0 in progress"
    // is noise on a list nobody has started.
    let progress_label = if in_progress > 0 {
        format!(", {in_progress} wip")
    } else {
        String::new()
    };
    let name = if pane.title.is_empty() {
        pane.kind.label().to_string()
    } else {
        format!("{} · {}", pane.kind.label(), pane.title)
    };

    let border_color = if focused {
        theme.primary.to_color()
    } else {
        theme.text_muted.to_color()
    };
    let mut title_style = Style::default().fg(border_color);
    if focused {
        title_style = title_style.add_modifier(Modifier::BOLD);
    }
    let block = Block::default()
        .title(Span::styled(format!(" {name} "), title_style))
        .title_bottom(Span::styled(
            format!(" {open} open{progress_label}, {done} done "),
            Style::default().fg(theme.text_muted.to_color()),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let scratchpad = pane.scratchpad();
    let mut constraints = Vec::new();
    if scratchpad.is_some() {
        constraints.push(Constraint::Length(2));
    }
    constraints.push(Constraint::Min(1));
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let mut idx = 0;
    if let Some(note) = scratchpad {
        draw_scratchpad(frame, rows[idx], note, theme);
        idx += 1;
    }
    draw_list(frame, rows[idx], pane, focused, theme, nerd_font);
}

fn draw_header(
    frame: &mut Frame,
    area: Rect,
    state: &TodoViewState,
    project_visible: bool,
    global_visible: bool,
    theme: &Theme,
) {
    let panes_label = format!(
        "  p project:{}  g global:{}",
        if project_visible { "shown" } else { "hidden" },
        if global_visible { "shown" } else { "hidden" },
    );
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
            panes_label,
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_scratchpad(frame: &mut Frame, area: Rect, note: &str, theme: &Theme) {
    let paragraph = Paragraph::new(Line::from(vec![
        Span::styled("scratch ", Style::default().fg(theme.warning.to_color())),
        Span::styled(note, Style::default().fg(theme.text.to_color())),
    ]))
    .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn draw_list(
    frame: &mut Frame,
    area: Rect,
    pane: &TodoPane,
    focused: bool,
    theme: &Theme,
    nerd_font: bool,
) {
    if pane.todos.is_empty() {
        let empty = Paragraph::new(Span::styled(
            " No TODOs yet.",
            Style::default().fg(theme.text_muted.to_color()),
        ));
        frame.render_widget(empty, area);
        return;
    }

    let visible = area.height as usize;
    // Keep the selected row in view.
    let scroll = if pane.selected >= pane.scroll_offset + visible {
        pane.selected + 1 - visible
    } else if pane.selected < pane.scroll_offset {
        pane.selected
    } else {
        pane.scroll_offset
    };

    let lines: Vec<Line> = pane
        .todos
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible)
        .map(|(i, todo)| todo_line(todo, i == pane.selected, focused, theme, nerd_font))
        .collect();

    frame.render_widget(Paragraph::new(lines), area);

    if pane.todos.len() > visible {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(pane.todos.len())
            .position(scroll)
            .viewport_content_length(visible);
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }
}

/// The move/copy scope chooser layered over the panes.
fn draw_scope_move(frame: &mut Frame, step: &TodoScopeMoveState, theme: &Theme) {
    let rows = (step.targets.len() as u16).min(6);
    let area = centered_rect(60, (34 + rows * 6).min(70), frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let (verb, detail) = if step.copy {
        (
            "Copy TODO to",
            "The original stays where it is. The copy starts unstarted — no session, no planned feature.",
        )
    } else {
        (
            "Move TODO to",
            "The item is re-filed as it is, keeping any session or feature already started for it.",
        )
    };

    let block = Block::default()
        .title(format!(" {verb} "))
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
                step.todo_title.clone(),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];
    for (i, (label, _)) in step.targets.iter().enumerate() {
        lines.push(option_line(
            label.clone(),
            i == step.selected,
            theme.primary.to_color(),
            theme,
        ));
    }
    lines.push(Line::from(""));
    lines.extend(detail_lines(detail, inner.width, theme));
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
            " back to list",
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
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
fn draw_launch_step(
    frame: &mut Frame,
    state: &TodoViewState,
    step: &TodoLaunchStep,
    theme: &Theme,
) {
    let pct_y = match step {
        // Three options, each with a detail line, plus header and footer.
        TodoLaunchStep::Choice { .. } => 56,
        TodoLaunchStep::Destination { .. } => 46,
    };
    let area = centered_rect(64, pct_y, frame.area());
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

/// "Implement next" landed on a TODO that already has work started for it.
///
/// Drawn over whichever surface asked — the dashboard or the TODOs list — so
/// the four options are the only thing that moves.
pub fn draw_todo_implement_choice_dialog(
    frame: &mut Frame,
    state: &TodoImplementChoiceState,
    theme: &Theme,
) {
    let area = centered_rect(64, 60, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Work has already started on this TODO ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.warning.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                state.todo_title.clone(),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "It is the next TODO in line, and every other one is done or underway.",
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]),
        Line::from(""),
    ];

    for (i, choice) in TodoImplementChoice::ALL.iter().enumerate() {
        lines.push(option_line(
            choice.label().to_string(),
            i == state.selected,
            theme.primary.to_color(),
            theme,
        ));
        lines.extend(detail_lines(choice.detail(), inner.width, theme));
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
            " confirm  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// One item's row. `selected` is the pane's own cursor; `focused` says whether
/// that pane owns the keyboard — an unfocused pane still shows where its
/// cursor sits, dimmed, so returning to it lands somewhere predictable.
fn todo_line<'a>(
    todo: &'a Todo,
    selected: bool,
    focused: bool,
    theme: &Theme,
    nerd_font: bool,
) -> Line<'a> {
    // The cursor row of the pane that owns the keyboard reads as the cursor;
    // the same row in a side pane is only a bookmark.
    let active = selected && focused;
    let (prio_marker, prio_color) = match todo.priority {
        TodoPriority::High => ("!", theme.danger.to_color()),
        TodoPriority::Med => ("·", theme.warning.to_color()),
        TodoPriority::Low => (" ", theme.text_muted.to_color()),
    };

    // Three states, not two: an item being worked reads differently from one
    // nobody has picked up, which is what makes "implement next" skipping it
    // legible rather than arbitrary.
    let checkbox = match todo.work.status {
        TodoStatus::Completed => "[x]",
        TodoStatus::InProgress => "[~]",
        TodoStatus::NotStarted => "[ ]",
    };
    let cursor = if selected { "› " } else { "  " };
    let cursor_color = if active {
        theme.primary.to_color()
    } else {
        theme.text_muted.to_color()
    };

    let title_style = if todo.work.status.is_completed() {
        Style::default()
            .fg(theme.text_muted.to_color())
            .add_modifier(Modifier::CROSSED_OUT)
    } else if todo.work.status.is_in_progress() {
        Style::default()
            .fg(theme.warning.to_color())
            .add_modifier(if active {
                Modifier::BOLD
            } else {
                Modifier::empty()
            })
    } else if active {
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
    let launched_indicator = if todo.work.agent_session_id.is_some() {
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
        Span::styled(cursor, Style::default().fg(cursor_color)),
        Span::styled(
            format!("{prio_marker} "),
            Style::default().fg(prio_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{checkbox} "),
            Style::default().fg(if todo.work.status.is_completed() {
                theme.success.to_color()
            } else if todo.work.status.is_in_progress() {
                theme.warning.to_color()
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

/// The key hint bar. It names the pane keys only when there is more than one
/// pane to move between, so a single-pane view does not advertise a `Tab` that
/// would do nothing.
fn draw_hint(
    frame: &mut Frame,
    area: Rect,
    state: &TodoViewState,
    project_visible: bool,
    global_visible: bool,
    theme: &Theme,
) {
    let base = "  j/k move  a add  e title  o notes  space state  P prio  J/K reorder  Enter start/plan  I next  b scratch  M/C move/copy  d del  p/g scopes  Esc/q close";
    let text = if state
        .visible_pane_indices(project_visible, global_visible)
        .len()
        > 1
    {
        format!("  Tab pane{base}")
    } else {
        base.to_string()
    };
    let hint = Line::from(vec![Span::styled(
        text,
        Style::default().fg(theme.text_muted.to_color()),
    )]);
    frame.render_widget(Paragraph::new(hint), area);
}

/// Title and the leading (keymap-independent) part of the hint for each edit
/// target. `draw_editor` appends the cancel key and vim affordance, which
/// differ between plain and vim mode.
fn editor_chrome(target: &TodoEditTarget) -> (&'static str, &'static str) {
    match target {
        TodoEditTarget::New => (" New TODO ", "Enter: add"),
        TodoEditTarget::Title => (" Edit title ", "Enter: save"),
        TodoEditTarget::Notes => (" Edit notes ", "Enter: save   Alt+Enter: newline"),
        TodoEditTarget::Scratchpad => (" Scratchpad ", "Enter: save   Alt+Enter: newline"),
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

    let (title, base_hint) = editor_chrome(&editor.target);
    // The cancel key and vim affordance depend on the keymap: plain mode cancels
    // on Esc, vim mode gives Esc to the editor (Insert→Normal) and cancels on
    // Ctrl+Q. The mode indicator leads the line so it survives right-truncation
    // in a modal narrower than the full hint.
    let hint = match editor.editor.vim_mode() {
        None => format!("{base_hint}   Esc: cancel   Ctrl+T: vim"),
        Some(crate::editor::VimMode::Normal) => {
            format!("NORMAL   {base_hint}   Ctrl+Q: cancel   Ctrl+T: vim off")
        }
        Some(crate::editor::VimMode::Insert) => {
            format!("INSERT   {base_hint}   Ctrl+Q: cancel   Ctrl+T: vim off")
        }
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
        .focused()
        .and_then(|pane| pane.selected_todo())
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

/// "Which feature should work this TODO?" — raised by a spawn from the project
/// or global pane, where nothing in the list names a checkout.
pub fn draw_todo_spawn_target_dialog(
    frame: &mut Frame,
    state: &TodoSpawnTargetState,
    theme: &Theme,
) {
    let rows = (state.candidates.len() as u16).min(8);
    let area = centered_rect(64, (34 + rows * 5).min(80), frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Where should this TODO be worked? ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let scope = match state.pane_kind {
        TodoPaneKind::Global => "the global list",
        _ => "the project list",
    };
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                state.todo.title.clone(),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                format!("lives in {scope}, which belongs to no one checkout."),
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]),
        Line::from(""),
    ];

    // A long candidate list scrolls with the cursor rather than overflowing.
    let visible = (inner.height as usize).saturating_sub(8).max(1);
    let start = state.selected.saturating_sub(visible.saturating_sub(1));
    for (i, (label, _, _)) in state
        .candidates
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
    {
        lines.push(option_line(
            label.clone(),
            i == state.selected,
            theme.primary.to_color(),
            theme,
        ));
    }

    lines.push(Line::from(""));
    lines.extend(detail_lines(
        "The feature you pick supplies the agent and mode, exactly as a worktree TODO's own feature would.",
        inner.width,
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
        Span::styled(" start  ", Style::default().fg(theme.text_muted.to_color())),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(" cancel", Style::default().fg(theme.text_muted.to_color())),
    ]));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// "What happens to this worktree's unfinished TODOs?" — the blocking prompt
/// before a feature whose worktree list still holds open work is deleted.
pub fn draw_todo_delete_disposition_dialog(
    frame: &mut Frame,
    state: &TodoDeleteDispositionState,
    theme: &Theme,
) {
    let area = centered_rect(64, 62, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" This worktree still has TODOs ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.warning.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items = if state.unfinished == 1 {
        "TODO"
    } else {
        "TODOs"
    };
    let mut lines = vec![
        Line::from(""),
        Line::from(vec![
            Span::raw(" Feature "),
            Span::styled(
                state.feature_name.as_str(),
                Style::default()
                    .fg(theme.danger.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" has "),
            Span::styled(
                format!("{} unfinished {items}", state.unfinished),
                Style::default().fg(theme.text.to_color()),
            ),
            Span::raw(" in its worktree list."),
        ]),
        Line::from(vec![Span::styled(
            " Deleting the feature deletes that list, so they need somewhere to go:",
            Style::default().fg(theme.text_muted.to_color()),
        )]),
        Line::from(""),
    ];

    for (i, choice) in TodoDeleteDisposition::ALL.iter().enumerate() {
        let accent = match choice {
            TodoDeleteDisposition::Delete => theme.danger.to_color(),
            TodoDeleteDisposition::Cancel => theme.text_muted.to_color(),
            _ => theme.primary.to_color(),
        };
        lines.push(option_line(
            choice.label().to_string(),
            i == state.selected,
            accent,
            theme,
        ));
        lines.extend(detail_lines(choice.detail(), inner.width, theme));
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
            " confirm  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::styled(
            " cancel the deletion",
            Style::default().fg(theme.text_muted.to_color()),
        ),
    ]));

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn hidden_root_view() -> TodoViewState {
        let pane = |kind, scope, title: &str| TodoPane {
            kind,
            scope,
            title: title.to_string(),
            list: None,
            todos: vec![Todo {
                id: format!("todo-{title}"),
                list_id: "list".to_string(),
                title: format!("secret {title} contents"),
                body: None,
                priority: TodoPriority::Med,
                sort_order: 0,
                work: crate::db::todos::TodoWorkState::default(),
                linked_feature_id: None,
                created_at: String::new(),
                updated_at: String::new(),
            }],
            selected: 0,
            scroll_offset: 0,
        };
        TodoViewState {
            pi: 0,
            fi: 0,
            project_name: "project".to_string(),
            feature_name: "root feature".to_string(),
            panes: vec![
                pane(
                    TodoPaneKind::Project,
                    crate::db::todos::TodoScope::Project {
                        project_id: "project-id".to_string(),
                    },
                    "project",
                ),
                pane(
                    TodoPaneKind::Global,
                    crate::db::todos::TodoScope::Global,
                    "global",
                ),
            ],
            focus: None,
            editor: None,
            todo_vim_enabled: false,
            pending_delete: false,
            launch: None,
            scope_move: None,
        }
    }

    #[test]
    fn both_hidden_root_scopes_render_placeholders_without_todo_contents() {
        let state = hidden_root_view();
        let backend = TestBackend::new(100, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_todos_view_with_visibility(
                    frame,
                    &state,
                    &Theme::default(),
                    false,
                    false,
                    false,
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
        assert!(rendered.contains("Project TODOs hidden — p to show"));
        assert!(rendered.contains("Global TODOs hidden — g to show"));
        assert!(!rendered.contains("secret project contents"));
        assert!(!rendered.contains("secret global contents"));
    }

    #[test]
    fn visible_scope_header_and_footer_advertise_the_new_keys() {
        let mut state = hidden_root_view();
        state.focus = Some(0);
        let backend = TestBackend::new(220, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                draw_todos_view_with_visibility(frame, &state, &Theme::default(), false, true, true)
            })
            .unwrap();

        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(rendered.contains("p project:shown"));
        assert!(rendered.contains("g global:shown"));
        assert!(rendered.contains("P prio"));
        assert!(rendered.contains("Enter start/plan"));
        assert!(rendered.contains("p/g scopes"));
        assert!(!rendered.contains("\\ hide side panes"));
    }
}
