use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap},
};

use super::super::dashboard::centered_rect;
use crate::app::{TodoEditTarget, TodoEditor, TodoViewState};
use crate::db::todos::{Todo, TodoPriority};
use crate::theme::Theme;

const CURSOR: &str = "\u{2588}";

/// Full-screen native TODOs overlay: a "left off here" carry-over banner on
/// top, then the project's TODO items.
pub fn draw_todos_view(frame: &mut Frame, state: &TodoViewState, theme: &Theme, nerd_font: bool) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }

    frame.render_widget(
        Block::default().style(Style::default().bg(theme.effective_bg())),
        area,
    );

    let carry_over = state
        .list
        .as_ref()
        .and_then(|l| l.carry_over.as_deref())
        .filter(|s| !s.trim().is_empty());

    // Header (1) + optional carry-over banner (3) + list (min) + hint (1).
    let mut constraints = vec![Constraint::Length(1)];
    if carry_over.is_some() {
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
    if let Some(note) = carry_over {
        draw_carry_over(frame, chunks[idx], note, theme);
        idx += 1;
    }
    let list_area = chunks[idx];
    idx += 1;
    let hint_area = chunks[idx];

    draw_list(frame, list_area, state, theme, nerd_font);
    draw_hint(frame, hint_area, theme);

    // Overlays on top of the list.
    if let Some(editor) = &state.editor {
        draw_editor(frame, editor, theme);
    } else if state.pending_delete {
        draw_delete_confirm(frame, state, theme);
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

fn draw_carry_over(frame: &mut Frame, area: Rect, note: &str, theme: &Theme) {
    let block = Block::default()
        .title(" Left off here ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.warning.to_color()));
    let paragraph = Paragraph::new(Span::styled(note, Style::default().fg(theme.text.to_color())))
        .block(block)
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, area);
}

fn draw_list(
    frame: &mut Frame,
    area: Rect,
    state: &TodoViewState,
    theme: &Theme,
    nerd_font: bool,
) {
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

fn todo_line<'a>(
    todo: &'a Todo,
    selected: bool,
    theme: &Theme,
    nerd_font: bool,
) -> Line<'a> {
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
            if nerd_font { "  \u{f036}" } else { "  ≡" }
        }
        _ => "",
    };

    let mut spans = vec![
        Span::styled(
            cursor,
            Style::default().fg(theme.primary.to_color()),
        ),
        Span::styled(
            format!("{prio_marker} "),
            Style::default()
                .fg(prio_color)
                .add_modifier(Modifier::BOLD),
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
    Line::from(spans)
}

fn draw_hint(frame: &mut Frame, area: Rect, theme: &Theme) {
    let hint = Line::from(vec![Span::styled(
        "  j/k move  a add  e title  o notes  space done  p prio  J/K reorder  b banner  d del  Esc/q close",
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
        TodoEditTarget::CarryOver => (
            " Left off here ",
            "Enter: save   Alt+Enter: newline   Esc: cancel",
        ),
    }
}

fn draw_editor(frame: &mut Frame, editor: &TodoEditor, theme: &Theme) {
    let multiline = matches!(
        editor.target,
        TodoEditTarget::Notes | TodoEditTarget::CarryOver
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
        .map(|l| Line::from(Span::styled(l.to_string(), Style::default().fg(theme.text.to_color()))))
        .collect();
    frame.render_widget(
        Paragraph::new(body).wrap(Wrap { trim: false }),
        chunks[0],
    );

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
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }),
        inner,
    );
}
