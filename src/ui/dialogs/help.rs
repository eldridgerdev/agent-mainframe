use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};

use super::super::dashboard::centered_rect;
use crate::theme::Theme;

pub fn draw_help(frame: &mut Frame, scroll_offset: usize, theme: &Theme) {
    let area = centered_rect(55, 70, frame.area());
    draw_help_at(frame, area, scroll_offset, theme);
}

fn draw_help_at(frame: &mut Frame, area: Rect, scroll_offset: usize, theme: &Theme) {
    crate::ui::draw_modal_overlay(frame, area, theme);

    let normal_keybinds: Vec<(&str, &str)> = vec![
        ("j/k / \u{2191}/\u{2193}", "Navigate up/down"),
        ("h / \u{2190}", "Collapse project/feature"),
        ("l / \u{2192}", "Expand project/feature"),
        ("Enter", "Toggle expand / view session"),
        ("s", "Add session (picker)"),
        ("S", "Pick session to resume"),
        ("N", "Create new project"),
        ("n", "Create new feature"),
        ("B", "Create batch features"),
        ("O", "Open AMF settings project"),
        ("A", "Manage agent harnesses"),
        ("d", "Delete project/feature/session"),
        ("D", "View debug log"),
        ("p", "Open syntax parser picker"),
        ("L", "Open prompt library"),
        ("G", "Review PR comments (experimental)"),
        ("T", "Theme picker"),
        ("c", "Start feature (create tmux)"),
        ("x", "Stop feature / remove session"),
        ("r", "Rename session/feature"),
        ("R", "Refresh statuses"),
        ("V", "Check pending diff review"),
        ("u", "Preferred harness / worktree config"),
        ("F", "Fork feature (new branch)"),
        ("f", "Cycle session filter"),
        ("y", "Toggle mark feature as ready"),
        ("Z", "Generate session summary"),
        ("i", "Input requests picker"),
        ("/", "Search and jump to item"),
        ("Ctrl+Space c", "Config wizard"),
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
                " closes  j/k  scroll",
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    for (key, desc) in &normal_keybinds {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:>14}", key),
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

    let view_keybinds: Vec<(&str, &str)> = vec![
        ("Ctrl+Q", "Exit view"),
        ("Ctrl+Space", "Open leader command menu"),
        ("any text key", "Open compose input (agent sessions)"),
        ("s", "Steering coach (experimental)"),
        ("e", "Toggle compose/direct input (agent sessions)"),
        ("d", "Diff viewer"),
        ("m", "Markdown file picker/viewer"),
        ("b", "Show/hide sidebar"),
        ("v", "Expand/collapse todos"),
        ("N", "Quick-capture a TODO for this project"),
        ("t / T", "Cycle next/prev session"),
        ("w", "Session switcher"),
        ("h", "Bookmark picker popup"),
        ("H / M", "Bookmark / unbookmark session"),
        ("1-9", "Jump to bookmark slot"),
        ("/", "Command picker (slash + AMF actions)"),
        ("a", "AMF local actions picker"),
        ("p", "Prompt library (inject saved prompt)"),
        ("R", "Refresh pane sizing"),
        ("D", "Debug log"),
        ("A", "Manage agent harnesses"),
    ];

    for (key, desc) in &view_keybinds {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:>14}", key),
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
        "  While reviewing PR comments:",
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )));

    let pr_review_keybinds: Vec<(&str, &str)> = vec![
        ("j/k", "Navigate comments"),
        ("Ctrl+D/U", "Scroll detail down/up"),
        ("h", "Hide/show resolved comments"),
        ("f", "Inject scoped fix into agent session"),
        ("", "(e edit · Tab inject · Ctrl+T vim)"),
        ("Space", "Mark comment for batch fix"),
        ("F", "Queue all marked fixes into the session"),
        ("t", "Toggle fix target session"),
        ("", "(first fix picks the review harness)"),
        ("R", "Reply 'Done in <sha>'"),
        ("n", "Reply 'not needed' (+ skip)"),
        ("x", "Resolve/reopen GitHub thread"),
        ("m", "Mark comment done"),
        ("s", "Skip comment (local)"),
        ("i", "Install syntax highlighting for file"),
        ("r", "Refresh comments from GitHub"),
        ("g", "Pick a different PR to review"),
        ("q / Esc", "Close review pane"),
    ];

    for (key, desc) in &pr_review_keybinds {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:>14}", key),
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
        "  In the PR picker:",
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )));

    let pr_picker_keybinds: Vec<(&str, &str)> = vec![
        ("j/k", "Navigate PRs"),
        ("Enter", "Open highlighted PR for review"),
        ("a", "Include/hide closed & merged PRs"),
        ("#", "Enter a PR number instead"),
        ("q / Esc", "Close picker"),
    ];

    for (key, desc) in &pr_picker_keybinds {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:>14}", key),
                Style::default()
                    .fg(theme.warning.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(*desc, Style::default().fg(theme.text.to_color())),
        ]));
    }

    let total_lines = lines.len();
    let visible_height = area.height.saturating_sub(2) as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = scroll_offset.min(max_scroll) as u16;

    let help = Paragraph::new(lines).scroll((scroll, 0)).block(
        Block::default()
            .title(" Keybindings ")
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.effective_bg()))
            .border_style(Style::default().fg(theme.primary.to_color())),
    );

    frame.render_widget(help, area);

    if total_lines > visible_height {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut scrollbar_state =
            ScrollbarState::new(max_scroll).position(scroll_offset.min(max_scroll));
        let scrollbar_area = Rect {
            x: area.x + area.width - 1,
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}
