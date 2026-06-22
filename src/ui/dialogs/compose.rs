use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, List, ListItem, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};

use crate::app::ComposeState;
use crate::editor::VimMode;
use crate::theme::Theme;

use super::editor_view::{count_wrapped_editor_lines, editor_lines, sync_editor_scroll};

// Tall enough that the box blankets the harness's own bottom UI
// (input prompt, any leftover typed text, and its status line).
const COMPOSE_MIN_INPUT_ROWS: usize = 6;
const COMPOSE_MAX_INPUT_ROWS: usize = 12;
const COMPOSE_MAX_SUGGESTION_ROWS: usize = 8;

/// Compose input drawn over the live pane, sized and positioned to
/// cover the harness's own input box at the bottom of the pane. The
/// pane above stays visible so the agent's output can be watched while
/// typing.
pub fn draw_compose_dialog(frame: &mut Frame, state: &mut ComposeState, theme: &Theme) {
    let area = frame.area();
    if area.width < 10 || area.height < 6 {
        return;
    }

    // Match the embedded pane: full main-pane width (excluding the
    // sidebar), flush with the bottom of the frame where the harness renders
    // its input box and status line.
    let width = crate::ui::viewing_main_width(&state.view, area.width);
    let x = area.x;

    let lines = editor_lines(
        &state.editor,
        theme,
        "Type your prompt — Enter sends, Alt+Enter for a newline, / for commands.",
    );

    let mut wrap_width = width.saturating_sub(2) as usize;
    let mut total_visual_lines = count_wrapped_editor_lines(&lines, wrap_width.max(1));
    // Never let the box (content + borders) outgrow the frame on
    // short terminals.
    let max_rows = (area.height.saturating_sub(4) as usize)
        .min(COMPOSE_MAX_INPUT_ROWS)
        .max(1);
    let visible_lines = total_visual_lines.clamp(COMPOSE_MIN_INPUT_ROWS.min(max_rows), max_rows);
    if total_visual_lines > visible_lines && wrap_width > 1 {
        wrap_width -= 1;
        total_visual_lines = count_wrapped_editor_lines(&lines, wrap_width);
    }
    sync_editor_scroll(
        &state.editor,
        &mut state.scroll_offset,
        &mut state.sync_scroll_to_cursor,
        visible_lines,
        wrap_width,
        total_visual_lines,
    );

    let box_height = (visible_lines as u16) + 2;
    let input_y = (area.y + area.height).saturating_sub(box_height);
    let input_area = Rect::new(x, input_y, width, box_height);

    let images_suffix = match state.images.len() {
        0 => String::new(),
        1 => " (1 image)".to_string(),
        n => format!(" ({n} images)"),
    };
    let title = match state.editor.vim_mode() {
        Some(VimMode::Insert) => format!(
            " Compose → {}{images_suffix} [Vim Insert] ",
            state.view.session_label
        ),
        Some(VimMode::Normal) => format!(
            " Compose → {}{images_suffix} [Vim Normal] ",
            state.view.session_label
        ),
        None => format!(" Compose → {}{images_suffix} ", state.view.session_label),
    };
    let hints = Line::from(vec![
        Span::styled(" Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" send  "),
        Span::styled("Alt+Enter", Style::default().fg(theme.warning.to_color())),
        Span::raw(" newline  "),
        Span::styled("Ctrl+V", Style::default().fg(theme.warning.to_color())),
        Span::raw(" paste  "),
        Span::styled("Esc", Style::default().fg(theme.warning.to_color())),
        Span::raw(" close  "),
        Span::styled("Ctrl+L", Style::default().fg(theme.warning.to_color())),
        Span::raw(" clear  "),
        Span::styled("Ctrl+T", Style::default().fg(theme.warning.to_color())),
        Span::raw(" vim  "),
        Span::styled("Ctrl+E", Style::default().fg(theme.warning.to_color())),
        Span::raw(" direct  "),
        Span::styled("Ctrl+P", Style::default().fg(theme.warning.to_color())),
        Span::raw(" library  "),
        Span::styled("Ctrl+S", Style::default().fg(theme.warning.to_color())),
        Span::raw(" save  "),
        Span::styled("Ctrl+Space", Style::default().fg(theme.warning.to_color())),
        Span::raw(" leader "),
    ]);

    let block = Block::default()
        .title(title)
        .title_bottom(hints)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));

    frame.render_widget(Clear, input_area);
    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((state.scroll_offset.min(u16::MAX as usize) as u16, 0));
    frame.render_widget(paragraph, input_area);

    if total_visual_lines > visible_lines {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(total_visual_lines)
            .position(state.scroll_offset)
            .viewport_content_length(visible_lines);
        frame.render_stateful_widget(scrollbar, input_area, &mut scrollbar_state);
    }

    draw_suggestions(frame, state, theme, x, width, input_y);
}

fn draw_suggestions(
    frame: &mut Frame,
    state: &ComposeState,
    theme: &Theme,
    x: u16,
    width: u16,
    input_y: u16,
) {
    if state.suggestions.is_empty() {
        return;
    }

    let rows = state.suggestions.len().min(COMPOSE_MAX_SUGGESTION_ROWS);
    let popup_height = (rows as u16) + 2;
    let popup_y = input_y.saturating_sub(popup_height);
    let popup_area = Rect::new(x, popup_y, width, popup_height);

    // Window the list so the selection stays visible.
    let start = state
        .suggestion_index
        .saturating_sub(rows.saturating_sub(1));
    let name_width = state
        .suggestions
        .iter()
        .filter_map(|idx| state.catalog.get(*idx))
        .map(|entry| entry.name.len() + 1)
        .max()
        .unwrap_or(0)
        .min(28);

    let items: Vec<ListItem> = state
        .suggestions
        .iter()
        .enumerate()
        .skip(start)
        .take(rows)
        .filter_map(|(pos, idx)| {
            let entry = state.catalog.get(*idx)?;
            let selected = pos == state.suggestion_index;
            let marker = if selected { "▸ " } else { "  " };
            let name_style = if selected {
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.text.to_color())
            };

            let mut spans = vec![
                Span::styled(marker.to_string(), name_style),
                Span::styled(format!("/{:<name_width$}", entry.name), name_style),
                Span::styled(
                    format!(" {:<8} ", entry.source.label()),
                    Style::default().fg(theme.info.to_color()),
                ),
            ];
            if !entry.description.is_empty() {
                spans.push(Span::styled(
                    entry.description.clone(),
                    Style::default().fg(theme.text_muted.to_color()),
                ));
            }
            Some(ListItem::new(Line::from(spans)))
        })
        .collect();

    let title = format!(
        " Commands ({}/{}) — ↑/↓ select · Tab complete ",
        state.suggestion_index + 1,
        state.suggestions.len()
    );
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.info.to_color()));

    frame.render_widget(Clear, popup_area);
    frame.render_widget(List::new(items).block(block), popup_area);
}
