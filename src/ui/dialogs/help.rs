//! The `?` keybinding reference.
//!
//! The dashboard's own keys live in [`DASHBOARD_KEYS`], grouped by area; the
//! two Ctrl+Space menus are read straight from the tables that draw them
//! ([`crate::ui::pane::LEADER_COMMANDS`] and
//! [`crate::ui::dashboard::DASHBOARD_LEADER_COMMANDS`]), so help cannot
//! describe a menu that no longer exists. The dashboard footer strip
//! (`ui::status`) only names keys that appear here, and a test parses the
//! dashboard key handler so a new binding cannot ship without a help row.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
};
use unicode_width::UnicodeWidthStr;

use super::super::dashboard::centered_rect;
use crate::theme::Theme;

/// Area headings for the dashboard keys, in display order.
pub(crate) const AREA_GETTING_AROUND: &str = "Getting around";
pub(crate) const AREA_PROJECTS: &str = "Projects & features";
pub(crate) const AREA_SESSIONS: &str = "Sessions";
pub(crate) const AREA_REVIEW: &str = "Review";
pub(crate) const AREA_PLAN: &str = "Plan";
pub(crate) const AREA_SETTINGS: &str = "Settings & other";

const AREAS: &[&str] = &[
    AREA_GETTING_AROUND,
    AREA_PROJECTS,
    AREA_SESSIONS,
    AREA_REVIEW,
    AREA_PLAN,
    AREA_SETTINGS,
];

/// Every dashboard key, as `(area, key, description)`. Each key appears once;
/// the area is where a newcomer would look for it.
pub(crate) const DASHBOARD_KEYS: &[(&str, &str, &str)] = &[
    (
        AREA_GETTING_AROUND,
        "j/k / \u{2191}/\u{2193}",
        "Move through projects, features and sessions",
    ),
    (
        AREA_GETTING_AROUND,
        "Ctrl+j / Ctrl+k",
        "Jump to the next / previous feature",
    ),
    (
        AREA_GETTING_AROUND,
        "h / \u{2190}",
        "Collapse project/feature",
    ),
    (
        AREA_GETTING_AROUND,
        "l / \u{2192}",
        "Expand project/feature",
    ),
    (
        AREA_GETTING_AROUND,
        "Enter",
        "Open a session; expand or collapse a project or feature",
    ),
    (AREA_GETTING_AROUND, "/", "Search and jump to an item"),
    (
        AREA_GETTING_AROUND,
        "i",
        "Needs attention: questions and finished work",
    ),
    (
        AREA_GETTING_AROUND,
        "?",
        "This help (press / in it to search)",
    ),
    (AREA_GETTING_AROUND, "Ctrl+Space", "Dashboard command menu"),
    (AREA_GETTING_AROUND, "q", "Quit"),
    (
        AREA_GETTING_AROUND,
        "Esc",
        "Ask to quit (y quits, n or Esc stays)",
    ),
    (AREA_PROJECTS, "N", "Create a new project"),
    (
        AREA_PROJECTS,
        "n",
        "Create a new feature in the selected project",
    ),
    (
        AREA_PROJECTS,
        "B",
        "Batch features into the selected project",
    ),
    (
        AREA_PROJECTS,
        "g",
        "Browse GitHub issues for the selected project",
    ),
    (
        AREA_PROJECTS,
        "r",
        "Rename feature/session (on a project: refresh)",
    ),
    (
        AREA_PROJECTS,
        "d",
        "Delete project/feature/session (asks first)",
    ),
    (AREA_PROJECTS, "F", "Fork feature (new branch)"),
    (AREA_PROJECTS, "y", "Toggle mark feature as ready"),
    (
        AREA_PROJECTS,
        "t",
        "Companion review feature: land its commits on the source branch",
    ),
    (AREA_PROJECTS, "z", "Dormant features (idle + unattended)"),
    (AREA_PROJECTS, "Z", "Generate a session summary"),
    (AREA_PROJECTS, "R", "Refresh statuses"),
    (AREA_SESSIONS, "s", "Add a session to the feature (picker)"),
    (AREA_SESSIONS, "c", "Start the feature or a stopped session"),
    (
        AREA_SESSIONS,
        "x",
        "Stop the feature or session (the session stays listed)",
    ),
    (AREA_SESSIONS, "S", "Resume a saved agent transcript"),
    (
        AREA_SESSIONS,
        "m",
        "Model / reasoning advice for the selected agent session",
    ),
    (AREA_SESSIONS, "I", "On a TODOs row: start the next TODO"),
    (AREA_REVIEW, "V", "Check pending diff review"),
    (
        AREA_REVIEW,
        "W",
        "AI Review for this feature (experimental)",
    ),
    (AREA_REVIEW, "G", "Split PR Workspace (experimental)"),
    (
        AREA_REVIEW,
        "K",
        "Learning Mode: read this codebase and ask questions",
    ),
    (AREA_PLAN, "P", "Run a plan interview for this feature"),
    (
        AREA_PLAN,
        "Q",
        "Run a Quick Plan interview for this feature",
    ),
    (
        AREA_SETTINGS,
        "u",
        "Preferred harness / worktree / session config",
    ),
    (AREA_SETTINGS, "A", "Manage agent harnesses"),
    (AREA_SETTINGS, "O", "Open the AMF settings project"),
    (AREA_SETTINGS, "T", "Theme picker"),
    (AREA_SETTINGS, "p", "Syntax parser picker"),
    (AREA_SETTINGS, "L", "Prompt library"),
    (AREA_SETTINGS, "E", "Edit headless AI prompt templates"),
    (AREA_SETTINGS, "w", "Context window / warning settings"),
    (AREA_SETTINGS, "D", "Debug log"),
];

/// What `Esc` means on each screen — the one key whose meaning changes most.
const ESC_MEANINGS: &[(&str, &str)] = &[
    (
        "Dashboard",
        "Asks \u{201c}Quit AMF?\u{201d} (y quits, n or Esc stays)",
    ),
    (
        "Session view",
        "Goes to the session (as ^[); Ctrl+Q leaves the view",
    ),
    (
        "Compose box",
        "Closes it and keeps the draft; type any letter to reopen it",
    ),
    ("Dialogs, menus", "Cancel and close"),
    ("This help", "Clears a search first, then closes"),
];

const SESSION_VIEW_KEYS: &[(&str, &str)] = &[
    ("Ctrl+Q", "Back to the dashboard"),
    ("Ctrl+Space", "Command menu (keys listed below)"),
    (
        "any letter",
        "Opens the compose box (agent sessions); the header says so",
    ),
    (
        "Esc",
        "Goes to the session as ^[ when compose is closed — it does not leave the view",
    ),
    (
        "Ctrl+Space e",
        "Switch between compose and direct input (direct: every key goes to the agent)",
    ),
];

const COMPOSE_KEYS: &[(&str, &str)] = &[
    ("Enter", "Send to the agent"),
    ("Shift+Enter", "Insert a newline"),
    ("Tab", "Complete a / command"),
    ("Ctrl+V", "Paste from the clipboard"),
    ("Ctrl+T", "Toggle Vim keys"),
    ("Ctrl+E", "Switch to direct input (draft kept)"),
    (
        "Esc / Ctrl+Q",
        "Close (draft kept); type a letter to reopen",
    ),
];

const TODOS_KEYS: &[(&str, &str)] = &[
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
    (
        "Enter",
        "Start work: agent now, or plan it first (jumps straight to a linked session or feature; an in-progress TODO cannot launch another agent)",
    ),
    (
        "p",
        "Show/hide the project TODO list (visibility is shared until AMF exits)",
    ),
    ("g", "Show/hide the global TODO list"),
    ("Tab / Shift+Tab", "Move between visible TODO lists"),
    ("M / C", "Move / copy the TODO to another list"),
    ("q / Esc / Ctrl+Q", "Exit TODOs view"),
];

const PLAN_SCREEN_KEYS: &[(&str, &str)] = &[
    (
        "",
        "The plan interview, plan review and Expert review list their keys in their own footers.",
    ),
    (
        "Enter",
        "Save an answer and continue; in plan review, accept",
    ),
    (
        "Ctrl+F",
        "Draft the plan now from saved answers (uses tokens)",
    ),
    (
        "Esc",
        "Cancel: launch without a plan, or leave the plan unchanged",
    ),
];

const PR_TRIAGE_KEYS: &[(&str, &str)] = &[
    ("j/k", "Navigate comments"),
    ("Ctrl+D/U", "Scroll detail down/up"),
    ("h", "Hide/show resolved comments"),
    (
        "o",
        "Cycle sort: fetch, file, author, humans first, conversations last",
    ),
    (
        "f",
        "Inject a scoped fix into an agent session (e edit · Tab inject · Ctrl+T vim). A finished investigation's findings are appended. The first fix picks the target: a live session, a named dedicated session + harness, or New feature… (an isolated worktree)",
    ),
    (
        "v",
        "Investigate this comment — read-only headless pass; the answer persists in the detail panel",
    ),
    (
        "e",
        "Attach optional context for the next Investigate — a hypothesis the run verifies, not a fact to assume",
    ),
    (
        "a",
        "Act on a finished investigation: post reply, ask follow-up, dismiss, keep as TODO",
    ),
    ("Space", "Mark comment for batch fix"),
    ("B", "Inject one combined prompt for all marked"),
    (
        "P",
        "Jump to the linked fix session (Ctrl+Space P there jumps back)",
    ),
    ("R", "Reply — pick Done / not-needed, then post"),
    (
        "M",
        "Add finding to review-memory doc (e edit · Tab category · g project/global)",
    ),
    (
        "m",
        "Mark — Done (local) / Skip (local) / Resolve on GitHub",
    ),
    ("i", "Install syntax highlighting for file"),
    ("r", "Refresh comments from GitHub"),
    ("g", "Pick a different PR to triage"),
    ("A", "Open the AI Review pane for this PR"),
    (
        "I",
        "Land the triage feature's commits on the PR (push to the PR branch, or cherry-pick into the source worktree — never while it's dirty)",
    ),
    ("q / Esc", "Close PR Triage"),
];

const AI_REVIEW_KEYS: &[(&str, &str)] = &[
    ("j/k", "Navigate findings"),
    ("Ctrl+D/U", "Scroll detail down/up"),
    ("s", "Skip/unskip finding (excludes it from W)"),
    ("e", "Edit finding body"),
    (
        "A",
        "Generate/regenerate the AI review (first run picks harness + model)",
    ),
    (
        "W",
        "Post kept findings to GitHub as a review (e edit summary)",
    ),
    (
        "q / Esc",
        "Close AI Review (back to PR Triage if opened from there)",
    ),
];

const PR_PICKER_KEYS: &[(&str, &str)] = &[
    ("j/k", "Navigate PRs"),
    ("Enter", "Open highlighted PR in PR Triage"),
    ("W", "Open highlighted PR in AI Review"),
    ("a", "Include/hide closed & merged PRs"),
    ("#", "Enter a PR number instead"),
    (
        "b",
        "Bootstrap review memory from recent PRs (g picks the project or global doc)",
    ),
    (
        "c",
        "Compact review memory: merge dupes, prune stale (g picks the doc)",
    ),
    ("Tab", "Switch to the Review tab (review a PR yourself)"),
    ("q / Esc", "Close picker"),
];

const REVIEW_TAB_KEYS: &[(&str, &str)] = &[
    ("j/k", "Navigate open PRs (yours and drafts included)"),
    (
        "Enter",
        "Review the highlighted PR in the diff viewer (no agent, no feature; your checkout is untouched)",
    ),
    ("r", "Reload the list"),
    ("Tab", "Back to the Triage tab"),
    (
        "q / Esc",
        "Close (or stop a PR that is still opening). In the review, q submits to GitHub and Esc pauses with the draft saved; ? there lists every key",
    ),
];

/// A run of keys under an optional sub-heading.
pub(crate) struct HelpBlock {
    pub heading: &'static str,
    pub entries: Vec<(&'static str, &'static str)>,
}

/// One area of the help screen.
pub(crate) struct HelpSection {
    pub title: &'static str,
    pub blocks: Vec<HelpBlock>,
}

fn block(heading: &'static str, entries: &[(&'static str, &'static str)]) -> HelpBlock {
    HelpBlock {
        heading,
        entries: entries.to_vec(),
    }
}

fn dashboard_block(area: &str) -> HelpBlock {
    HelpBlock {
        heading: "",
        entries: DASHBOARD_KEYS
            .iter()
            .filter(|(a, _, _)| *a == area)
            .map(|(_, k, d)| (*k, *d))
            .collect(),
    }
}

/// The session-view Ctrl+Space menu, one block per menu group.
fn session_leader_blocks() -> Vec<HelpBlock> {
    let mut blocks: Vec<HelpBlock> = Vec::new();
    for (group, key, desc) in crate::ui::pane::LEADER_COMMANDS {
        let heading = leader_group_heading(group);
        match blocks.last_mut() {
            Some(last) if last.heading == heading => last.entries.push((*key, *desc)),
            _ => blocks.push(HelpBlock {
                heading,
                entries: vec![(*key, *desc)],
            }),
        }
    }
    blocks
}

/// Static sub-heading text for a leader-menu group (headings are `'static`
/// so filtering and rendering never allocate per row).
fn leader_group_heading(group: &str) -> &'static str {
    match group {
        "Sessions" => "In a session: Ctrl+Space, then \u{2014} Sessions",
        "Bookmarks" => "In a session: Ctrl+Space, then \u{2014} Bookmarks",
        "Agent" => "In a session: Ctrl+Space, then \u{2014} Agent",
        "Review & plan" => "In a session: Ctrl+Space, then \u{2014} Review & plan",
        "Remote & help" => "In a session: Ctrl+Space, then \u{2014} Remote & help",
        _ => "In a session: Ctrl+Space, then \u{2014} More",
    }
}

/// Every section of the help screen, in display order.
pub(crate) fn help_sections() -> Vec<HelpSection> {
    let mut sections = Vec::with_capacity(AREAS.len());
    for area in AREAS {
        let mut blocks = vec![dashboard_block(area)];
        match *area {
            AREA_GETTING_AROUND => {
                blocks.push(block("What Esc does on each screen", ESC_MEANINGS));
            }
            AREA_SESSIONS => {
                blocks.push(block("In a session view", SESSION_VIEW_KEYS));
                blocks.extend(session_leader_blocks());
                blocks.push(block("In the compose box", COMPOSE_KEYS));
                blocks.push(block("In the TODOs view", TODOS_KEYS));
            }
            AREA_REVIEW => {
                blocks.push(block("In PR Triage", PR_TRIAGE_KEYS));
                blocks.push(block("In AI Review", AI_REVIEW_KEYS));
                blocks.push(block("In the PR picker", PR_PICKER_KEYS));
                blocks.push(block("In the PR picker's Review tab", REVIEW_TAB_KEYS));
            }
            AREA_PLAN => {
                blocks.push(block("On the plan screens", PLAN_SCREEN_KEYS));
            }
            AREA_SETTINGS => {
                blocks.push(HelpBlock {
                    heading: "On the dashboard: Ctrl+Space, then",
                    entries: crate::ui::dashboard::DASHBOARD_LEADER_COMMANDS.to_vec(),
                });
            }
            _ => {}
        }
        sections.push(HelpSection {
            title: area,
            blocks,
        });
    }
    sections
}

fn matches(haystack: &str, needle_lower: &str) -> bool {
    haystack.to_lowercase().contains(needle_lower)
}

/// The sections a search leaves, with non-matching rows removed. A query
/// naming a section or sub-heading keeps everything under it, so "review"
/// shows the whole Review area rather than only rows that say "review".
pub(crate) fn filter_sections(query: &str) -> Vec<HelpSection> {
    let needle = query.trim().to_lowercase();
    let sections = help_sections();
    if needle.is_empty() {
        return sections;
    }
    sections
        .into_iter()
        .filter_map(|section| {
            if matches(section.title, &needle) {
                return Some(section);
            }
            let blocks: Vec<HelpBlock> = section
                .blocks
                .into_iter()
                .filter_map(|b| {
                    if !b.heading.is_empty() && matches(b.heading, &needle) {
                        return Some(b);
                    }
                    let entries: Vec<_> = b
                        .entries
                        .into_iter()
                        .filter(|(k, d)| {
                            !k.is_empty() && (matches(k, &needle) || matches(d, &needle))
                        })
                        .collect();
                    (!entries.is_empty()).then_some(HelpBlock {
                        heading: b.heading,
                        entries,
                    })
                })
                .collect();
            (!blocks.is_empty()).then_some(HelpSection {
                title: section.title,
                blocks,
            })
        })
        .collect()
}

/// Number of key rows a query leaves (notes excluded).
pub(crate) fn match_count(sections: &[HelpSection]) -> usize {
    sections
        .iter()
        .flat_map(|s| &s.blocks)
        .flat_map(|b| &b.entries)
        .filter(|(k, _)| !k.is_empty())
        .count()
}

const KEY_COL: usize = 16;

/// Greedy word wrap by display width; a word longer than the line is split.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let w = word.width();
        let cur = current.width();
        if cur > 0 && cur + 1 + w > width {
            lines.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
        while current.width() > width {
            let mut cut = String::new();
            let mut rest = String::new();
            for ch in current.chars() {
                if cut.width() < width && rest.is_empty() {
                    cut.push(ch);
                } else {
                    rest.push(ch);
                }
            }
            lines.push(cut);
            current = rest;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn body_lines(sections: &[HelpSection], width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let key_style = Style::default()
        .fg(theme.warning.to_color())
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(theme.text.to_color());
    let muted = Style::default().fg(theme.text_muted.to_color());
    let title_style = Style::default()
        .fg(theme.primary.to_color())
        .add_modifier(Modifier::BOLD);
    let heading_style = Style::default()
        .fg(theme.secondary.to_color())
        .add_modifier(Modifier::BOLD);

    // Narrow screens give the description the room and let the key sit on
    // its own line, so nothing is cut off at 80 columns or below.
    let key_col = if width >= 50 { KEY_COL } else { 0 };
    let indent = 2 + key_col + 2;
    let desc_width = width.saturating_sub(indent).max(10);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (si, section) in sections.iter().enumerate() {
        if si > 0 {
            lines.push(Line::from(""));
        }
        let rule = "\u{2500}".repeat(width.saturating_sub(section.title.width() + 5).min(40));
        lines.push(Line::from(vec![
            Span::styled(format!(" {} ", section.title), title_style),
            Span::styled(rule, muted),
        ]));
        for b in &section.blocks {
            if !b.heading.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", b.heading),
                    heading_style,
                )));
            }
            for (key, desc) in &b.entries {
                if key.is_empty() {
                    for chunk in wrap_words(desc, width.saturating_sub(4)) {
                        lines.push(Line::from(Span::styled(format!("    {chunk}"), muted)));
                    }
                    continue;
                }
                let wrapped = wrap_words(desc, desc_width);
                if key_col == 0 {
                    lines.push(Line::from(Span::styled(format!("  {key}"), key_style)));
                    for chunk in wrapped {
                        lines.push(Line::from(Span::styled(format!("    {chunk}"), text_style)));
                    }
                    continue;
                }
                let key_cell = if key.width() > key_col {
                    // An over-long key gets a line of its own.
                    lines.push(Line::from(Span::styled(format!("  {key}"), key_style)));
                    " ".repeat(key_col)
                } else {
                    format!("{:>w$}", key, w = key_col)
                };
                for (i, chunk) in wrapped.into_iter().enumerate() {
                    let lead = if i == 0 {
                        Span::styled(format!("  {key_cell}  "), key_style)
                    } else {
                        Span::raw(" ".repeat(indent))
                    };
                    lines.push(Line::from(vec![lead, Span::styled(chunk, text_style)]));
                }
            }
        }
    }
    lines
}

/// Draw the help overlay. `query` filters the rows; `searching` shows the
/// search box as live input. Returns the clamped scroll offset.
pub fn draw_help(
    frame: &mut Frame,
    scroll_offset: usize,
    query: &str,
    searching: bool,
    theme: &Theme,
) -> usize {
    let area = centered_rect(90, 85, frame.area());
    draw_help_at(frame, area, scroll_offset, query, searching, theme)
}

fn draw_help_at(
    frame: &mut Frame,
    area: Rect,
    scroll_offset: usize,
    query: &str,
    searching: bool,
    theme: &Theme,
) -> usize {
    crate::ui::draw_modal_overlay(frame, area, theme);

    let block = Block::default()
        .title(" Keybindings ")
        .borders(Borders::ALL)
        .style(Style::default().bg(theme.effective_bg()))
        .border_style(Style::default().fg(theme.primary.to_color()));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return 0;
    }

    let key_style = Style::default()
        .fg(theme.warning.to_color())
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(theme.text_muted.to_color());

    // Row 0: search box / how to drive this screen. It stays put while the
    // body scrolls, so the query is never scrolled out of sight.
    let sections = filter_sections(query);
    let top = if searching || !query.is_empty() {
        let mut spans = vec![
            Span::styled(" / ", key_style),
            Span::styled(
                query.to_string(),
                Style::default().fg(theme.text.to_color()),
            ),
        ];
        if searching {
            spans.push(Span::styled("\u{258f}", key_style));
        }
        spans.push(Span::styled(
            format!(
                "  {} match{}  ",
                match_count(&sections),
                if match_count(&sections) == 1 {
                    ""
                } else {
                    "es"
                }
            ),
            muted,
        ));
        if searching {
            spans.extend([
                Span::styled("Enter", key_style),
                Span::styled(" keep  ", muted),
                Span::styled("Esc", key_style),
                Span::styled(" clear", muted),
            ]);
        } else {
            spans.extend([
                Span::styled("Esc", key_style),
                Span::styled(" clear  ", muted),
                Span::styled("/", key_style),
                Span::styled(" edit", muted),
            ]);
        }
        Line::from(spans)
    } else {
        Line::from(vec![
            Span::styled(" Esc", key_style),
            Span::styled(" close  ", muted),
            Span::styled("/", key_style),
            Span::styled(" search  ", muted),
            Span::styled("j/k", key_style),
            Span::styled(" scroll  ", muted),
            Span::styled("g/G", key_style),
            Span::styled(" top/end", muted),
        ])
    };
    frame.render_widget(
        Paragraph::new(top),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );

    let body_area = Rect::new(
        inner.x,
        inner.y.saturating_add(2.min(inner.height)),
        inner.width,
        inner.height.saturating_sub(2),
    );
    if body_area.height == 0 {
        return 0;
    }
    // One column is the scrollbar's, one more keeps text off it.
    let width = (body_area.width as usize).saturating_sub(2);
    let lines = if sections.is_empty() {
        vec![Line::from(Span::styled(
            format!("  No keys match \u{201c}{}\u{201d}.", query.trim()),
            muted,
        ))]
    } else {
        body_lines(&sections, width, theme)
    };

    let total_lines = lines.len();
    let visible_height = body_area.height as usize;
    let max_scroll = total_lines.saturating_sub(visible_height);
    let scroll = scroll_offset.min(max_scroll);

    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), body_area);

    if total_lines > visible_height && body_area.width > 0 {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight);
        let mut scrollbar_state = ScrollbarState::new(max_scroll).position(scroll);
        let scrollbar_area = Rect {
            x: body_area.x + body_area.width - 1,
            y: body_area.y,
            width: 1,
            height: body_area.height,
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
    scroll
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn render(width: u16, height: u16, scroll: usize, query: &str) -> (String, usize) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        let mut clamped = 0;
        terminal
            .draw(|frame| {
                clamped = draw_help(frame, scroll, query, !query.is_empty(), &Theme::default());
            })
            .unwrap();
        let buffer = terminal.backend().buffer();
        let text = buffer
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        (text, clamped)
    }

    /// Every row the help can show, read by scrolling through it.
    fn all_pages(width: u16, height: u16, query: &str) -> String {
        let (_, max) = render(width, height, usize::MAX, query);
        let mut seen = String::new();
        for scroll in 0..=max {
            seen.push_str(&render(width, height, scroll, query).0);
            seen.push('\n');
        }
        seen
    }

    #[test]
    fn help_is_grouped_under_area_headings_in_order() {
        for (w, h) in [(80u16, 24u16), (120, 40)] {
            let seen = all_pages(w, h, "");
            let mut last = 0;
            for area in AREAS {
                let at = seen
                    .find(&format!(" {area} \u{2500}"))
                    .unwrap_or_else(|| panic!("{w}x{h}: heading {area} missing"));
                assert!(at >= last, "{w}x{h}: {area} out of order");
                last = at;
            }
            assert!(seen.contains("What Esc does on each screen"));
            assert!(seen.contains("Goes to the session (as ^[)"));
            assert!(seen.contains("Close picker"));
        }
    }

    #[test]
    fn plan_interview_key_block_is_collapsed() {
        let seen = all_pages(120, 40, "");
        assert!(!seen.contains("During a plan interview"));
        assert!(!seen.contains("Restore the previous interview"));
        assert!(seen.contains("list their keys in their own footers"));
    }

    #[test]
    fn help_rows_are_never_cut_off_at_80_columns() {
        // Every description word is reachable by scrolling — nothing is
        // clipped at the right edge of a narrow terminal.
        let seen = all_pages(80, 24, "");
        for section in help_sections() {
            for b in section.blocks {
                for (_, desc) in b.entries {
                    for word in desc.split_whitespace() {
                        assert!(seen.contains(word), "`{word}` of `{desc}` never shown");
                    }
                }
            }
        }
    }

    #[test]
    fn search_keeps_matching_rows_and_their_headings() {
        let sections = filter_sections("debug");
        let text: Vec<_> = sections
            .iter()
            .flat_map(|s| &s.blocks)
            .flat_map(|b| &b.entries)
            .map(|(_, d)| *d)
            .collect();
        assert!(text.contains(&"Debug log"));
        assert!(text.iter().all(|d| d.to_lowercase().contains("debug")));

        // A section title keeps the whole section.
        let review = filter_sections("review");
        assert!(review.iter().any(
            |s| s.title == AREA_REVIEW && s.blocks.iter().any(|b| b.heading == "In PR Triage")
        ));

        // Case-insensitive, and a miss says so.
        assert!(match_count(&filter_sections("THEME")) >= 1);
        assert_eq!(match_count(&filter_sections("zzzz-nothing")), 0);
        let (screen, _) = render(80, 24, 0, "zzzz-nothing");
        assert!(screen.contains("No keys match"));
        let (screen, _) = render(80, 24, 0, "theme");
        assert!(screen.contains("Theme picker"));
        assert!(screen.contains("match"));
        assert!(!screen.contains("Close picker"));
    }

    #[test]
    fn session_view_compose_toggle_row_tracks_the_leader_key() {
        let row = SESSION_VIEW_KEYS
            .iter()
            .find(|(_, d)| d.starts_with("Switch between compose"))
            .expect("compose toggle row");
        assert_eq!(
            row.0,
            format!("Ctrl+Space {}", crate::ui::pane::COMPOSE_TOGGLE_KEY)
        );
    }

    #[test]
    fn help_reads_both_ctrl_space_menus_from_their_tables() {
        let sections = help_sections();
        let entries: Vec<(&str, &str)> = sections
            .iter()
            .flat_map(|s| &s.blocks)
            .flat_map(|b| b.entries.iter().copied())
            .collect();
        for (_, key, desc) in crate::ui::pane::LEADER_COMMANDS {
            assert!(
                entries.contains(&(*key, *desc)),
                "session leader {key} missing"
            );
        }
        for entry in crate::ui::dashboard::DASHBOARD_LEADER_COMMANDS {
            assert!(
                entries.contains(entry),
                "dashboard leader {} missing",
                entry.0
            );
        }
    }

    /// Split a help key cell like `"j/k / ↑/↓"` or `"Space / x"` into the
    /// single characters it binds.
    fn bound_chars(key: &str) -> Vec<char> {
        let mut out = Vec::new();
        if key == "/" {
            return vec!['/'];
        }
        if let Some((a, b)) = key.split_once('-')
            && a.chars().count() == 1
            && b.chars().count() == 1
        {
            let (a, b) = (a.chars().next().unwrap(), b.chars().next().unwrap());
            return (a..=b).collect();
        }
        for part in key.split(" / ") {
            if part.contains('+') || part.chars().count() > 3 {
                continue;
            }
            for piece in part.split('/') {
                let mut chars = piece.chars();
                if let (Some(c), None) = (chars.next(), chars.next()) {
                    out.push(c);
                }
            }
        }
        out
    }

    /// The `KeyCode::Char('x')` literals bound in one function's body.
    fn handler_chars(source: &str, start: &str, end: &str) -> Vec<char> {
        let from = source.find(start).expect("handler start");
        let body = &source[from..];
        let body = &body[..body.find(end).expect("handler end")];
        let mut out = Vec::new();
        let mut rest = body;
        while let Some(at) = rest.find("KeyCode::Char('") {
            rest = &rest[at + "KeyCode::Char('".len()..];
            let mut chars = rest.chars();
            if let (Some(c), Some('\'')) = (chars.next(), chars.next()) {
                out.push(c);
            }
        }
        // `'1'..='9'` style ranges.
        let mut rest = body;
        while let Some(at) = rest.find("'..='") {
            let lo = rest[..at].chars().last();
            let hi = rest[at + 5..].chars().next();
            if let (Some(lo), Some(hi)) = (lo, hi) {
                out.extend(lo..=hi);
            }
            rest = &rest[at + 5..];
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    #[test]
    fn every_dashboard_key_has_a_help_row() {
        let source = include_str!("../../handlers/normal.rs");
        let documented: Vec<char> = DASHBOARD_KEYS
            .iter()
            .flat_map(|(_, k, _)| bound_chars(k))
            .collect();
        let bound = handler_chars(
            source,
            "pub fn handle_normal_key",
            "pub(crate) const DASHBOARD_KEYBINDING_ACTIONS",
        );
        assert!(bound.len() > 30, "parser found too few keys: {bound:?}");
        // `f` is the session filter, compiled out while
        // `DASHBOARD_SESSION_FILTER_ENABLED` is false; `j`/`k` with Ctrl
        // and the space of Ctrl+Space are documented as chords.
        let exempt = [' ', 'f'];
        for c in bound {
            if exempt.contains(&c) {
                continue;
            }
            assert!(
                documented.contains(&c),
                "dashboard key {c:?} is bound in handle_normal_key but has no row in help::DASHBOARD_KEYS"
            );
        }

        let leader_documented: Vec<char> = crate::ui::dashboard::DASHBOARD_LEADER_COMMANDS
            .iter()
            .flat_map(|(k, _)| bound_chars(k))
            .collect();
        let leader_bound = handler_chars(source, "fn handle_normal_leader_key", "#[cfg(test)]");
        assert!(leader_bound.len() > 10, "parser found too few leader keys");
        for c in leader_bound {
            assert!(
                leader_documented.contains(&c),
                "dashboard leader key {c:?} has no row in DASHBOARD_LEADER_COMMANDS"
            );
        }
    }

    #[test]
    fn dashboard_keys_are_listed_once() {
        let mut seen: Vec<&str> = Vec::new();
        for (_, key, _) in DASHBOARD_KEYS {
            assert!(!seen.contains(key), "{key} listed twice");
            seen.push(key);
        }
    }

    #[test]
    fn wrap_words_respects_width() {
        let lines = wrap_words("alpha beta gamma delta", 11);
        assert_eq!(lines, vec!["alpha beta", "gamma delta"]);
        assert!(wrap_words("abcdefghijkl", 5).iter().all(|l| l.width() <= 5));
        assert_eq!(wrap_words("", 5), vec![String::new()]);
    }
}
