use ratatui::{
    Frame,
    layout::{Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::app::{TextSelection, ViewState};
use crate::project::VibeMode;
use crate::theme::Theme;

fn rainbow_spans(text: &str, theme: &Theme) -> Vec<Span<'static>> {
    let colors = [
        theme.error.to_color(),
        theme.warning.to_color(),
        theme.success.to_color(),
        theme.accent.to_color(),
        theme.info.to_color(),
        theme.accent_alt.to_color(),
    ];
    text.chars()
        .enumerate()
        .map(|(i, ch)| {
            let color = colors[i % colors.len()];
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
    theme: &Theme,
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
    let mut header_spans = vec![Span::raw("  ")];

    // Hide project/feature/session info when leader or scroll is active
    if !leader_active && !view.scroll_mode {
        header_spans.push(Span::styled(
            format!("{} ", view.project_name),
            Style::default()
                .fg(theme.project_name.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            format!("/{} ", view.feature_name),
            Style::default()
                .fg(theme.feature_name.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            format!("/{} ", view.session_label),
            Style::default().fg(theme.fg.to_color()),
        ));
        match view.vibe_mode {
            VibeMode::Vibeless => header_spans.push(Span::styled(
                "[vibeless] ",
                Style::default().fg(theme.mode_vibeless.to_color()),
            )),
            VibeMode::Vibe => header_spans.push(Span::styled(
                "[vibe] ",
                Style::default().fg(theme.mode_vibe.to_color()),
            )),
            VibeMode::SuperVibe => {
                header_spans.push(Span::raw("["));
                header_spans.extend(rainbow_spans("supervibe", theme));
                header_spans.push(Span::raw("] "));
            }
            VibeMode::Review => header_spans.push(Span::styled(
                "[review] ",
                Style::default().fg(theme.mode_review.to_color()),
            )),
        };
        if view.review {
            header_spans.push(Span::styled(
                "[review] ",
                Style::default().fg(theme.accent.to_color()),
            ));
        }
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
                .fg(theme.leader_fg.to_color())
                .bg(theme.accent_alt.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        let help = if view.scroll_passthrough {
            "j/k:PgUp/Dn - q/Esc:exit"
        } else {
            "j/k:scroll PgUp/Dn:page - q/Esc:exit"
        };
        header_spans.push(Span::styled(
            help,
            Style::default().fg(theme.accent_alt.to_color()),
        ));
    } else if leader_active {
        header_spans.push(Span::styled(
            "|LEADER ",
            Style::default()
                .fg(theme.leader_fg.to_color())
                .bg(theme.leader_bg.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            " q:exit  t/T:cycle  w:switcher  n/p:feature  /:commands  i:inputs  o:scroll  x:stop  f:review  ?:help",
            Style::default().fg(theme.leader_bg.to_color()),
        ));
    } else {
        header_spans.push(Span::styled(
            "| ",
            Style::default().fg(theme.muted.to_color()),
        ));
        header_spans.push(Span::styled(
            "Ctrl+Space",
            Style::default().fg(theme.warning.to_color()),
        ));
        header_spans.push(Span::styled(
            " commands",
            Style::default().fg(theme.fg.to_color()),
        ));
    }

    if pending_count > 0 && !view.scroll_mode {
        header_spans.push(Span::styled(
            format!(
                " | {} input{}",
                pending_count,
                if pending_count == 1 { "" } else { "s" },
            ),
            Style::default()
                .fg(theme.error.to_color())
                .add_modifier(Modifier::BOLD),
        ));
    }

    let header = Paragraph::new(Line::from(header_spans))
        .style(Style::default().bg(theme.effective_header_bg()));
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
        let text = ansi_to_ratatui_text_with_selection(
            pane_content,
            content_area.width,
            content_area.height,
            &view.selection,
        );
        let paragraph = Paragraph::new(text);
        frame.render_widget(paragraph, content_area);

        if !view.scroll_mode
            && let Some((cursor_x, cursor_y)) = tmux_cursor
        {
            let max_x = content_area.width.saturating_sub(1);
            let max_y = content_area.height.saturating_sub(1);
            let abs_x = content_area.x + cursor_x.min(max_x);
            let abs_y = content_area.y + cursor_y.min(max_y);
            let frame_max_x = frame.area().width.saturating_sub(1);
            let frame_max_y = frame.area().height.saturating_sub(1);
            frame.set_cursor_position(Position::new(
                abs_x.min(frame_max_x),
                abs_y.min(frame_max_y),
            ));
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
        lines.push(Line::raw(strip_ansi_codes(line_text)));
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

fn ansi_to_ratatui_text_with_selection<'a>(
    raw: &str,
    cols: u16,
    rows: u16,
    selection: &TextSelection,
) -> Vec<Line<'a>> {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    let normalized = raw.replace('\n', "\r\n");
    parser.process(normalized.as_bytes());
    let screen = parser.screen();

    let (sel_start_row, sel_start_col, sel_end_row, sel_end_col) = selection.normalized();
    let has_selection = selection.has_selection;

    let mut lines = Vec::with_capacity(rows as usize);

    for row in 0..rows {
        let mut spans: Vec<Span<'a>> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();
        let mut in_selection = false;

        for col in 0..cols {
            let is_selected = has_selection
                && ((row > sel_start_row && row < sel_end_row)
                    || (row == sel_start_row
                        && row == sel_end_row
                        && col >= sel_start_col
                        && col < sel_end_col)
                    || (row == sel_start_row && row < sel_end_row && col >= sel_start_col)
                    || (row > sel_start_row && row == sel_end_row && col < sel_end_col));

            if is_selected != in_selection && !current_text.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current_text),
                    current_style,
                ));
            }
            in_selection = is_selected;

            let cell = screen.cell(row, col);
            let cell = match cell {
                Some(c) => c,
                None => continue,
            };

            let mut style = vt100_cell_to_style(cell);
            if is_selected {
                style = style
                    .bg(ratatui::style::Color::Rgb(70, 100, 140))
                    .fg(ratatui::style::Color::White);
            }

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

fn vt100_color_to_ratatui(color: vt100::Color) -> ratatui::style::Color {
    match color {
        vt100::Color::Default => ratatui::style::Color::Reset,
        vt100::Color::Idx(i) => ratatui::style::Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => ratatui::style::Color::Rgb(r, g, b),
    }
}
