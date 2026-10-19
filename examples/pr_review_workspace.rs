//! Disposable PR workspace prototype. No database, GitHub, or agent access.
//! Run: cargo run --locked --example pr_review_workspace

use std::{
    io,
    time::{Duration, Instant},
};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

const ACCENT: Color = Color::Rgb(113, 203, 190);
const MUTED: Color = Color::Rgb(145, 154, 175);
const BG: Color = Color::Rgb(20, 24, 34);
const FILES: [&str; 3] = ["src/recovery.rs", "src/session.rs", "tests/recovery.rs"];
const THREADS: [&str; 3] = [
    "Handle survives a restart?",
    "Retry needs a limit",
    "Cover the empty-session case",
];
const FINDINGS: [&str; 3] = [
    "Stale handle after reconnect",
    "Retry loop can run forever",
    "Missing cleanup on failure",
];
const THREAD_BODY: [&str; 3] = [
    "@maya · GitHub thread · line 44\n\nDoes this handle still belong to the current process after a restart? We might reconnect to an old session.\n\n@author\nGood catch. I will check the process identity before reconnecting.",
    "@alex · GitHub thread · line 46\n\nCould we bound the retry count here? A failed connection currently keeps the recovery worker busy forever.",
    "@maya · GitHub thread · line 47\n\nPlease add a regression test for recovery when the session list is empty.",
];
const AI_BODY: [&str; 3] = [
    "AI · High priority · line 44\n\nA cached handle can point at a process from before the restart. Reusing it without checking identity can reconnect the user to the wrong session.\n\nSuggested check\nCompare the stored process identity with the current one before reconnecting.\n\nGenerated against head a1b2c3 · sample model",
    "AI · Medium priority · line 46\n\nA failed reconnect retries without a limit. A permanently unavailable session can occupy this worker indefinitely.\n\nSuggested check\nAdd a retry budget and surface the last error.\n\nGenerated against head a1b2c3 · sample model",
    "AI · Medium priority · line 48\n\nThe failure path returns without releasing the temporary handle. Repeated failures may leave unused handles behind.\n\nSuggested check\nUse an owned guard for cleanup on both success and error.\n\nGenerated against head a1b2c3 · sample model",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Files,
    Threads,
    Ai,
}

impl Category {
    fn index(self) -> usize {
        match self {
            Self::Files => 0,
            Self::Threads => 1,
            Self::Ai => 2,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Navigator,
    Diff,
    Context,
}

impl Pane {
    fn next(self, reverse: bool) -> Self {
        match (self, reverse) {
            (Self::Navigator, false) | (Self::Context, true) => Self::Diff,
            (Self::Diff, false) | (Self::Navigator, true) => Self::Context,
            _ => Self::Navigator,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Overlay {
    None,
    Draft,
    Preview,
    Agent,
    Help,
}

struct Prototype {
    category: Category,
    selected: [usize; 3],
    file: usize,
    pane: Pane,
    expanded: bool,
    resolved: [bool; 3],
    skipped: [bool; 3],
    read: [bool; 3],
    drafts: [String; 3],
    editor: String,
    diff_scroll: [u16; 3],
    context_scroll: u16,
    overlay_scroll: u16,
    overlay: Overlay,
    ai_started: Option<Instant>,
    notice: String,
}

impl Default for Prototype {
    fn default() -> Self {
        Self {
            category: Category::Threads,
            selected: [0; 3],
            file: 0,
            pane: Pane::Navigator,
            expanded: false,
            resolved: [false; 3],
            skipped: [false; 3],
            read: [false; 3],
            drafts: Default::default(),
            editor: String::new(),
            diff_scroll: [0; 3],
            context_scroll: 0,
            overlay_scroll: 0,
            overlay: Overlay::None,
            ai_started: None,
            notice: "Sample PR · nothing is sent or persisted. Press ? for controls.".into(),
        }
    }
}

impl Prototype {
    fn selection(&self) -> usize {
        self.selected[self.category.index()]
    }

    fn select(&mut self, category: Category, delta: isize) {
        self.category = category;
        let index = category.index();
        self.selected[index] = self.selected[index].saturating_add_signed(delta).min(2);
        self.file = match category {
            Category::Ai => [0, 1, 0][self.selection()],
            _ => self.selection(),
        };
        self.context_scroll = 0;
    }

    fn key(&mut self, key: KeyEvent) -> bool {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return false;
        }
        if self.overlay == Overlay::Draft {
            match key.code {
                KeyCode::Esc => {
                    self.overlay = Overlay::None;
                }
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.drafts[self.file] = self.editor.trim().to_owned();
                    self.overlay = Overlay::None;
                    self.notice = "Draft saved in memory. Open Preview to inspect it.".into();
                }
                KeyCode::Char(c) => self.editor.push(c),
                KeyCode::Enter => self.editor.push('\n'),
                KeyCode::Backspace => {
                    self.editor.pop();
                }
                _ => {}
            }
            return true;
        }
        if self.overlay != Overlay::None {
            match key.code {
                KeyCode::PageDown | KeyCode::Down | KeyCode::Char('j') => {
                    self.overlay_scroll = self.overlay_scroll.saturating_add(1);
                }
                KeyCode::PageUp | KeyCode::Up | KeyCode::Char('k') => {
                    self.overlay_scroll = self.overlay_scroll.saturating_sub(1);
                }
                KeyCode::Esc | KeyCode::Char('q') => self.overlay = Overlay::None,
                KeyCode::Enter if self.overlay == Overlay::Agent => {
                    self.overlay = Overlay::None;
                    self.notice =
                        "Simulated fix complete · returned to the same feedback. No files changed."
                            .into();
                }
                KeyCode::Enter if self.overlay == Overlay::Preview => {
                    self.notice =
                        "Preview only · GitHub submission is not connected in this prototype."
                            .into();
                    self.overlay = Overlay::None;
                }
                _ => {}
            }
            return true;
        }
        match key.code {
            KeyCode::Char('q') => return false,
            KeyCode::Esc => {
                self.expanded = false;
                self.pane = Pane::Navigator;
            }
            KeyCode::Char('1') => self.browse(Category::Files),
            KeyCode::Char('2') => self.browse(Category::Threads),
            KeyCode::Char('3') => self.browse(Category::Ai),
            KeyCode::Tab => self.pane = self.pane.next(false),
            KeyCode::BackTab => self.pane = self.pane.next(true),
            KeyCode::Enter if self.pane == Pane::Navigator => self.pane = Pane::Diff,
            KeyCode::Char('z') => self.expanded = !self.expanded,
            KeyCode::Down | KeyCode::Char('j') => self.move_in_pane(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_in_pane(-1),
            KeyCode::Char('r') => self.read[self.file] = !self.read[self.file],
            KeyCode::Char('s') => {
                let selected = self.selection();
                match self.category {
                    Category::Threads => self.resolved[selected] = !self.resolved[selected],
                    Category::Ai => self.skipped[selected] = !self.skipped[selected],
                    Category::Files => self.read[self.file] = !self.read[self.file],
                }
            }
            KeyCode::Char('c') => {
                self.editor.clone_from(&self.drafts[self.file]);
                self.overlay = Overlay::Draft;
            }
            KeyCode::Char('a') if self.ai_started.is_none() => {
                self.ai_started = Some(Instant::now());
                self.notice = "Simulating AI review · continue reading; results will not move your selection.".into();
            }
            KeyCode::Char('p') => self.open_overlay(Overlay::Preview),
            KeyCode::Char('f') => self.open_overlay(Overlay::Agent),
            KeyCode::Char('?') => self.open_overlay(Overlay::Help),
            _ => {}
        }
        true
    }

    fn open_overlay(&mut self, overlay: Overlay) {
        self.overlay_scroll = 0;
        self.overlay = overlay;
    }

    fn browse(&mut self, category: Category) {
        self.select(category, 0);
        self.pane = Pane::Navigator;
    }

    fn move_in_pane(&mut self, delta: isize) {
        match self.pane {
            Pane::Navigator => self.select(self.category, delta),
            Pane::Diff => {
                self.diff_scroll[self.file] = self.diff_scroll[self.file]
                    .saturating_add_signed(delta as i16)
                    .min(12)
            }
            Pane::Context => {
                self.context_scroll = self
                    .context_scroll
                    .saturating_add_signed(delta as i16)
                    .min(10)
            }
        }
    }

    fn tick(&mut self) {
        if self
            .ai_started
            .is_some_and(|start| start.elapsed() >= Duration::from_secs(3))
        {
            self.ai_started = None;
            self.notice = "AI review ready · 3 sample findings. Press 3 to inspect them.".into();
        }
    }
}

fn panel(title: &str, focused: bool) -> Block<'_> {
    Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if focused {
            ACCENT
        } else {
            Color::Rgb(60, 69, 88)
        }))
}

fn draw(frame: &mut Frame, state: &Prototype) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(Color::Rgb(220, 225, 237))),
        area,
    );
    let rows = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(area);
    let ai = if state.ai_started.is_some() {
        "running…"
    } else {
        "3 findings"
    };
    let header = vec![
        Line::from(vec![
            Span::styled(
                " PR #482 ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::raw("Improve session recovery  ·  feature/recovery"),
        ]),
        Line::from(format!(
            " {} open threads  ·  AI: {ai}  ·  {} drafts  ·  {}/3 files read  ·  SAMPLE DATA",
            state.resolved.iter().filter(|v| !**v).count(),
            state.drafts.iter().filter(|v| !v.is_empty()).count(),
            state.read.iter().filter(|v| **v).count()
        )),
    ];
    frame.render_widget(Paragraph::new(header), rows[0]);
    let body = rows[1];
    if state.expanded || body.width < 85 {
        draw_pane(frame, state, state.pane, body);
    } else if body.width >= 125 {
        let columns = Layout::horizontal([
            Constraint::Percentage(24),
            Constraint::Percentage(44),
            Constraint::Percentage(32),
        ])
        .split(body);
        draw_pane(frame, state, Pane::Navigator, columns[0]);
        draw_pane(frame, state, Pane::Diff, columns[1]);
        draw_pane(frame, state, Pane::Context, columns[2]);
    } else {
        let columns = Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)])
            .split(body);
        draw_pane(frame, state, Pane::Diff, columns[0]);
        draw_pane(frame, state, Pane::Context, columns[1]);
        if state.pane == Pane::Navigator {
            let drawer = Rect {
                width: body.width.min(40),
                ..body
            };
            frame.render_widget(Clear, drawer);
            draw_pane(frame, state, Pane::Navigator, drawer);
        }
    }
    let footer = vec![
        Line::styled(
            " 1 Files  2 Threads  3 AI  ·  Tab focus  j/k move  z expand  ? help  q quit",
            Style::default().fg(ACCENT),
        ),
        Line::from(" c draft  s resolve/skip  r read  a run AI  f fix  p preview"),
        Line::styled(format!(" {}", state.notice), Style::default().fg(MUTED)),
    ];
    frame.render_widget(Paragraph::new(footer), rows[2]);
    if state.overlay != Overlay::None {
        draw_overlay(frame, state);
    }
}

fn draw_pane(frame: &mut Frame, state: &Prototype, pane: Pane, area: Rect) {
    let focused = pane == state.pane;
    match pane {
        Pane::Navigator => {
            let rows = Layout::vertical([Constraint::Length(2), Constraint::Min(0)]).split(area);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    category_label("1 Files ", state.category == Category::Files),
                    category_label("2 Threads ", state.category == Category::Threads),
                    category_label("3 AI", state.category == Category::Ai),
                ])),
                rows[0],
            );
            let items: Vec<ListItem<'_>> = (0..3)
                .map(|index| {
                    let (label, status) = match state.category {
                        Category::Files => (
                            FILES[index],
                            if state.read[index] { "read" } else { "unread" },
                        ),
                        Category::Threads => (
                            THREADS[index],
                            if state.resolved[index] {
                                "resolved (demo)"
                            } else {
                                "open · GitHub"
                            },
                        ),
                        Category::Ai => (
                            FINDINGS[index],
                            if state.skipped[index] {
                                "skipped"
                            } else {
                                "kept · unpublished"
                            },
                        ),
                    };
                    ListItem::new(vec![
                        Line::from(label),
                        Line::styled(format!("  {status}"), Style::default().fg(MUTED)),
                        Line::from(""),
                    ])
                })
                .collect();
            let list = List::new(items)
                .block(panel("Navigate", focused))
                .highlight_style(Style::default().bg(Color::Rgb(38, 64, 69)).fg(ACCENT))
                .highlight_symbol("› ");
            frame.render_stateful_widget(
                list,
                rows[1],
                &mut ListState::default().with_selected(Some(state.selection())),
            );
        }
        Pane::Diff => {
            let lines = diff_lines(state.file);
            frame.render_widget(
                Paragraph::new(lines)
                    .block(panel(FILES[state.file], focused))
                    .scroll((state.diff_scroll[state.file], 0)),
                area,
            );
        }
        Pane::Context => {
            let (title, text) = match state.category {
                Category::Threads => (
                    "GitHub thread",
                    format!(
                        "{}\n\nStatus: {}\n\ns resolve (demo) · f fix\nc write your own draft",
                        THREAD_BODY[state.selection()],
                        if state.resolved[state.selection()] {
                            "resolved locally in prototype"
                        } else {
                            "open"
                        }
                    ),
                ),
                Category::Ai => (
                    "AI finding",
                    format!(
                        "{}\n\nStatus: {}\n\ns keep/skip · f fix\np preview outgoing review",
                        AI_BODY[state.selection()],
                        if state.skipped[state.selection()] {
                            "skipped"
                        } else {
                            "kept · unpublished"
                        }
                    ),
                ),
                Category::Files => (
                    "Your review",
                    format!(
                        "{}\n\nFile {}\n\nDraft comment\n{}\n\nc write/edit draft · r mark read\np preview outgoing review",
                        FILES[state.file],
                        if state.read[state.file] {
                            "read"
                        } else {
                            "not yet read"
                        },
                        if state.drafts[state.file].is_empty() {
                            "No draft yet."
                        } else {
                            &state.drafts[state.file]
                        }
                    ),
                ),
            };
            frame.render_widget(
                Paragraph::new(text)
                    .wrap(Wrap { trim: false })
                    .scroll((state.context_scroll, 0))
                    .block(panel(title, focused)),
                area,
            );
        }
    }
}

fn category_label(text: &str, selected: bool) -> Span<'_> {
    Span::styled(
        text,
        Style::default()
            .fg(if selected { ACCENT } else { MUTED })
            .add_modifier(if selected {
                Modifier::BOLD
            } else {
                Modifier::empty()
            }),
    )
}

fn diff_lines(file: usize) -> Vec<Line<'static>> {
    let source: &[&str] = match file {
        0 => &[
            "    @@ -38,9 +38,14 @@",
            " 38 use crate::session::Handle;",
            " 39",
            " 40 pub fn recover(cache: &Cache) -> Result<()> {",
            " 41     let session = cache.current();",
            " 42",
            "-43     let handle = connect(session)?;",
            "+43     let handle = cache.cached_handle();",
            "+44     reconnect(handle)?;     // [thread] [AI]",
            " 45",
            "+46     while !handle.is_ready() {",
            "+47         handle.retry()?;",
            "+48     }",
            " 49",
            " 50     Ok(())",
            " 51 }",
            "",
            "    [thread] Handle survives a restart?",
            "    [AI] Stale handle after reconnect",
            "",
            "    Sample diff · c to comment · r mark read",
        ],
        1 => &[
            "    @@ -40,6 +40,12 @@",
            " 40 impl Session {",
            " 41     pub fn reconnect(&mut self) -> Result<()> {",
            " 42         self.state = State::Connecting;",
            " 43",
            "-44         self.connect_once()?;",
            "+44         loop {",
            "+45             if self.connect_once().is_ok() {",
            "+46                 break;    // [thread] [AI]",
            "+47             }",
            "+48         }",
            " 49",
            " 50         self.state = State::Ready;",
            " 51         Ok(())",
            " 52     }",
            " 53 }",
            "",
            "    [thread] Retry needs a limit",
            "    [AI] Retry loop can run forever",
        ],
        _ => &[
            "    @@ -40,3 +40,13 @@",
            " 40 #[test]",
            " 41 fn reconnects_after_restart() {",
            " 42     let fixture = Fixture::new();",
            "+43     fixture.start_session();",
            "+44     fixture.restart();",
            "+45",
            "+46     assert!(fixture.recover().is_ok());",
            "+47     assert!(fixture.session_is_ready());",
            "+48 }",
            "",
            "    [thread] Cover the empty-session case",
            "    [AI] Missing cleanup on failure",
            "",
            "    Sample data: findings may need verification.",
        ],
    };
    source
        .iter()
        .map(|line| {
            let color = if line.starts_with('+') {
                Color::Rgb(139, 211, 158)
            } else if line.starts_with('-') {
                Color::Rgb(237, 145, 147)
            } else if line.contains("[thread]") || line.contains("[AI]") {
                ACCENT
            } else {
                Color::Rgb(202, 211, 228)
            };
            Line::styled(*line, Style::default().fg(color))
        })
        .collect()
}

fn draw_overlay(frame: &mut Frame, state: &Prototype) {
    let area = frame.area();
    let width = area.width.min(88);
    let height = area.height.min(28);
    let popup = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let (title, text) = match state.overlay {
        Overlay::Draft => ("Draft comment · Ctrl+S save · Esc cancel", format!("{} · sample line 44\n\n{}▏", FILES[state.file], state.editor)),
        Overlay::Preview => {
            let mut text = String::from("PREVIEW ONLY · no GitHub connection\n\nYOUR DRAFT COMMENTS\n");
            let mut count = 0;
            for (index, draft) in state.drafts.iter().enumerate().filter(|(_, value)| !value.is_empty()) {
                text.push_str(&format!("\n{}\n{}\n", FILES[index], draft));
                count += 1;
            }
            if count == 0 { text.push_str("No draft comments.\n"); }
            text.push_str("\nAI FINDINGS · explicitly attributed · unpublished\n");
            for (index, finding) in FINDINGS.iter().enumerate() {
                if !state.skipped[index] { text.push_str(&format!("• {finding}\n")); }
            }
            text.push_str("\nResolved threads are not outgoing review comments.\n\nEnter closes this preview without posting. Esc returns.");
            ("Outgoing review preview", text)
        }
        Overlay::Agent => ("Simulated fix session", format!("PR #482 · {}\n\nSelected feedback: {}\n\nAn agent would receive the selected feedback and code context here. This prototype does not launch an agent.\n\nEnter simulates completion and returns to your exact selection.\nEsc returns without simulating a fix.", FILES[state.file], match state.category { Category::Threads => THREADS[state.selection()], Category::Ai => FINDINGS[state.selection()], Category::Files => "Manual review of this file" })),
        Overlay::Help => ("PR workspace prototype · controls", "1 / 2 / 3    Files / GitHub threads / AI findings\nTab          Focus next pane (Shift+Tab goes back)\nj / k        Move in navigator; scroll code or context\nEnter        Move from navigator to code\nz            Expand or restore focused pane\nEsc          Restore layout and focus navigator\n\nc            Write a draft for the selected file\nCtrl+S       Save draft in memory (inside editor)\nr            Toggle file read status\ns            Resolve thread / skip AI finding (demo)\na            Simulate a background AI review\nf            Visit simulated agent fix session\np            Preview human drafts and kept AI findings\nq / Ctrl+C   Quit (all prototype state is discarded)\n\nWide: three panes. Medium: navigator drawer.\nNarrow: focused pane only. Resize your terminal to try.\n\nAll data is fictional. No external calls or saved changes.\nEsc closes help.".into()),
        Overlay::None => return,
    };
    frame.render_widget(Clear, popup);
    let block = panel(title, true);
    let inner = block.inner(popup);
    let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
    let max_scroll = paragraph
        .line_count(inner.width)
        .saturating_sub(usize::from(inner.height))
        .min(usize::from(u16::MAX)) as u16;
    let scroll = if state.overlay == Overlay::Draft {
        max_scroll
    } else {
        state.overlay_scroll.min(max_scroll)
    };
    frame.render_widget(
        paragraph
            .scroll((scroll, 0))
            .block(block)
            .style(Style::default().bg(BG)),
        popup,
    );
}

fn main() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = (|| {
        let mut state = Prototype::default();
        loop {
            state.tick();
            terminal.draw(|frame| draw(frame, &state))?;
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && !state.key(key)
            {
                break;
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn switching_categories_restores_selection_and_draft_survives_cancel() {
        let mut state = Prototype::default();
        state.select(Category::Threads, 1);
        state.select(Category::Ai, 2);
        state.select(Category::Threads, 0);
        assert_eq!(state.file, 1);
        state.drafts[1] = "Original draft".into();
        state.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
        state.key(KeyEvent::new(KeyCode::Char('!'), KeyModifiers::NONE));
        state.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(state.drafts[1], "Original draft");
    }

    #[test]
    fn background_completion_keeps_reading_position() {
        let mut state = Prototype::default();
        state.select(Category::Files, 2);
        state.pane = Pane::Diff;
        state.diff_scroll[2] = 5;
        state.ai_started = Some(Instant::now() - Duration::from_secs(4));
        state.tick();
        assert!(state.ai_started.is_none());
        assert_eq!(state.file, 2);
        assert_eq!(state.diff_scroll[2], 5);
        assert!(state.pane == Pane::Diff);
    }

    #[test]
    fn renders_responsive_layouts_and_overlays() {
        for (width, height) in [(150, 40), (100, 30), (60, 24), (20, 8)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            let mut state = Prototype::default();
            for pane in [Pane::Navigator, Pane::Diff, Pane::Context] {
                state.pane = pane;
                for overlay in [
                    Overlay::None,
                    Overlay::Draft,
                    Overlay::Preview,
                    Overlay::Agent,
                    Overlay::Help,
                ] {
                    state.overlay = overlay;
                    terminal.draw(|frame| draw(frame, &state)).unwrap();
                }
            }
        }
    }
}
