use ratatui::{
    Frame,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};
use unicode_width::UnicodeWidthStr;

use crate::app::MarkdownViewerState;
use crate::theme::Theme;

use super::super::dashboard::centered_rect;

pub fn draw_markdown_viewer(frame: &mut Frame, state: &mut MarkdownViewerState, theme: &Theme) {
    let area = centered_rect(86, 86, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" Markdown - {} ", state.title);
    let inner = {
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.effective_bg()))
            .border_style(Style::default().fg(theme.info.to_color()));
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
            .constraints([Constraint::Min(1), Constraint::Length(2)])
            .split(inner)
    };
    let content_area = chunks[0];

    let lines = crate::markdown::render_markdown(&state.content, theme);
    let visible_lines = content_area.height as usize;
    let mut wrap_width = content_area.width as usize;
    let mut total_visual_lines = count_wrapped_lines(&lines, wrap_width);
    if total_visual_lines > visible_lines && wrap_width > 1 {
        wrap_width -= 1;
        total_visual_lines = count_wrapped_lines(&lines, wrap_width);
    }
    let max_scroll = total_visual_lines.saturating_sub(visible_lines);
    state.scroll_offset = state.scroll_offset.min(max_scroll);
    let scroll_offset = state.scroll_offset;

    let paragraph = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((scroll_offset.min(u16::MAX as usize) as u16, 0));
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

    let hints = Paragraph::new(Line::from(vec![
        Span::styled(
            "j/k:scroll  PgUp/PgDn:page  g/G:top/bottom  Esc:close  ",
            Style::default().fg(theme.text_muted.to_color()),
        ),
        Span::styled(
            state.source_path.display().to_string(),
            Style::default()
                .fg(theme.secondary.to_color())
                .add_modifier(Modifier::ITALIC),
        ),
    ]))
    .wrap(Wrap { trim: false });
    frame.render_widget(hints, chunks[1]);
}

fn count_wrapped_lines(lines: &[Line<'static>], width: usize) -> usize {
    if width == 0 {
        return 0;
    }

    lines.iter()
        .map(|line| {
            let text = line
                .spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>();
            UnicodeWidthStr::width(text.as_str()).max(1).div_ceil(width)
        })
        .sum()
}
