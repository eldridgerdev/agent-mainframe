//! Rendering for the Design Review room: an agenda, the selected item's
//! content and its thread, or the Usage tab. Drawing reads the room only;
//! every read from `amf.db` happened when the room loaded or changed.

use std::path::Path;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use super::super::dashboard::centered_rect;
use crate::app::design_review::room::{DesignReviewRoom, InputKind, RoomTab, header_label};
use crate::design_review::agenda::ItemState;
use crate::design_review::usage::{self, CALL_COLUMNS, COLUMNS};
use crate::theme::Theme;

/// Shown in help and under the Usage table.
pub const USAGE_ACCURACY_NOTE: &str = "The Usage tab is the accurate figure: it prices every call at its own \
     model's rates. The sidebar's token cost can disagree until issue #863 is fixed.";

pub fn draw_design_review(frame: &mut Frame, room: &mut DesignReviewRoom, theme: &Theme) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.effective_bg())),
        area,
    );

    let typing = room.input.is_some();
    let mut constraints = vec![Constraint::Length(2), Constraint::Min(3)];
    if typing {
        constraints.push(Constraint::Length(3));
    }
    constraints.push(Constraint::Length(2));
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    draw_header(frame, chunks[0], room, theme);
    match room.tab {
        RoomTab::Agenda => draw_agenda_tab(frame, chunks[1], room, theme),
        RoomTab::Usage => draw_usage_tab(frame, chunks[1], room, theme),
    }
    let mut next = 2;
    if typing {
        draw_input(frame, chunks[next], room, theme);
        next += 1;
    }
    draw_footer(frame, chunks[next], room, theme);
    if room.help_open {
        draw_help(frame, theme);
    }
}

fn text_style(theme: &Theme) -> Style {
    Style::default().fg(theme.text.to_color())
}

fn muted(theme: &Theme) -> Style {
    Style::default().fg(theme.text_muted.to_color())
}

fn state_color(state: ItemState, theme: &Theme) -> ratatui::style::Color {
    match state {
        ItemState::Accepted => theme.success.to_color(),
        ItemState::ChangeRequested => theme.warning.to_color(),
        ItemState::QuestionOpen | ItemState::ReReview => theme.info.to_color(),
        ItemState::Pending => theme.text_muted.to_color(),
    }
}

fn draw_header(frame: &mut Frame, area: Rect, room: &DesignReviewRoom, theme: &Theme) {
    let tab = |label: &'static str, active: bool| {
        let style = if active {
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            muted(theme)
        };
        Span::styled(label, style)
    };
    let title = Line::from(vec![
        Span::raw("  "),
        Span::styled(
            header_label(),
            Style::default()
                .fg(theme.primary.to_color())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("  ·  Round {}", room.round), text_style(theme)),
        Span::styled(
            format!("  ·  {} (u: usage)", room.cost_total),
            text_style(theme),
        ),
    ]);
    let reviewed = room
        .items
        .iter()
        .filter(|i| ItemState::parse(&i.state) == ItemState::Accepted)
        .count();
    let sub = Line::from(vec![
        Span::raw("  "),
        tab("Agenda", room.tab == RoomTab::Agenda),
        Span::raw("   "),
        tab("Usage", room.tab == RoomTab::Usage),
        Span::styled(
            format!(
                "     {} / {}  ·  {} of {} accepted",
                room.project_name,
                room.feature_name,
                reviewed,
                room.items.len()
            ),
            muted(theme),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(vec![title, sub]).style(Style::default().bg(theme.effective_header_bg())),
        area,
    );
}

fn pane_block<'a>(title: String, theme: &Theme) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(theme.border.to_color()))
        .title(Span::styled(title, text_style(theme)))
        .style(Style::default().bg(theme.effective_header_bg()))
}

fn draw_agenda_tab(frame: &mut Frame, area: Rect, room: &mut DesignReviewRoom, theme: &Theme) {
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(32), Constraint::Min(20)])
        .split(area);
    draw_agenda(frame, panes[0], room, theme);
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(62), Constraint::Min(4)])
        .split(panes[1]);
    draw_content(frame, right[0], room, theme);
    draw_thread(frame, right[1], room, theme);
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let keep = width.saturating_sub(1);
    format!("{}…", text.chars().take(keep).collect::<String>())
}

fn draw_agenda(frame: &mut Frame, area: Rect, room: &mut DesignReviewRoom, theme: &Theme) {
    let block = pane_block(" Agenda ".to_string(), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let visible = inner.height as usize;
    if visible == 0 {
        return;
    }
    if room.selected < room.list_scroll {
        room.list_scroll = room.selected;
    } else if room.selected >= room.list_scroll + visible {
        room.list_scroll = room.selected + 1 - visible;
    }
    let width = inner.width as usize;
    let lines: Vec<Line> = room
        .items
        .iter()
        .enumerate()
        .skip(room.list_scroll)
        .take(visible)
        .map(|(index, item)| {
            let state = ItemState::parse(&item.state);
            let stale = item.stale.tag();
            let body = truncate(&item.title, width.saturating_sub(3 + stale.chars().count()));
            let mut line = Line::from(vec![
                Span::styled(
                    format!("{} ", state.glyph()),
                    Style::default().fg(state_color(state, theme)),
                ),
                Span::styled(body, text_style(theme)),
                Span::styled(stale, Style::default().fg(theme.warning.to_color())),
            ]);
            if index == room.selected {
                line = line.style(
                    Style::default()
                        .bg(theme.selection.to_color())
                        .add_modifier(Modifier::BOLD),
                );
            }
            line
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_content(frame: &mut Frame, area: Rect, room: &mut DesignReviewRoom, theme: &Theme) {
    let (title, stale) = match room.selected_item() {
        Some(item) => (
            format!(
                " {} · {} ",
                truncate(&item.title, area.width.saturating_sub(24) as usize),
                ItemState::parse(&item.state).label()
            ),
            item.stale.banner(),
        ),
        None => (" Content ".to_string(), None),
    };
    let block = pane_block(title, theme);
    let mut inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    if let Some(text) = stale {
        let banner = Rect { height: 1, ..inner };
        frame.render_widget(
            Paragraph::new(Span::styled(
                text,
                Style::default().fg(theme.warning.to_color()),
            )),
            banner,
        );
        inner.y += 1;
        inner.height = inner.height.saturating_sub(1);
    }
    if room.items.is_empty() {
        return;
    }
    super::markdown::draw_markdown_document(
        frame,
        inner,
        &room.content,
        Path::new("design-review.md"),
        &mut room.content_scroll,
        &mut room.content_width,
        &mut room.content_lines,
        theme,
        Some(&mut room.diagrams),
    );
}

/// Lines of a thread, oldest first: a speaker line, the body, a blank line.
fn thread_lines<'a>(room: &DesignReviewRoom, theme: &Theme) -> Vec<Line<'a>> {
    let mut lines = Vec::new();
    for turn in &room.turns {
        let (speaker, color) = match turn.role.as_str() {
            "reviewer" => ("You".to_string(), theme.primary.to_color()),
            "record" => (
                format!("Record ({})", turn.level.as_deref().unwrap_or("L0")),
                theme.info.to_color(),
            ),
            other => (
                match turn.level.as_deref() {
                    Some(level) => format!("{other} ({level})"),
                    None => other.to_string(),
                },
                theme.secondary.to_color(),
            ),
        };
        lines.push(Line::from(Span::styled(
            speaker,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )));
        for body in turn.body.lines() {
            lines.push(Line::from(Span::styled(
                body.to_string(),
                text_style(theme),
            )));
        }
        lines.push(Line::raw(""));
    }
    lines
}

fn draw_thread(frame: &mut Frame, area: Rect, room: &mut DesignReviewRoom, theme: &Theme) {
    let block = pane_block(" Thread ".to_string(), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    if room.turns.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "No discussion yet. s asks a question, n adds a note, c requests a change.",
                muted(theme),
            ))
            .wrap(Wrap { trim: true }),
            inner,
        );
        return;
    }
    let paragraph = Paragraph::new(thread_lines(room, theme)).wrap(Wrap { trim: false });
    let total = paragraph.line_count(inner.width);
    let max_up = total.saturating_sub(inner.height as usize);
    room.thread_scroll = room.thread_scroll.min(max_up);
    let top = (max_up - room.thread_scroll) as u16;
    frame.render_widget(paragraph.scroll((top, 0)), inner);
}

fn draw_input(frame: &mut Frame, area: Rect, room: &DesignReviewRoom, theme: &Theme) {
    let Some(input) = &room.input else {
        return;
    };
    let width = area.width.saturating_sub(4) as usize;
    let shown: String = {
        let chars: Vec<char> = input.text.chars().collect();
        let start = chars.len().saturating_sub(width.saturating_sub(1));
        chars[start..].iter().collect()
    };
    let block = pane_block(format!(" {} ", input.kind.prompt()), theme).border_style(
        Style::default().fg(match input.kind {
            InputKind::RequestChange => theme.warning.to_color(),
            _ => theme.border_focus.to_color(),
        }),
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(shown, text_style(theme)),
            Span::styled("▏", Style::default().fg(theme.primary.to_color())),
        ]))
        .block(block),
        area,
    );
}

fn key_hint<'a>(key: &'a str, label: &'a str, theme: &Theme) -> [Span<'a>; 2] {
    [
        Span::styled(
            format!(" {key}"),
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.shortcut_background.to_color()),
        ),
        Span::styled(format!(" {label} "), muted(theme)),
    ]
}

fn draw_footer(frame: &mut Frame, area: Rect, room: &DesignReviewRoom, theme: &Theme) {
    let hints: Vec<(&str, &str)> = if room.input.is_some() {
        vec![("Enter", "send"), ("Esc", "cancel"), ("Ctrl+U", "clear")]
    } else if room.tab == RoomTab::Usage {
        if room.usage.drill.is_some() {
            vec![
                ("j/k", "move"),
                ("Esc", "back to groups"),
                ("y", "copy Markdown"),
            ]
        } else {
            vec![
                ("j/k", "move"),
                ("Enter", "calls"),
                ("g", "group by"),
                ("y", "copy Markdown"),
                ("Tab", "agenda"),
                ("?", "help"),
                ("Esc", "agenda"),
            ]
        }
    } else {
        vec![
            ("j/k", "move"),
            ("a", "accept"),
            ("n", "note"),
            ("c", "request change"),
            ("s", "ask"),
            ("o", "open files"),
            ("Tab", "usage"),
            ("?", "help"),
            ("Esc", "close"),
        ]
    };
    let mut spans = Vec::new();
    for (key, label) in hints {
        spans.extend(key_hint(key, label, theme));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans))
            .wrap(Wrap { trim: true })
            .style(Style::default().bg(theme.effective_header_bg())),
        area,
    );
}

fn draw_help(frame: &mut Frame, theme: &Theme) {
    let area = centered_rect(78, 80, frame.area());
    frame.render_widget(Clear, area);
    let entry = |key: &str, text: &str| {
        Line::from(vec![
            Span::styled(
                format!("  {key:<12}"),
                Style::default()
                    .fg(theme.primary.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(text.to_string(), text_style(theme)),
        ])
    };
    let heading = |text: &str| {
        Line::from(Span::styled(
            text.to_string(),
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(theme.text.to_color()),
        ))
    };
    let lines = vec![
        heading("Design Review (Claude only for now)"),
        Line::raw(""),
        heading("Agenda"),
        entry("j / k", "Move through the agenda"),
        entry("PgUp / PgDn", "Scroll the content pane"),
        entry("[ / ]", "Scroll the thread"),
        entry("← / →", "Pan a wide diagram"),
        entry("m", "Show a diagram's source"),
        entry("a", "Accept the item"),
        entry("n", "Accept with a note"),
        entry("c", "Request a change"),
        entry("s", "Ask a question (answered from the decision's record)"),
        entry("o / Enter", "Open the item's files in the diff viewer"),
        entry("Tab / u", "Open the Usage tab"),
        entry("Esc / q", "Close the room"),
        Line::raw(""),
        heading("Usage"),
        entry("g", "Group by phase, task, agent, model, item or round"),
        entry("Enter", "Open a group's calls"),
        entry("y", "Copy the table as Markdown"),
        entry("r", "Reload the ledger"),
        Line::raw(""),
        Line::from(Span::styled(
            "~ estimated   * price assumed   - unavailable",
            muted(theme),
        )),
        Line::raw(""),
        Line::from(Span::styled(USAGE_ACCURACY_NOTE, text_style(theme))),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(pane_block(" Help ".to_string(), theme))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn pad(cells: &[String], widths: &[usize]) -> String {
    cells
        .iter()
        .zip(widths)
        .enumerate()
        .map(|(i, (cell, w))| {
            if i == 0 {
                format!("{cell:<w$}")
            } else {
                format!("{cell:>w$}")
            }
        })
        .collect::<Vec<_>>()
        .join("  ")
}

fn widths(header: &[&str], rows: &[Vec<String>]) -> Vec<usize> {
    (0..header.len())
        .map(|i| {
            rows.iter()
                .map(|r| r.get(i).map_or(0, |c| c.chars().count()))
                .chain(std::iter::once(header[i].chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect()
}

fn draw_usage_tab(frame: &mut Frame, area: Rect, room: &mut DesignReviewRoom, theme: &Theme) {
    let block = pane_block(" Usage ".to_string(), theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }
    let table = room.usage.table();
    let note = Line::from(Span::styled(USAGE_ACCURACY_NOTE, muted(theme)));

    if room.usage.rows.is_empty() {
        let lines = vec![
            Line::from(Span::styled(
                "No usage recorded yet. Staff runs and developer turns made for the review appear here.",
                muted(theme),
            )),
            Line::raw(""),
            note,
        ];
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    }

    let header_style = Style::default()
        .add_modifier(Modifier::BOLD)
        .fg(theme.text.to_color());
    let selection = Style::default().bg(theme.selection.to_color());
    // `head` stays put, `body` scrolls to keep the selection visible, `foot`
    // wraps at the bottom.
    let mut head: Vec<Line> = Vec::new();
    let mut body: Vec<Line> = Vec::new();
    let mut foot: Vec<Line> = vec![Line::raw("")];
    let selected_line;

    if let Some(group) = room.usage.drill.and_then(|g| table.groups.get(g)) {
        head.push(Line::from(Span::styled(
            format!(
                "Calls in {} {}  (Esc for groups)",
                table.grouping.label(),
                group.key
            ),
            text_style(theme),
        )));
        head.push(Line::raw(""));
        let rows: Vec<Vec<String>> = group
            .rows
            .iter()
            .map(|i| usage::call_cells(&room.usage.rows[*i]))
            .collect();
        let w = widths(&CALL_COLUMNS, &rows);
        let header: Vec<String> = CALL_COLUMNS.iter().map(|c| c.to_string()).collect();
        head.push(Line::from(Span::styled(pad(&header, &w), header_style)));
        for (index, row) in rows.iter().enumerate() {
            let mut line = Line::from(Span::styled(pad(row, &w), text_style(theme)));
            if index == room.usage.call_selected {
                line = line.style(selection);
            }
            body.push(line);
        }
        selected_line = room.usage.call_selected;
    } else {
        head.push(Line::from(Span::styled(
            format!("Group by: {}  (g to change)", table.grouping.label()),
            text_style(theme),
        )));
        head.push(Line::raw(""));
        let rows = usage::table_cells(&table);
        let mut header: Vec<&str> = COLUMNS.to_vec();
        header[0] = table.grouping.label();
        let w = widths(&header, &rows);
        let names: Vec<String> = header.iter().map(|c| c.to_string()).collect();
        head.push(Line::from(Span::styled(pad(&names, &w), header_style)));
        let total_index = rows.len() - 1;
        for (index, row) in rows.iter().enumerate() {
            if index == total_index {
                body.push(Line::from(Span::styled(
                    "─".repeat(pad(row, &w).chars().count()),
                    muted(theme),
                )));
            }
            let mut line = Line::from(Span::styled(pad(row, &w), text_style(theme)));
            if index == room.usage.selected && index != total_index {
                line = line.style(selection);
            }
            body.push(line);
        }
        selected_line = room.usage.selected;
        foot.push(Line::from(Span::styled(
            usage::footer_line(&table),
            text_style(theme),
        )));
        foot.push(Line::from(Span::styled(usage::MARKER_LEGEND, muted(theme))));
    }
    foot.push(Line::raw(""));
    foot.push(note);

    // The list keeps at least one row; the footer gives way first.
    let foot = Paragraph::new(foot).wrap(Wrap { trim: false });
    let head_height = (head.len() as u16).min(inner.height);
    let foot_height =
        (foot.line_count(inner.width) as u16).min(inner.height.saturating_sub(head_height + 1));
    let body_height = inner.height - head_height - foot_height;
    let [head_area, body_area, foot_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(head_height),
            Constraint::Length(body_height),
            Constraint::Length(foot_height),
        ])
        .areas(inner);

    let visible = body_area.height as usize;
    let scroll = &mut room.usage.scroll;
    if selected_line < *scroll {
        *scroll = selected_line;
    } else if visible > 0 && selected_line >= *scroll + visible {
        *scroll = selected_line + 1 - visible;
    }
    *scroll = (*scroll).min(body.len().saturating_sub(visible));

    frame.render_widget(Paragraph::new(head), head_area);
    frame.render_widget(Paragraph::new(body).scroll((*scroll as u16, 0)), body_area);
    frame.render_widget(foot, foot_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::design_review::DesignReviewItem;
    use crate::design_review::agenda::Staleness;
    use ratatui::{Terminal, backend::TestBackend};

    pub(crate) fn blank_room() -> DesignReviewRoom {
        let item = |position: i64, kind: &str, title: &str, state: &str, stale: Staleness| {
            DesignReviewItem {
                id: format!("i{position}"),
                round_id: "r".into(),
                feature_id: "f".into(),
                project_id: "p".into(),
                position,
                kind: kind.into(),
                ref_key: Some(kind.into()),
                label: None,
                title: title.into(),
                paths: vec![],
                state: state.into(),
                stale,
                created_at: "t".into(),
                updated_at: "t".into(),
            }
        };
        let view = crate::app::ViewState::new(
            "proj".into(),
            "feat".into(),
            "s".into(),
            "w".into(),
            "Claude".into(),
            crate::project::SessionKind::Claude,
            crate::project::VibeMode::default(),
            true,
        );
        DesignReviewRoom {
            from_view: view,
            feature_id: "f".into(),
            project_name: "proj".into(),
            feature_name: "feat".into(),
            workdir: std::path::PathBuf::from("/tmp"),
            round: 1,
            items: vec![
                item(0, "brief", "Brief", "accepted", Staleness::Current),
                item(
                    1,
                    "decision",
                    "d1 Outbox file",
                    "pending",
                    Staleness::PossiblyStale,
                ),
                item(
                    2,
                    "unexplained-hit",
                    "Cargo.toml",
                    "pending",
                    Staleness::Retired,
                ),
            ],
            selected: 1,
            list_scroll: 0,
            decisions: vec![],
            turns: vec![],
            content: "# d1 Outbox file\n\n## Why\n\nappend only\n".into(),
            content_scroll: 0,
            content_width: 0,
            content_lines: vec![],
            diagrams: Default::default(),
            thread_scroll: 0,
            tab: RoomTab::Agenda,
            input: None,
            help_open: false,
            usage: Default::default(),
            cost_total: "—".into(),
        }
    }

    fn render(room: &mut DesignReviewRoom, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|frame| draw_design_review(frame, room, &Theme::default()))
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        buffer
            .content()
            .chunks(width as usize)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn agenda_tab_shows_header_agenda_content_and_thread() {
        let mut room = blank_room();
        let screen = render(&mut room, 120, 36);
        for want in [
            "Design Review · Claude only for now",
            "Round 1",
            "— (u: usage)",
            "✓ Brief",
            "○ d1 Outbox file",
            "stale?",
            "Possibly stale",
            "append only",
            "No discussion yet",
        ] {
            assert!(screen.contains(want), "missing {want:?}:\n{screen}");
        }
    }

    #[test]
    fn a_retired_item_is_not_called_possibly_stale() {
        let mut room = blank_room();
        room.selected = 2;
        let screen = render(&mut room, 120, 36);
        assert!(screen.contains("off agenda"), "{screen}");
        assert!(screen.contains("No longer on the agenda"), "{screen}");
        assert!(!screen.contains("Possibly stale"), "{screen}");
    }

    #[test]
    fn usage_lists_scroll_to_keep_the_selection_visible() {
        let mut room = blank_room();
        room.tab = RoomTab::Usage;
        room.usage.rows = (0..40)
            .map(|i| usage::UsageRow {
                id: i,
                phase: "prepare".into(),
                task: format!("task-{i:02}"),
                agent: "presenter".into(),
                model: Some(format!("model-{i:02}")),
                cost_usd: Some(0.01),
                started_at: "2026-11-13T10:00:00Z".into(),
                ..Default::default()
            })
            .collect();
        room.usage.grouping = usage::Grouping::Model;
        room.usage.selected = 39;
        let screen = render(&mut room, 120, 24);
        assert!(screen.contains("model-39"), "{screen}");
        assert!(!screen.contains("model-00"), "{screen}");

        room.usage.grouping = usage::Grouping::Agent;
        room.usage.selected = 0;
        room.usage.drill = Some(0);
        room.usage.call_selected = 39;
        let screen = render(&mut room, 120, 24);
        assert!(screen.contains("task-39"), "{screen}");
        assert!(!screen.contains("task-00"), "{screen}");
        room.usage.call_selected = 0;
        let screen = render(&mut room, 120, 24);
        assert!(screen.contains("task-00"), "{screen}");
    }

    #[test]
    fn narrow_terminals_still_draw() {
        let mut room = blank_room();
        let screen = render(&mut room, 60, 20);
        assert!(screen.contains("Design Review"), "{screen}");
    }

    #[test]
    fn usage_tab_and_help_render_the_accuracy_note() {
        let mut room = blank_room();
        room.tab = RoomTab::Usage;
        let screen = render(&mut room, 120, 36);
        assert!(screen.contains("No usage recorded yet"), "{screen}");
        assert!(screen.contains("#863"), "{screen}");
        room.help_open = true;
        let screen = render(&mut room, 120, 40);
        assert!(screen.contains("Group by phase"), "{screen}");
    }
}
