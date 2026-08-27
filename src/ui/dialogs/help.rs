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
        ("Enter", "Toggle expand / view or recover session"),
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
        ("G", "Open PR Triage (experimental)"),
        ("W", "Open AI Review for this feature (experimental)"),
        (
            "K",
            "Open Learning Mode (read this codebase, ask questions)",
        ),
        ("P", "Run a plan interview for this feature"),
        ("T", "Theme picker"),
        ("c", "Start feature (create tmux)"),
        ("x", "Stop feature / remove session"),
        ("r", "Rename session/feature"),
        ("R", "Refresh statuses"),
        ("V", "Check pending diff review"),
        ("u", "Preferred harness / worktree config"),
        ("F", "Fork feature (new branch)"),
        ("y", "Toggle mark feature as ready"),
        ("Z", "Generate session summary"),
        ("z", "Dormant features (idle + unattended)"),
        ("w", "Context window / warning settings"),
        ("i", "Needs attention: questions and finished work"),
        ("I", "On a TODOs row: start the next TODO"),
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
        "  During a plan interview:",
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )));

    let plan_interview_keybinds: Vec<(&str, &str)> = vec![
        ("r / d", "Resume or discard a saved draft (on entry)"),
        (
            "Enter",
            "Save answer and continue; at the AI prompt, review raw plan (no tokens)",
        ),
        ("Alt+Enter", "Insert a newline (free-text answers)"),
        ("j/k / \u{2191}/\u{2193}", "Choose a select-option answer"),
        (
            "e",
            "Type your own answer to a select question (submitted with any picked option; Enter still submits)",
        ),
        ("Ctrl+B", "Return to the previous question"),
        ("Ctrl+S", "Skip an optional question"),
        ("Ctrl+R", "Restore the previous interview's answer (re-run)"),
        (
            "a",
            "Ask AI follow-ups at the optional AI prompt (uses tokens)",
        ),
        ("Ctrl+F", "Draft plan now from saved answers (uses tokens)"),
        (
            "Esc",
            "Cancel (launch without plan, or leave plan unchanged)",
        ),
        (
            "y / n",
            "Open a still-running session with the kickoff prompt seeded but unsent, or leave it alone",
        ),
    ];

    for (key, desc) in &plan_interview_keybinds {
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
        "  During plan review:",
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )));

    let plan_review_keybinds: Vec<(&str, &str)> = vec![
        ("j/k / PgUp/PgDn", "Scroll the rendered plan"),
        ("e", "Edit raw plan markdown"),
        (
            "a",
            "Agent review of the plan, or re-open one already held (uses tokens for a new review)",
        ),
        (
            "f",
            "Give free-form feedback; the agent may inspect the repository read-only (uses tokens)",
        ),
        (
            "i",
            "Research focused questions in isolated read-only contexts, then merge findings (uses tokens)",
        ),
        ("r", "Regenerate the plan (uses tokens)"),
        (
            "Enter",
            "Accept plan (on creation: launch feature and seed the kickoff prompt)",
        ),
        ("Ctrl+S", "Save edit and return to preview"),
        ("Ctrl+S (feedback)", "Send feedback and revise the plan"),
        (
            "Ctrl+S (research)",
            "Run isolated investigators and merge their findings",
        ),
        ("Esc", "Discard edit or confirm abort"),
    ];

    for (key, desc) in &plan_review_keybinds {
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
        "  During an agent review of the plan:",
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )));

    let plan_critique_keybinds: Vec<(&str, &str)> = vec![
        ("j/k / PgUp/PgDn", "Scroll the review"),
        ("r", "Revise the plan with this feedback (uses tokens)"),
        ("Esc / Enter", "Back to the plan, unchanged"),
    ];

    for (key, desc) in &plan_critique_keybinds {
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
        ("d", "Diff viewer (all changes / commit)"),
        ("m", "Markdown file picker/viewer"),
        ("n", "Open current plan"),
        (
            "F",
            "Start a fresh-context session (asks for your prompt first)",
        ),
        ("b", "Show/hide sidebar"),
        ("v", "Expand/collapse todos"),
        ("Ctrl+Space z", "Complete this session's referenced TODO"),
        ("Ctrl+Space Z", "Clear this session's TODO reference"),
        ("N", "Quick-capture a TODO for this worktree"),
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
        ("P", "Back to PR Triage (if one is stashed)"),
        ("G", "Open PR Triage for this feature"),
        ("W", "Open AI Review for this feature"),
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
        "  In the TODOs view:",
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )));

    let todos_keybinds: Vec<(&str, &str)> = vec![
        ("j/k / \u{2191}/\u{2193}", "Navigate TODOs"),
        ("a / n", "Add a TODO"),
        ("e", "Edit title"),
        ("o", "Edit notes"),
        ("b", "Edit scratchpad banner"),
        ("Space / x", "Cycle state: todo → in progress → done"),
        ("P", "Cycle priority (High/Med/Low)"),
        ("J / K", "Reorder up/down"),
        ("d", "Delete TODO (y/n confirm)"),
        ("I", "Start the next TODO in the visible lists"),
        ("Enter", "Start work: agent now, or plan it first"),
        ("", "  (jumps straight to a linked session or feature)"),
        ("", "  (an in-progress TODO cannot launch another agent)"),
        ("p", "Show/hide the project TODO list"),
        ("g", "Show/hide the global TODO list"),
        ("", "  (visibility is shared until AMF exits)"),
        ("Tab / Shift+Tab", "Move between visible TODO lists"),
        ("M / C", "Move / copy the TODO to another list"),
        ("q / Esc / Ctrl+Q", "Exit TODOs view"),
    ];

    for (key, desc) in &todos_keybinds {
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
        "  In PR Triage:",
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )));

    let pr_review_keybinds: Vec<(&str, &str)> = vec![
        ("j/k", "Navigate comments"),
        ("Ctrl+D/U", "Scroll detail down/up"),
        ("h", "Hide/show resolved comments"),
        (
            "o",
            "Cycle sort (fetch/file/author/humans-first/conversations-last)",
        ),
        ("f", "Inject scoped fix into agent session"),
        ("", "(e edit · Tab inject · Ctrl+T vim)"),
        ("", "(first fix/batch picks the fix target: existing"),
        ("", "live session, a named dedicated session + harness, or"),
        ("", "New feature… — an isolated worktree with its own"),
        ("", "harness + vibe mode, set up in one compact form)"),
        ("Space", "Mark comment for batch fix"),
        ("B", "Inject one combined prompt for all marked"),
        ("P", "Jump to the linked fix session"),
        ("", "(Ctrl+Space P there jumps back)"),
        ("R", "Reply — pick Done / not-needed, then post"),
        ("M", "Add finding to review-memory doc"),
        ("", "(e edit · Tab category · g project/global)"),
        (
            "m",
            "Mark — Done (local) / Skip (local) / Resolve on GitHub",
        ),
        ("i", "Install syntax highlighting for file"),
        ("r", "Refresh comments from GitHub"),
        ("g", "Pick a different PR to triage"),
        ("A", "Open the AI Review pane for this PR"),
        ("I", "Land the triage feature's commits on the PR"),
        ("", "(push to the PR branch, or cherry-pick into the"),
        ("", "source worktree — never while it's dirty)"),
        ("q / Esc", "Close PR Triage"),
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
        "  In AI Review:",
        Style::default()
            .fg(theme.primary.to_color())
            .add_modifier(Modifier::BOLD),
    )));

    let ai_review_keybinds: Vec<(&str, &str)> = vec![
        ("j/k", "Navigate findings"),
        ("Ctrl+D/U", "Scroll detail down/up"),
        ("s", "Skip/unskip finding (excludes it from W)"),
        ("e", "Edit finding body"),
        ("A", "Generate/regenerate the AI review"),
        ("", "(first run picks harness + model)"),
        ("W", "Post kept findings to GitHub as a review"),
        ("", "(e edit summary)"),
        (
            "q / Esc",
            "Close AI Review (back to PR Triage if opened from there)",
        ),
    ];

    for (key, desc) in &ai_review_keybinds {
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
        ("Enter", "Open highlighted PR in PR Triage"),
        ("W", "Open highlighted PR in AI Review"),
        ("a", "Include/hide closed & merged PRs"),
        ("#", "Enter a PR number instead"),
        ("b", "Bootstrap review memory from recent PRs"),
        ("", "(g picks the project or global doc)"),
        ("c", "Compact review memory (merge dupes, prune stale)"),
        ("", "(g picks the project or global doc)"),
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
