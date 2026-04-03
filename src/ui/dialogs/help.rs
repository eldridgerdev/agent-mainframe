use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

use super::super::dashboard::centered_rect;
use crate::theme::Theme;

pub fn draw_help(frame: &mut Frame, theme: &Theme) {
    let area = centered_rect(55, 70, frame.area());
    draw_help_at(frame, area, theme);
}

pub fn draw_help_bottom_right(frame: &mut Frame, theme: &Theme) {
    let viewport = frame.area();
    let width = (viewport.width.saturating_mul(55) / 100).max(40);
    let height = (viewport.height.saturating_mul(70) / 100).max(12);
    let area = Rect::new(
        viewport.x + viewport.width.saturating_sub(width + 1),
        viewport.y + viewport.height.saturating_sub(height + 1),
        width,
        height,
    );
    draw_help_at(frame, area, theme);
}

fn draw_help_at(frame: &mut Frame, area: Rect, theme: &Theme) {
    crate::ui::draw_modal_overlay(frame, area, theme);

    let keybinds: Vec<(&str, &str)> = vec![
        ("j/k / \u{2191}/\u{2193}", "Navigate up/down"),
        ("h", "Collapse project / go to parent"),
        ("l", "Expand project / view feature"),
        ("Enter", "Toggle expand / view session"),
        ("s", "Add session (picker)"),
        ("S", "Pick session to resume"),
        ("N", "Create new project"),
        ("n", "Create new feature"),
        ("O", "Open AMF settings project"),
        ("d", "Delete project/feature/session"),
        ("D", "View debug log"),
        ("P", "Open syntax parser picker"),
        ("c", "Start feature (create tmux)"),
        ("x", "Stop feature / remove session"),
        ("r", "Rename session"),
        ("u", "Project preferred agent / worktree agent config"),
        ("F", "Fork feature (new branch)"),
        ("y", "Toggle mark feature as ready"),
        ("Z", "Generate session summary"),
        ("i", "Input requests picker"),
        ("/", "Search and jump to item"),
        ("R", "Refresh statuses"),
        ("?", "Toggle this help"),
        ("q / Esc", "Quit"),
    ];

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                "  ESC",
                Style::default()
                    .fg(theme.effective_bg())
                    .bg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                " closes this menu",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];
    for (key, desc) in &keybinds {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:>12}", key),
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(*desc, Style::default().fg(theme.text.to_color())),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  While viewing (embedded tmux):",
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "Ctrl+Q"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Exit view", Style::default().fg(theme.text.to_color())),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "Ctrl+Space"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Leader key (then: q s t T w h / a n p i b l v m r R x f d D ? H M 1-9)",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "s"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Steering coach", Style::default().fg(theme.text.to_color())),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "d"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Diff viewer", Style::default().fg(theme.text.to_color())),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "m"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Markdown file picker/viewer",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "b"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Show/hide sidebar",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "v"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Expand/collapse todos",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "t / T"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Cycle next/prev session",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "w"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Session switcher",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "h"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Bookmark picker popup",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "H / M"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Bookmark / unbookmark session",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "1-9"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Jump to bookmark slot",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "/"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Command picker (slash + AMF actions)",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "a"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "AMF local actions picker",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "R"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Refresh pane sizing",
            Style::default().fg(theme.text.to_color()),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "D"),
            Style::default()
                .fg(theme.warning.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Debug log", Style::default().fg(theme.text.to_color())),
    ]));

    let help = Paragraph::new(lines).block(
        Block::default()
            .title(" Keybindings ")
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.effective_bg()))
            .border_style(Style::default().fg(theme.primary.to_color())),
    );

    frame.render_widget(help, area);
}
