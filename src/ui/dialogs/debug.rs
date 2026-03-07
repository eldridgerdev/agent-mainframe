use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
    Frame,
};
use unicode_width::UnicodeWidthStr;

use super::super::dashboard::centered_rect;
use crate::debug::{DebugLog, LogLevel};

pub fn draw_debug_log(frame: &mut Frame, debug_log: &DebugLog, scroll_offset: usize) {
    let area = centered_rect(80, 80, frame.area());
    frame.render_widget(Clear, area);

    let title = format!(" Debug Log - {} ", debug_log.log_file().display());
    let inner = {
        let block = Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Magenta));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        inner
    };

    if inner.height < 4 {
        return;
    }

    let entries: Vec<Line> = debug_log
        .entries()
        .iter()
        .map(|entry| {
            let level_color = match entry.level {
                LogLevel::Debug => Color::Gray,
                LogLevel::Info => Color::Green,
                LogLevel::Warn => Color::Yellow,
                LogLevel::Error => Color::Red,
            };
            let time = entry.timestamp.format("%H:%M:%S%.3f");
            Line::from(vec![
                Span::styled(format!("{} ", time), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("[{:<5}] ", entry.level.display()),
                    Style::default()
                        .fg(level_color)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{}: ", entry.context),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(&entry.message, Style::default().fg(Color::White)),
            ])
        })
        .collect();

    let content_area = {
        use ratatui::layout::{Constraint, Direction, Layout};
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(inner);
        chunks[0]
    };

    let visible_lines = content_area.height as usize;
    let mut wrap_width = content_area.width as usize;
    let mut total_visual_lines = count_wrapped_lines(debug_log, wrap_width);
    if total_visual_lines > visible_lines && wrap_width > 1 {
        // Reserve one column for the scrollbar and recompute wrapped height.
        wrap_width -= 1;
        total_visual_lines = count_wrapped_lines(debug_log, wrap_width);
    }
    let max_scroll = total_visual_lines.saturating_sub(visible_lines);
    let scroll_offset = scroll_offset.min(max_scroll);

    let paragraph = Paragraph::new(entries)
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

    let hint = Line::from(vec![
        Span::styled(
            "j/k:scroll  c:clear  Esc:close  ",
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("({} entries)", debug_log.len()),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    let hint_paragraph = Paragraph::new(hint).right_aligned();

    use ratatui::layout::{Constraint, Direction, Layout};
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(inner);
    frame.render_widget(hint_paragraph, chunks[1]);
}

fn count_wrapped_lines(debug_log: &DebugLog, width: usize) -> usize {
    if width == 0 {
        return 0;
    }

    debug_log
        .entries()
        .iter()
        .map(|entry| {
            let line = format!(
                "{} [{:<5}] {}: {}",
                entry.timestamp.format("%H:%M:%S%.3f"),
                entry.level.display(),
                entry.context,
                entry.message
            );
            UnicodeWidthStr::width(line.as_str()).max(1).div_ceil(width)
        })
        .sum()
}
