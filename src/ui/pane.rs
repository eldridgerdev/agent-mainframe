use ratatui::{
    layout::{Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::app::ViewState;
use crate::project::VibeMode;

const RAINBOW_COLORS: &[Color] = &[
    Color::Red,
    Color::Rgb(255, 127, 0),
    Color::Yellow,
    Color::Green,
    Color::Cyan,
    Color::Blue,
    Color::Magenta,
];

fn rainbow_spans(text: &str) -> Vec<Span<'static>> {
    text.chars()
        .enumerate()
        .map(|(i, ch)| {
            let color = RAINBOW_COLORS[i % RAINBOW_COLORS.len()];
            Span::styled(
                ch.to_string(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

pub fn draw(
    frame: &mut Frame,
    view: &ViewState,
    pane_content: &str,
    leader_active: bool,
    pending_count: usize,
    tmux_cursor: Option<(u16, u16)>,
) {
    let area = frame.area();
    let header_area = Rect::new(area.x, area.y, area.width, 1);
    let content_area = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );

    // Single line header - minimal info
    let mut header_spans = vec![
        Span::raw("  "),
        Span::styled(
            format!("{} ", view.project_name),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("/{} ", view.feature_name),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("/{} ", view.session_label),
            Style::default().fg(Color::White),
        ),
    ];
    match view.vibe_mode {
        VibeMode::Vibeless => header_spans.push(Span::styled(
            "[vibeless] ",
            Style::default().fg(Color::Green),
        )),
        VibeMode::Vibe => {
            header_spans.push(Span::styled("[vibe] ", Style::default().fg(Color::Yellow)))
        }
        VibeMode::SuperVibe => {
            header_spans.push(Span::raw("["));
            header_spans.extend(rainbow_spans("supervibe"));
            header_spans.push(Span::raw("] "));
        }
        VibeMode::Review => header_spans.push(Span::styled(
            "[review] ",
            Style::default().fg(Color::Magenta),
        )),
    };
    if view.review {
        header_spans.push(Span::styled(
            "[review] ",
            Style::default().fg(Color::Cyan),
        ));
    }

    if view.scroll_mode {
        let scroll_pct = if view.scroll_total_lines > 0 && !view.scroll_passthrough {
            (view.scroll_offset as f64 / view.scroll_total_lines as f64 * 100.0) as u8
        } else {
            0
        };
        let mode_label = if view.scroll_passthrough {
            "APP"
        } else {
            &format!("{}%", scroll_pct)
        };
        header_spans.push(Span::styled(
            format!("|SCROLL {} ", mode_label),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        ));
        let help = if view.scroll_passthrough {
            "j/k:PgUp/Dn - q/Esc:exit"
        } else {
            "j/k:scroll PgUp/Dn:page - q/Esc:exit"
        };
        header_spans.push(Span::styled(help, Style::default().fg(Color::Magenta)));
    } else if leader_active {
        header_spans.push(Span::styled(
            "|LEADER ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            "q:exit t/T:cycle w:switcher n/p:feature /:commands i:inputs s:attach o:scroll x:stop f:review ?:help",
            Style::default().fg(Color::Yellow),
        ));
    } else {
        header_spans.push(Span::styled(
            "| ",
            Style::default().fg(Color::DarkGray),
        ));
        header_spans.push(Span::styled(
            "Ctrl+Space",
            Style::default().fg(Color::Yellow),
        ));
        header_spans.push(Span::styled(
            " commands",
            Style::default().fg(Color::White),
        ));
    }

    if pending_count > 0 && !view.scroll_mode {
        header_spans.push(Span::styled(
            format!(" | {} input{}",
                pending_count,
                if pending_count == 1 { "" } else { "s" },
            ),
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        ));
    }

    let header = Paragraph::new(Line::from(header_spans))
        .style(Style::default().bg(Color::Rgb(76, 79, 105)));
    frame.render_widget(header, header_area);

    if view.scroll_mode && !view.scroll_passthrough {
        let text = scroll_content_to_lines(
            &view.scroll_content,
            view.scroll_offset,
            content_area.height,
        );
        let paragraph = Paragraph::new(text);
        frame.render_widget(paragraph, content_area);
    } else {
        let text = ansi_to_ratatui_text(pane_content, content_area.width, content_area.height);
        let paragraph = Paragraph::new(text);
        frame.render_widget(paragraph, content_area);

        if !view.scroll_mode
            && let Some((cursor_x, cursor_y)) = tmux_cursor
        {
            let max_x = content_area.width.saturating_sub(1);
            let max_y = content_area.height.saturating_sub(1);
            let abs_x = content_area.x + cursor_x.min(max_x);
            let abs_y = content_area.y + cursor_y.min(max_y);
            frame.set_cursor_position(Position::new(abs_x, abs_y));
        }
    }
}

fn scroll_content_to_lines(content: &str, offset: usize, rows: u16) -> Vec<Line<'static>> {
    let all_lines: Vec<&str> = content.lines().collect();
    let total_lines = all_lines.len();
    let start = offset.min(total_lines);
    let end = (start + rows as usize).min(total_lines);

    let mut lines = Vec::with_capacity(rows as usize);
    for i in start..end {
        let line_text = all_lines.get(i).unwrap_or(&"");
        lines.push(Line::styled(
            strip_ansi_codes(line_text),
            Style::default().fg(Color::Reset),
        ));
    }
    while lines.len() < rows as usize {
        lines.push(Line::raw(""));
    }
    lines
}

fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            if let Some(&next) = chars.peek()
                && next == '['
            {
                chars.next();
                while let Some(&c) = chars.peek() {
                    chars.next();
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
                continue;
            }
        } else {
            result.push(ch);
        }
    }
    result
}

fn ansi_to_ratatui_text<'a>(raw: &str, cols: u16, rows: u16) -> Vec<Line<'a>> {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    let normalized = raw.replace('\n', "\r\n");
    parser.process(normalized.as_bytes());
    let screen = parser.screen();

    let mut lines = Vec::with_capacity(rows as usize);

    for row in 0..rows {
        let mut spans: Vec<Span<'a>> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();

        for col in 0..cols {
            let cell = screen.cell(row, col);
            let cell = match cell {
                Some(c) => c,
                None => continue,
            };

            let style = vt100_cell_to_style(cell);

            if style != current_style && !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }
            current_style = style;
            current_text.push_str(&cell.contents());
        }

        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }

        lines.push(Line::from(spans));
    }

    lines
}

fn vt100_cell_to_style(cell: &vt100::Cell) -> Style {
    let mut style = Style::default();

    style = style.fg(vt100_color_to_ratatui(cell.fgcolor()));
    style = style.bg(vt100_color_to_ratatui(cell.bgcolor()));

    if cell.bold() {
        style = style.add_modifier(Modifier::BOLD);
    }
    if cell.italic() {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if cell.underline() {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if cell.inverse() {
        style = style.add_modifier(Modifier::REVERSED);
    }

    style
}

fn vt100_color_to_ratatui(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
