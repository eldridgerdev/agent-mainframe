use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use crate::app::{TextSelection, ViewState};
use crate::project::VibeMode;
use crate::theme::Theme;

const LEADER_COMMANDS: &[(&str, &str)] = &[
    ("q", "Exit view"),
    ("t / T", "Next / prev session"),
    ("w", "Session switcher"),
    ("n / p", "Next / prev feature"),
    ("/", "Command picker"),
    ("i", "Pending inputs"),
    ("s", "Steering coach"),
    ("b", "Show/hide sidebar"),
    ("g", "Generate summary"),
    ("l", "Latest prompt"),
    ("v", "Expand/collapse todos"),
    ("d", "Diff viewer"),
    ("m", "Markdown viewer"),
    ("o", "Scroll mode"),
    ("r", "Refresh statuses"),
    ("x", "Stop session"),
    ("f", "Final review"),
    ("D", "Debug log"),
    ("?", "Help"),
];

const CLAUDE_SIDEBAR_WIDTH: u16 = 36;
const CLAUDE_SIDEBAR_MIN_MAIN_WIDTH: u16 = 72;

#[derive(Debug, Clone)]
pub(crate) struct ClaudeSidebarData {
    pub status_text: String,
    pub task_text: String,
    pub prompt_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentLayout {
    main: Rect,
    sidebar: Option<Rect>,
}

pub(crate) fn viewing_main_width(view: &ViewState, total_width: u16) -> u16 {
    sidebar_width(view, total_width)
        .map(|sidebar_width| total_width.saturating_sub(sidebar_width))
        .unwrap_or(total_width)
}

fn sidebar_width(view: &ViewState, total_width: u16) -> Option<u16> {
    if !view.has_claude_sidebar() {
        return None;
    }

    if total_width < CLAUDE_SIDEBAR_MIN_MAIN_WIDTH + CLAUDE_SIDEBAR_WIDTH {
        return None;
    }

    Some(CLAUDE_SIDEBAR_WIDTH)
}

fn split_content_area(content_area: Rect, view: &ViewState) -> ContentLayout {
    let Some(sidebar_width) = sidebar_width(view, content_area.width) else {
        return ContentLayout {
            main: content_area,
            sidebar: None,
        };
    };

    let main_width = content_area.width.saturating_sub(sidebar_width);
    if main_width == 0 {
        return ContentLayout {
            main: content_area,
            sidebar: None,
        };
    }

    ContentLayout {
        main: Rect::new(
            content_area.x,
            content_area.y,
            main_width,
            content_area.height,
        ),
        sidebar: Some(Rect::new(
            content_area.x + main_width,
            content_area.y,
            sidebar_width,
            content_area.height,
        )),
    }
}

fn rainbow_spans(text: &str, theme: &Theme) -> Vec<Span<'static>> {
    let colors = [
        theme.danger.to_color(),
        theme.warning.to_color(),
        theme.success.to_color(),
        theme.primary.to_color(),
        theme.info.to_color(),
        theme.secondary.to_color(),
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
    sidebar_data: Option<&ClaudeSidebarData>,
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
    let layout = split_content_area(content_area, view);
    let main_content_area = layout.main;

    // Single line header - minimal info
    let mut header_spans = vec![Span::raw("  ")];

    // Hide project/feature/session info when leader or scroll is active
    if !leader_active && !view.scroll_mode {
        header_spans.push(Span::styled(
            format!("{} ", view.project_name),
            Style::default()
                .fg(theme.project_title.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            format!("/{} ", view.feature_name),
            Style::default()
                .fg(theme.feature_title.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            format!("/{} ", view.session_label),
            Style::default().fg(theme.text.to_color()),
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
        };
        if view.review {
            header_spans.push(Span::styled(
                "[review] ",
                Style::default().fg(theme.mode_review.to_color()),
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
                .fg(theme.shortcut_text.to_color())
                .bg(theme.secondary.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        let help = if view.scroll_passthrough {
            "wheel/j/k:PgUp/Dn - q/Esc:exit"
        } else {
            "wheel/j/k:scroll PgUp/Dn:page - q/Esc:exit"
        };
        header_spans.push(Span::styled(
            help,
            Style::default().fg(theme.secondary.to_color()),
        ));
    } else if leader_active {
        header_spans.push(Span::styled(
            "|LEADER ",
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.shortcut_background.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            "press a command key",
            Style::default().fg(theme.shortcut_background.to_color()),
        ));
    } else {
        header_spans.push(Span::styled(
            "| ",
            Style::default().fg(theme.text_muted.to_color()),
        ));
        header_spans.push(Span::styled(
            "Ctrl+Space",
            Style::default().fg(theme.warning.to_color()),
        ));
        header_spans.push(Span::styled(
            " commands",
            Style::default().fg(theme.text.to_color()),
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
                .fg(theme.danger.to_color())
                .add_modifier(Modifier::BOLD),
        ));
    }

    let header = Paragraph::new(Line::from(header_spans))
        .style(Style::default().bg(theme.effective_header_bg()));
    frame.render_widget(header, header_area);

    if let Some(sidebar_area) = layout.sidebar {
        draw_claude_sidebar(
            frame,
            sidebar_area,
            sidebar_data,
            view.todos_expanded,
            theme,
        );
    }

    if view.scroll_mode && !view.scroll_passthrough {
        let text = scroll_content_to_lines(
            &view.scroll_content,
            view.scroll_offset,
            main_content_area.height,
        );
        let paragraph = Paragraph::new(text).style(
            Style::default()
                .fg(theme.text.to_color())
                .bg(theme.effective_bg()),
        );
        frame.render_widget(paragraph, main_content_area);
    } else {
        let text = ansi_to_ratatui_text_with_selection(
            pane_content,
            main_content_area.width,
            main_content_area.height,
            &view.selection,
            theme,
        );
        let paragraph = Paragraph::new(text).style(
            Style::default()
                .fg(theme.text.to_color())
                .bg(theme.effective_bg()),
        );
        frame.render_widget(paragraph, main_content_area);

        if !view.scroll_mode
            && let Some((cursor_x, cursor_y)) = tmux_cursor
        {
            let max_x = main_content_area.width.saturating_sub(1);
            let max_y = main_content_area.height.saturating_sub(1);
            let abs_x = main_content_area.x + cursor_x.min(max_x);
            let abs_y = main_content_area.y + cursor_y.min(max_y);
            let frame_max_x = frame.area().width.saturating_sub(1);
            let frame_max_y = frame.area().height.saturating_sub(1);
            frame.set_cursor_position(Position::new(
                abs_x.min(frame_max_x),
                abs_y.min(frame_max_y),
            ));
        }
    }

    if leader_active {
        draw_leader_menu(frame, main_content_area, theme);
    }
}

fn draw_claude_sidebar(
    frame: &mut Frame,
    area: Rect,
    data: Option<&ClaudeSidebarData>,
    todos_expanded: bool,
    theme: &Theme,
) {
    if area.width < 16 || area.height < 8 {
        return;
    }

    let block = Block::default()
        .title(Span::styled(
            " Claude Sidebar ",
            Style::default()
                .fg(theme.session_icon_claude.to_color())
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.border.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width == 0 || inner.height == 0 {
        return;
    }

    let fallback = ClaudeSidebarData {
        status_text: "No sidebar data available.".to_string(),
        task_text: "No task data yet.".to_string(),
        prompt_text: "No recent prompt.\nUse leader+l to open prompt history.".to_string(),
    };
    let data = data.unwrap_or(&fallback);
    let (sections, sections_with_content): (Vec<Rect>, Vec<(&str, &str)>) = if todos_expanded {
        (
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(sidebar_section_height(&data.status_text, 4, 7)),
                    Constraint::Min(10),
                ])
                .split(inner)
                .to_vec(),
            vec![
                ("Status", data.status_text.as_str()),
                ("Todos", data.task_text.as_str()),
            ],
        )
    } else {
        (
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(sidebar_section_height(&data.status_text, 4, 7)),
                    Constraint::Min(sidebar_section_height(&data.task_text, 11, 13)),
                    Constraint::Length(sidebar_section_height(&data.prompt_text, 4, 5)),
                ])
                .split(inner)
                .to_vec(),
            vec![
                ("Status", data.status_text.as_str()),
                ("Todos", data.task_text.as_str()),
                ("Prompt", data.prompt_text.as_str()),
            ],
        )
    };
    for ((title, body), section) in sections_with_content.iter().zip(sections.iter()) {
        let accent = sidebar_section_color(title, theme);
        let paragraph = Paragraph::new(styled_sidebar_lines(title, body, theme))
            .wrap(Wrap { trim: false })
            .style(Style::default().bg(theme.effective_bg()))
            .block(
                Block::default()
                    .title(sidebar_section_title(title, theme))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(accent)),
            );
        frame.render_widget(paragraph, *section);
    }
}

fn sidebar_section_title(title: &str, theme: &Theme) -> Line<'static> {
    let accent = sidebar_section_color(title, theme);
    let mut spans = vec![Span::styled(
        format!(" {} ", title),
        Style::default().fg(accent).add_modifier(Modifier::BOLD),
    )];

    if title == "Todos" {
        spans.push(Span::styled(
            "leader+v ",
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ));
    }

    Line::from(spans)
}

fn sidebar_section_color(title: &str, theme: &Theme) -> Color {
    match title {
        "Status" => theme.warning.to_color(),
        "Todos" => theme.success.to_color(),
        "Prompt" => theme.session_icon_claude.to_color(),
        _ => theme.border.to_color(),
    }
}

fn sidebar_section_height(body: &str, min_inner_lines: u16, max_inner_lines: u16) -> u16 {
    let inner_lines = body.lines().count() as u16;
    inner_lines.clamp(min_inner_lines, max_inner_lines) + 2
}

fn styled_sidebar_lines<'a>(title: &str, body: &'a str, theme: &Theme) -> Vec<Line<'a>> {
    body.lines()
        .map(|line| {
            if title == "Todos" {
                return styled_todo_line(line, theme);
            }
            if let Some((label, value)) = line.split_once(": ") {
                Line::from(vec![
                    Span::styled(
                        format!("{label}: "),
                        Style::default()
                            .fg(sidebar_section_color(title, theme))
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        value.to_string(),
                        sidebar_value_style(title, label, value, theme),
                    ),
                ])
            } else {
                Line::from(Span::styled(
                    line.to_string(),
                    sidebar_value_style(title, "", line, theme),
                ))
            }
        })
        .collect()
}

fn styled_todo_line<'a>(line: &'a str, theme: &Theme) -> Line<'a> {
    if line.starts_with("[x] ") {
        let text = line.trim_start_matches("[x] ");
        return Line::from(vec![
            Span::styled(
                "[x] ".to_string(),
                Style::default()
                    .fg(theme.success.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                text.to_string(),
                Style::default().fg(theme.text_muted.to_color()),
            ),
        ]);
    }

    if line.starts_with("[>] ") {
        let text = line.trim_start_matches("[>] ");
        return Line::from(vec![
            Span::styled(
                "[>] ".to_string(),
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                text.to_string(),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]);
    }

    if line.starts_with("[ ] ") {
        let text = line.trim_start_matches("[ ] ");
        return Line::from(vec![
            Span::styled(
                "[ ] ".to_string(),
                Style::default().fg(theme.text_muted.to_color()),
            ),
            Span::styled(text.to_string(), Style::default().fg(theme.text.to_color())),
        ]);
    }

    if line.starts_with("    ") {
        return Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.status_detail.to_color()),
        ));
    }

    Line::from(Span::styled(
        line.to_string(),
        Style::default().fg(theme.text_muted.to_color()),
    ))
}

fn sidebar_value_style(title: &str, label: &str, value: &str, theme: &Theme) -> Style {
    let lower = value.to_lowercase();
    let color = if label == "State" {
        match lower.as_str() {
            "active" => theme.status_active.to_color(),
            "idle" => theme.status_idle.to_color(),
            "stopped" => theme.status_stopped.to_color(),
            _ => theme.text.to_color(),
        }
    } else if label == "At" {
        theme.text_muted.to_color()
    } else if label == "Tool" {
        theme.info.to_color()
    } else if label == "Mode" {
        match lower.as_str() {
            "vibeless" => theme.mode_vibeless.to_color(),
            "vibe" => theme.mode_vibe.to_color(),
            "supervibe" => theme.mode_supervibe.to_color(),
            "review" => theme.mode_review.to_color(),
            _ => theme.text.to_color(),
        }
    } else if label == "Cost" {
        theme.warning.to_color()
    } else if lower.contains("waiting") {
        theme.status_waiting.to_color()
    } else if lower.contains("thinking") || lower.contains("running tool") {
        theme.info.to_color()
    } else if lower.contains("ready") {
        theme.success.to_color()
    } else if lower.contains("generating") {
        theme.info.to_color()
    } else if lower.contains("unavailable") {
        theme.text_muted.to_color()
    } else if title == "Todos" {
        theme.text.to_color()
    } else if matches!(label, "Input" | "Output" | "Effective") {
        theme.status_detail.to_color()
    } else if label == "Branch" {
        theme.text_muted.to_color()
    } else if title == "Prompt" {
        theme.text.to_color()
    } else {
        theme.text.to_color()
    };

    let mut style = Style::default().fg(color);
    if label == "State"
        || label == "Tool"
        || label == "Mode"
        || lower.contains("waiting")
        || lower.contains("thinking")
        || lower.contains("running tool")
        || lower.contains("ready")
        || lower.contains("generating")
    {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn draw_leader_menu(frame: &mut Frame, content_area: Rect, theme: &Theme) {
    if content_area.width < 30 || content_area.height < 8 {
        return;
    }

    let longest_label = LEADER_COMMANDS
        .iter()
        .map(|(key, desc)| key.len() + desc.len() + 4)
        .max()
        .unwrap_or(24) as u16;
    let width = (longest_label + 4).clamp(30, content_area.width.saturating_sub(2));
    let height = (LEADER_COMMANDS.len() as u16 + 2).min(content_area.height.saturating_sub(1));
    let x = content_area.x + content_area.width.saturating_sub(width + 1);
    let y = content_area.y + content_area.height.saturating_sub(height + 1);
    let area = Rect::new(x, y, width, height);

    let lines: Vec<Line<'static>> = LEADER_COMMANDS
        .iter()
        .map(|(key, desc)| {
            Line::from(vec![
                Span::styled(
                    format!("{:<6}", key),
                    Style::default()
                        .fg(theme.info.to_color())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(*desc, Style::default().fg(theme.text.to_color())),
            ])
        })
        .collect();

    let popup = Paragraph::new(lines).block(
        Block::default()
            .title(" Ctrl+Space commands ")
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.effective_bg()))
            .border_style(Style::default().fg(theme.info.to_color())),
    );

    frame.render_widget(Clear, area);
    frame.render_widget(popup, area);
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
    theme: &Theme,
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
                    .bg(theme.effective_selection_bg())
                    .fg(theme.text.to_color());
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

    if let Some(color) = vt100_color_to_ratatui(cell.fgcolor()) {
        style = style.fg(color);
    }
    if let Some(color) = vt100_color_to_ratatui(cell.bgcolor()) {
        style = style.bg(color);
    }

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

fn vt100_color_to_ratatui(color: vt100::Color) -> Option<ratatui::style::Color> {
    match color {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(ratatui::style::Color::Indexed(i)),
        vt100::Color::Rgb(r, g, b) => Some(ratatui::style::Color::Rgb(r, g, b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn sample_view(session_kind: crate::project::SessionKind) -> ViewState {
        ViewState::new(
            "proj".into(),
            "feat".into(),
            "amf-feat".into(),
            "claude".into(),
            "Claude".into(),
            session_kind,
            VibeMode::Vibeless,
            false,
        )
    }

    #[test]
    fn claude_sidebar_width_is_reserved_when_view_is_wide_enough() {
        let width = viewing_main_width(&sample_view(crate::project::SessionKind::Claude), 120);
        assert_eq!(width, 84);
    }

    #[test]
    fn non_claude_sessions_keep_full_width() {
        let width = viewing_main_width(&sample_view(crate::project::SessionKind::Codex), 120);
        assert_eq!(width, 120);
    }

    #[test]
    fn hidden_claude_sidebar_uses_full_width() {
        let mut view = sample_view(crate::project::SessionKind::Claude);
        view.sidebar_visible = false;
        let width = viewing_main_width(&view, 120);
        assert_eq!(width, 120);
    }

    #[test]
    fn claude_sidebar_shell_renders_in_view() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Claude);
        let theme = Theme::default();
        let sidebar = ClaudeSidebarData {
            status_text: "Waiting for input\nUsage: 1.2K tokens".into(),
            task_text: "Current: Investigate task tracking".into(),
            prompt_text: "Preview: Resume the task.".into(),
        };

        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &view,
                    "hello",
                    Some(&sidebar),
                    false,
                    0,
                    None,
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(rendered.contains("Claude Sidebar"));
        assert!(rendered.contains("Waiting for input"));
        assert!(!rendered.contains("Session"));
        assert!(rendered.contains("Todos"));
        assert!(rendered.contains("Prompt"));
    }

    #[test]
    fn sample_view_defaults_to_collapsed_todos() {
        let view = sample_view(crate::project::SessionKind::Claude);
        assert!(view.sidebar_visible);
        assert!(!view.todos_expanded);
    }
}
