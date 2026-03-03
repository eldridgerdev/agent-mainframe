use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::super::dashboard::centered_rect;

pub fn draw_help(frame: &mut Frame) {
    let area = centered_rect(55, 70, frame.area());
    frame.render_widget(Clear, area);

    let keybinds: Vec<(&str, &str)> = vec![
        ("j/k / \u{2191}/\u{2193}", "Navigate up/down"),
        ("h", "Collapse project / go to parent"),
        ("l", "Expand project / view feature"),
        ("Enter", "Toggle expand / view session"),
        ("s", "Switch to tmux session"),
        ("S", "Pick opencode session to resume"),
        ("N", "Create new project"),
        ("n", "Create new feature"),
        ("O", "Open AMF settings project"),
        ("d", "Delete project/feature/session"),
        ("c", "Start feature (create tmux)"),
        ("x", "Stop feature / remove session"),
        ("r", "Rename session"),
        ("t", "Add terminal session"),
        ("a", "Add Claude session"),
        ("v", "Add nvim session"),
        ("F", "Fork feature (new branch)"),
        ("m", "Create memo (.claude/notes.md)"),
        ("i", "Input requests picker"),
        ("/", "Search and jump to item"),
        ("R", "Refresh statuses"),
        ("?", "Toggle this help"),
        ("q / Esc", "Quit"),
    ];

    let mut lines: Vec<Line> = vec![Line::from("")];
    for (key, desc) in &keybinds {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:>12}", key),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(*desc, Style::default().fg(Color::White)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  While viewing (embedded tmux):",
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "Ctrl+Q"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Exit view", Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "Ctrl+Space"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(
            "Leader key (then: q t T w / s n p i r x f ?)",
            Style::default().fg(Color::White),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "t / T"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Cycle next/prev session", Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "w"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Session switcher", Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {:>12}", "/"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("Custom commands picker", Style::default().fg(Color::White)),
    ]));

    let help = Paragraph::new(lines).block(
        Block::default()
            .title(" Keybindings ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );

    frame.render_widget(help, area);
}
