use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use crate::app::{MarkdownLoadingState, MarkdownViewerState};
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_markdown_viewer(frame: &mut Frame, state: &mut MarkdownViewerState, theme: &Theme) {
    let area = centered_rect(86, 86, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let title = format!(" Markdown - {} ", state.title);
    let inner = {
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.effective_header_bg()))
            .border_style(
                Style::default()
                    .fg(theme.info.to_color())
                    .add_modifier(Modifier::BOLD),
            );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    };

    if inner.height < 4 {
        return;
    }

    let chunks = {
        use ratatui::layout::{Constraint, Direction, Layout};
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(inner)
    };
    let content_area = chunks[0];

    let visible_lines = content_area.height as usize;
    let render_width = content_area.width.saturating_sub(1).max(1);
    if state.rendered_width != render_width || state.rendered_lines.is_empty() {
        state.rendered_lines = crate::markdown::render_markdown(
            &state.content,
            theme,
            render_width as usize,
            Some(&state.source_path),
        )
        .lines;
        state.rendered_width = render_width;
    }
    let total_visual_lines = state.rendered_lines.len();
    let max_scroll = total_visual_lines.saturating_sub(visible_lines);
    state.scroll_offset = state.scroll_offset.min(max_scroll);
    let scroll_offset = state.scroll_offset;

    let visible = state
        .rendered_lines
        .iter()
        .skip(scroll_offset)
        .take(visible_lines)
        .cloned()
        .collect::<Vec<_>>();
    let paragraph = Paragraph::new(visible).style(Style::default().bg(theme.effective_header_bg()));
    frame.render_widget(paragraph, content_area);

    if total_visual_lines > visible_lines {
        let scrollbar = Scrollbar::default()
            .orientation(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));
        let mut scrollbar_state = ScrollbarState::new(total_visual_lines)
            .position(scroll_offset)
            .viewport_content_length(visible_lines);
        frame.render_stateful_widget(scrollbar, area, &mut scrollbar_state);
    }

    let hints = Paragraph::new(vec![
        Line::from(Span::styled(
            state.source_path.display().to_string(),
            Style::default()
                .fg(theme.secondary.to_color())
                .add_modifier(Modifier::ITALIC),
        )),
        Line::from(Span::styled(
            if state.return_to_picker.is_some() {
                "j/k:scroll  Ctrl+j/k:fast  PgUp/PgDn:page  g/G:top/bottom  /:files  b:back  Esc:close"
            } else {
                "j/k:scroll  Ctrl+j/k:fast  PgUp/PgDn:page  g/G:top/bottom  /:files  Esc:close"
            },
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ])
    .style(Style::default().bg(theme.effective_header_bg()));
    frame.render_widget(hints, chunks[1]);
}

pub fn draw_markdown_loading(
    frame: &mut Frame,
    state: &MarkdownLoadingState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let area = centered_rect(54, 28, frame.area());
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Markdown ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_header_bg()))
        .border_style(
            Style::default()
                .fg(theme.info.to_color())
                .add_modifier(Modifier::BOLD),
        );
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let throbber = throbber_widgets_tui::Throbber::default()
        .style(Style::default().fg(theme.warning.to_color()));
    let spinner = throbber.to_symbol_span(throbber_state);

    let loading = Paragraph::new(vec![
        Line::from(""),
        Line::from(vec![
            spinner,
            Span::styled(
                " Loading markdown...",
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            state.title.clone(),
            Style::default().fg(theme.text_muted.to_color()),
        )),
    ]);

    frame.render_widget(loading, inner);
}
