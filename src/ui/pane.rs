use ratatui::{
    layout::{Constraint, Direction, Layout, Position},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
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
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1)])
        .split(frame.area());

    let mut header_spans = vec![
        Span::styled(
            format!(" {} ", view.project_name),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("/ {} ", view.feature_name),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("/ {} ", view.session_label),
            Style::default().fg(Color::DarkGray),
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
    };

    if leader_active {
        header_spans.push(Span::styled(
            "| LEADER ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        header_spans.push(Span::styled(
            " q:exit t/T:cycle w:switcher n/p:feature /:commands i:inputs s:attach x:stop ?:help",
            Style::default().fg(Color::Yellow),
        ));
    } else {
        header_spans.push(Span::styled("| ", Style::default().fg(Color::DarkGray)));
        header_spans.push(Span::styled(
            "Ctrl+Space",
            Style::default().fg(Color::Yellow),
        ));
        header_spans.push(Span::styled(
            " command palette",
            Style::default().fg(Color::DarkGray),
        ));
    }

    if pending_count > 0 {
        header_spans.push(Span::styled(
            format!(
                " | {} input{}",
                pending_count,
                if pending_count == 1 { "" } else { "s" },
            ),
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }

    let border_color = if leader_active {
        Color::Yellow
    } else {
        Color::Cyan
    };

    let header = Paragraph::new(Line::from(header_spans)).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color)),
    );
    frame.render_widget(header, chunks[0]);

    let content_area = chunks[1];
    let text = ansi_to_ratatui_text(pane_content, content_area.width, content_area.height);
    let paragraph = Paragraph::new(text);
    frame.render_widget(paragraph, content_area);

    if let Some((cursor_x, cursor_y)) = tmux_cursor {
        let abs_x = content_area.x + cursor_x;
        let abs_y = content_area.y + cursor_y.saturating_sub(1);
        frame.set_cursor_position(Position::new(abs_x, abs_y));
    }
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
