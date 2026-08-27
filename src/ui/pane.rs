use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
    },
};

use crate::app::{TextSelection, ViewState};
use crate::context_display::format_context_indicator;
use crate::context_tracking::SessionContextSnapshot;
use crate::project::{SessionKind, VibeMode};
use crate::theme::Theme;

const LEADER_COMMANDS: &[(&str, &str)] = &[
    ("q", "Exit view"),
    ("t / T", "Next / prev session"),
    ("w", "Session switcher"),
    ("/", "Command picker"),
    ("h", "Bookmark picker"),
    ("H", "Bookmark session"),
    ("M", "Unbookmark session"),
    ("1-9", "Jump to bookmark slot"),
    ("i", "Needs attention"),
    ("s", "Steering coach (experimental)"),
    ("g", "Generate summary"),
    ("l", "Latest prompt"),
    ("p", "Prompt library"),
    ("d", "Diff viewer (all changes / commit)"),
    ("m", "Markdown viewer"),
    ("n", "Open current plan"),
    ("F", "Fresh context"),
    ("X", "Dismiss context hint"),
    ("b", "Show / hide sidebar"),
    ("v", "Expand / collapse todos"),
    ("V", "Check pending diff review"),
    ("o", "Scroll mode"),
    ("r", "Refresh statuses"),
    ("R", "Refresh pane sizing"),
    ("x", "Stop session"),
    ("c", "Copy Remote Control URL"),
    ("C", "Toggle Remote Control (/rc)"),
    ("O", "Open Remote Control URL"),
    ("f", "Final review"),
    ("G", "PR Triage"),
    ("W", "AI Review"),
    ("D", "Debug log"),
    ("?", "Help"),
];

const CLAUDE_SIDEBAR_WIDTH: u16 = 32;
const OPENCODE_SIDEBAR_WIDTH: u16 = 36;
const SIDEBAR_MIN_MAIN_WIDTH: u16 = 72;
pub(crate) const SCROLLBAR_WIDTH: u16 = 1;

#[derive(Debug, Clone)]
pub(crate) struct AgentSidebarData {
    pub agent_kind: SessionKind,
    pub status_text: String,
    #[allow(dead_code)] // populated but not rendered yet
    pub model_text: Option<String>,
    pub prompt_text: String,
    pub work_text: Option<String>,
    pub todos_text: Option<String>,
    /// TODO-menu-originated session references, resolved from AMF's TODO DB.
    /// The section content is global (every reference across all projects).
    pub active_todos_text: Option<String>,
    /// Whether the *currently viewed* session itself carries a menu-launched
    /// TODO reference. `leader z` / `leader Z` only act on the current
    /// session, so the header affordance is shown only when this is true.
    pub active_todo_affordance: bool,
    pub summary_text: String,
    pub pr_triage_text: Option<String>,
    pub plan_text: String,
    pub context_snapshot: Option<SessionContextSnapshot>,
    pub context_hint_visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ContentLayout {
    main: Rect,
    sidebar: Option<Rect>,
}

#[derive(Debug, Clone)]
struct SidebarSection {
    title: &'static str,
    body: String,
    constraint: Constraint,
}

pub(crate) fn viewing_main_width(view: &ViewState, total_width: u16) -> u16 {
    sidebar_width(view, total_width)
        .map(|sidebar_width| total_width.saturating_sub(sidebar_width))
        .unwrap_or(total_width)
}

fn preferred_sidebar_width(view: &ViewState) -> u16 {
    match view.session_kind {
        SessionKind::Opencode => OPENCODE_SIDEBAR_WIDTH,
        _ => CLAUDE_SIDEBAR_WIDTH,
    }
}

fn sidebar_width(view: &ViewState, total_width: u16) -> Option<u16> {
    view.sidebar_session_kind()?;

    let sidebar_width = preferred_sidebar_width(view);
    if total_width < SIDEBAR_MIN_MAIN_WIDTH + sidebar_width {
        return None;
    }

    Some(sidebar_width)
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

/// The embedded view's needs-attention badge, or `None` when nothing wants
/// looking at. Public so hit-testing can measure exactly what the header
/// renders rather than reimplementing it.
pub fn attention_badge_text(attention_count: usize) -> Option<String> {
    (attention_count > 0).then(|| {
        format!(
            " | {} need{} attention",
            attention_count,
            if attention_count == 1 { "s" } else { "" },
        )
    })
}

#[allow(clippy::too_many_arguments)]
#[allow(dead_code)] // exercised only by unit tests
pub fn draw(
    frame: &mut Frame,
    view: &ViewState,
    pane_content: &str,
    sidebar_data: Option<&AgentSidebarData>,
    leader_active: bool,
    attention_count: usize,
    tmux_cursor: Option<(u16, u16)>,
    compose_intercept: Option<bool>,
    next_prev_feature: (Option<char>, Option<char>),
    theme: &Theme,
) {
    let throbber_state = throbber_widgets_tui::ThrobberState::default();
    draw_with_lines(
        frame,
        view,
        pane_content,
        &[],
        sidebar_data,
        leader_active,
        attention_count,
        tmux_cursor,
        compose_intercept,
        next_prev_feature,
        &throbber_state,
        theme,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_with_lines(
    frame: &mut Frame,
    view: &ViewState,
    pane_content: &str,
    pane_lines: &[Line<'static>],
    sidebar_data: Option<&AgentSidebarData>,
    leader_active: bool,
    attention_count: usize,
    tmux_cursor: Option<(u16, u16)>,
    compose_intercept: Option<bool>,
    next_prev_feature: (Option<char>, Option<char>),
    throbber_state: &throbber_widgets_tui::ThrobberState,
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
                .fg(theme.warning.to_color())
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
            format!("| SCROLL COPY {mode_label} "),
            Style::default()
                .fg(theme.shortcut_text.to_color())
                .bg(theme.info.to_color())
                .add_modifier(Modifier::BOLD),
        ));
        let help = if view.scroll_passthrough {
            "wheel/j/k/Ctrl+j/k:PgUp/Dn - q/Esc:exit"
        } else {
            "wheel/j/k/Ctrl+j/k:scroll PgUp/Dn:page - q/Esc:exit"
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

    if !view.scroll_mode
        && let Some(text) = attention_badge_text(attention_count)
    {
        header_spans.push(Span::styled(
            text,
            Style::default()
                .fg(theme.danger.to_color())
                .add_modifier(Modifier::BOLD),
        ));
    }

    let header = Paragraph::new(Line::from(header_spans))
        .style(Style::default().bg(theme.effective_header_bg()));
    frame.render_widget(header, header_area);

    if let Some(sidebar_area) = layout.sidebar {
        let fallback_agent_kind = view.sidebar_session_kind().unwrap_or(SessionKind::Claude);
        draw_agent_sidebar(
            frame,
            sidebar_area,
            sidebar_data,
            fallback_agent_kind,
            theme,
        );
    }

    if view.startup_mask_active() {
        draw_startup_loading(frame, main_content_area, view, throbber_state, theme);
    } else if view.scroll_mode && !view.scroll_passthrough {
        let scrollbar_width = SCROLLBAR_WIDTH.min(main_content_area.width);
        let content_width = main_content_area.width.saturating_sub(scrollbar_width);
        let content_area = Rect::new(
            main_content_area.x,
            main_content_area.y,
            content_width,
            main_content_area.height,
        );

        if scrollbar_width > 0 {
            let scrollbar_area = Rect::new(
                main_content_area.x + content_width,
                main_content_area.y,
                scrollbar_width,
                main_content_area.height,
            );
            let scrollbar = Scrollbar::default()
                .orientation(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("↑"))
                .end_symbol(Some("↓"));
            let mut scrollbar_state = ScrollbarState::new(view.scroll_total_lines)
                .position(view.scroll_offset)
                .viewport_content_length(main_content_area.height as usize);
            frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
        }

        let text = scroll_content_to_lines_with_selection(
            &view.scroll_content,
            view.scroll_offset,
            main_content_area.height,
            content_area.width,
            &view.selection,
            theme,
        );
        let paragraph = Paragraph::new(text).style(
            Style::default()
                .fg(theme.text.to_color())
                .bg(theme.effective_bg()),
        );
        frame.render_widget(paragraph, content_area);
    } else {
        let pane_style = Style::default()
            .fg(theme.text.to_color())
            .bg(theme.effective_bg());
        if view.selection.has_selection || pane_lines.is_empty() {
            let text = ansi_to_ratatui_text_with_selection(
                pane_content,
                main_content_area.width,
                main_content_area.height,
                &view.selection,
                theme,
            );
            let paragraph = Paragraph::new(text).style(pane_style);
            frame.render_widget(paragraph, main_content_area);
        } else {
            // Write the already-rendered lines straight into the frame
            // buffer instead of cloning every line per draw
            // (`pane_lines.to_vec()` was a per-frame allocation of the
            // whole screen's spans).
            let buf = frame.buffer_mut();
            buf.set_style(main_content_area, pane_style);
            for (i, line) in pane_lines
                .iter()
                .take(main_content_area.height as usize)
                .enumerate()
            {
                buf.set_line(
                    main_content_area.x,
                    main_content_area.y + i as u16,
                    line,
                    main_content_area.width,
                );
            }
        }

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
        draw_leader_menu(
            frame,
            main_content_area,
            theme,
            compose_intercept,
            next_prev_feature,
        );
    }
}

fn draw_startup_loading(
    frame: &mut Frame,
    area: Rect,
    view: &ViewState,
    throbber_state: &throbber_widgets_tui::ThrobberState,
    theme: &Theme,
) {
    let bg = theme.effective_bg();
    frame.render_widget(Block::default().style(Style::default().bg(bg)), area);

    if area.width < 20 || area.height < 3 {
        return;
    }

    let panel_height = 5.min(area.height);
    let panel_y = area.y + area.height.saturating_sub(panel_height) / 2;
    let panel = Rect::new(area.x, panel_y, area.width, panel_height);
    let harness = match view.session_kind {
        SessionKind::Claude => "Claude",
        SessionKind::Codex => "Codex",
        SessionKind::Opencode => "opencode",
        SessionKind::Pi => "Pi",
        _ => "agent",
    };
    let throbber = throbber_widgets_tui::Throbber::default()
        .throbber_style(Style::default().fg(theme.warning.to_color()))
        .throbber_set(throbber_widgets_tui::BRAILLE_EIGHT_DOUBLE)
        .use_type(throbber_widgets_tui::WhichUse::Spin);
    let spinner = throbber.to_symbol_span(throbber_state);
    let lines = vec![
        Line::from(vec![
            spinner,
            Span::styled(
                format!(" Starting {harness}"),
                Style::default()
                    .fg(theme.text.to_color())
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            "Preparing the harness...",
            Style::default().fg(theme.secondary.to_color()),
        )),
    ];
    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .style(Style::default().bg(bg))
        .wrap(Wrap { trim: true });
    frame.render_widget(paragraph, panel);
}

fn draw_agent_sidebar(
    frame: &mut Frame,
    area: Rect,
    data: Option<&AgentSidebarData>,
    fallback_agent_kind: SessionKind,
    theme: &Theme,
) {
    if area.width < 16 || area.height < 8 {
        return;
    }

    let (agent_kind, title, title_color) = data
        .map(|data| {
            let (title, color) = sidebar_title_and_color(&data.agent_kind, theme);
            (data.agent_kind.clone(), title, color)
        })
        .unwrap_or_else(|| {
            let kind = fallback_agent_kind;
            let (title, color) = sidebar_title_and_color(&kind, theme);
            (kind, title, color)
        });

    let block = Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(title_color)
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

    let fallback = AgentSidebarData {
        agent_kind,
        status_text: String::new(),
        model_text: None,
        prompt_text: String::new(),
        work_text: None,
        todos_text: None,
        active_todos_text: None,
        active_todo_affordance: false,
        summary_text: String::new(),
        pr_triage_text: None,
        plan_text: String::new(),
        context_snapshot: None,
        context_hint_visible: false,
    };
    let data = data.unwrap_or(&fallback);
    let sections_with_content = sidebar_sections(data, inner.width);
    let constraints = sections_with_content
        .iter()
        .map(|section| section.constraint)
        .collect::<Vec<_>>();

    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);
    for (sidebar_section, section) in sections_with_content.iter().zip(sections.iter()) {
        let accent = sidebar_section_color(sidebar_section.title, theme);
        let mut block = Block::default()
            .title_top(Line::from(Span::styled(
                format!(" {} ", sidebar_section.title),
                Style::default().fg(accent).add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(accent));
        if sidebar_section.title == "Prompt" {
            block = block.title_top(
                Line::from(Span::styled(
                    " <leader l> ",
                    Style::default().fg(theme.text_muted.to_color()),
                ))
                .alignment(Alignment::Right),
            );
        }
        if sidebar_section.title == "PR Triage" {
            block = block.title_top(
                Line::from(Span::styled(
                    " <leader G> ",
                    Style::default().fg(theme.text_muted.to_color()),
                ))
                .alignment(Alignment::Right),
            );
        }
        if sidebar_section.title == "Plan" {
            block = block.title_top(
                Line::from(Span::styled(
                    " <leader n> ",
                    Style::default().fg(theme.text_muted.to_color()),
                ))
                .alignment(Alignment::Right),
            );
        }
        if sidebar_section.title == "Fresh Context" {
            block = block.title_top(
                Line::from(Span::styled(
                    " <leader F> ",
                    Style::default().fg(theme.text_muted.to_color()),
                ))
                .alignment(Alignment::Right),
            );
        }
        // `leader z` / `leader Z` act only on the viewed session, so the
        // header hint is shown only when that session has its own reference —
        // not merely because some other session's reference populated the
        // (globally-scoped) section body.
        if sidebar_section.title == "Active TODOs" && data.active_todo_affordance {
            block = block.title_top(
                Line::from(Span::styled(
                    " <leader z complete · Z clear> ",
                    Style::default().fg(theme.text_muted.to_color()),
                ))
                .alignment(Alignment::Right),
            );
        }
        let paragraph = Paragraph::new(styled_sidebar_lines(
            sidebar_section.title,
            sidebar_section.body.as_str(),
            theme,
        ))
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(theme.effective_bg()))
        .block(block);
        frame.render_widget(paragraph, *section);
    }
}

fn sidebar_sections(data: &AgentSidebarData, section_width: u16) -> Vec<SidebarSection> {
    let mut sections = Vec::new();

    if !data.status_text.trim().is_empty() {
        sections.push(SidebarSection {
            title: "Status",
            body: data.status_text.clone(),
            constraint: Constraint::Length(status_section_height(&data.status_text, section_width)),
        });
    }

    if data.context_hint_visible
        && let Some(snapshot) = data.context_snapshot.as_ref()
    {
        let indicator = format_context_indicator(snapshot);
        let body = format!(
            "Usage: {}\nAction: Fresh context: <leader F>\nDismiss: <leader X>",
            indicator.text
        );
        // Three labelled lines that each wrap to two inner lines at the
        // default 32-column sidebar width -- the ceiling has to clear five so
        // the `Dismiss` action is never the row that gets clipped.
        sections.push(SidebarSection {
            title: "Fresh Context",
            constraint: Constraint::Length(sidebar_section_height(&body, section_width, 2, 6)),
            body,
        });
    }

    if !data.plan_text.trim().is_empty() {
        sections.push(SidebarSection {
            title: "Plan",
            body: data.plan_text.clone(),
            constraint: Constraint::Length(sidebar_section_height(
                &data.plan_text,
                section_width,
                1,
                2,
            )),
        });
    }

    if let Some(pr_triage_text) = data
        .pr_triage_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        sections.push(SidebarSection {
            title: "PR Triage",
            body: pr_triage_text.to_string(),
            constraint: Constraint::Length(sidebar_section_height(
                pr_triage_text,
                section_width,
                2,
                6,
            )),
        });
    }

    let is_opencode = matches!(data.agent_kind, SessionKind::Opencode);

    if let Some(work_text) = data.work_text.as_deref() {
        sections.push(SidebarSection {
            title: "Work",
            body: work_text.to_string(),
            constraint: Constraint::Length(sidebar_section_height(work_text, section_width, 2, 6)),
        });
    }
    if !is_opencode && !data.summary_text.trim().is_empty() {
        sections.push(SidebarSection {
            title: "Summary",
            body: data.summary_text.clone(),
            constraint: Constraint::Length(summary_section_height(
                &data.summary_text,
                section_width,
            )),
        });
    }
    if !data.prompt_text.trim().is_empty() {
        sections.push(SidebarSection {
            title: "Prompt",
            body: data.prompt_text.clone(),
            constraint: Constraint::Length(prompt_section_height(&data.prompt_text, section_width)),
        });
    }
    if let Some(todos_text) = data.todos_text.as_deref() {
        sections.push(SidebarSection {
            title: "Todos",
            body: todos_text.to_string(),
            constraint: Constraint::Length(sidebar_section_height(
                todos_text,
                section_width,
                2,
                13,
            )),
        });
    }
    if let Some(active_todos_text) = data.active_todos_text.as_deref() {
        sections.push(SidebarSection {
            title: "Active TODOs",
            body: active_todos_text.to_string(),
            constraint: Constraint::Length(sidebar_section_height(
                active_todos_text,
                section_width,
                2,
                10,
            )),
        });
    }
    if is_opencode && !data.summary_text.trim().is_empty() {
        sections.push(SidebarSection {
            title: "Summary",
            body: data.summary_text.clone(),
            constraint: Constraint::Length(summary_section_height(
                &data.summary_text,
                section_width,
            )),
        });
    }

    sections
}

fn sidebar_title_and_color(agent_kind: &SessionKind, theme: &Theme) -> (&'static str, Color) {
    match agent_kind {
        SessionKind::Claude => ("Claude Sidebar", theme.session_icon_claude.to_color()),
        SessionKind::Codex => ("Codex Sidebar", theme.session_icon_codex.to_color()),
        SessionKind::Opencode => ("Opencode Sidebar", theme.session_icon_opencode.to_color()),
        SessionKind::Pi => ("Pi Sidebar", theme.primary.to_color()),
        _ => ("Harness Sidebar", theme.border.to_color()),
    }
}

fn sidebar_section_color(title: &str, theme: &Theme) -> Color {
    match title {
        "Status" => theme.warning.to_color(),
        "Prompt" => theme.secondary.to_color(),
        "Work" => theme.primary.to_color(),
        "Todos" => theme.success.to_color(),
        "Active TODOs" => theme.success.to_color(),
        "Summary" => theme.info.to_color(),
        "PR Triage" => theme.info.to_color(),
        "Plan" => theme.warning.to_color(),
        "Fresh Context" => theme.danger.to_color(),
        _ => theme.border.to_color(),
    }
}

fn sidebar_section_height(
    body: &str,
    section_width: u16,
    min_inner_lines: u16,
    max_inner_lines: u16,
) -> u16 {
    let inner_width = section_width.saturating_sub(2).max(1) as usize;
    let inner_lines = body
        .lines()
        .map(|line| {
            let line_width = line.chars().count().max(1);
            line_width.div_ceil(inner_width)
        })
        .sum::<usize>() as u16;
    inner_lines.clamp(min_inner_lines, max_inner_lines) + 2
}

fn prompt_section_height(body: &str, section_width: u16) -> u16 {
    sidebar_section_height(body, section_width, 1, 3)
}

fn status_section_height(body: &str, section_width: u16) -> u16 {
    sidebar_section_height(body, section_width, 1, 8)
}

fn summary_section_height(body: &str, section_width: u16) -> u16 {
    sidebar_section_height(body, section_width, 1, 4)
}

fn styled_sidebar_lines<'a>(title: &str, body: &'a str, theme: &Theme) -> Vec<Line<'a>> {
    body.lines()
        .map(|line| {
            if title == "Active TODOs" && line.ends_with(" · complete") {
                return Line::from(Span::styled(
                    line.to_string(),
                    Style::default()
                        .fg(theme.success.to_color())
                        .add_modifier(Modifier::DIM),
                ));
            }
            // Progress bar: "████░░░░ 2/5"
            if title == "Todos" && (line.starts_with('█') || line.starts_with('░')) {
                let split = line.find('░').unwrap_or(line.len());
                let (filled, rest) = line.split_at(split);
                return Line::from(vec![
                    Span::styled(
                        filled.to_string(),
                        Style::default().fg(theme.success.to_color()),
                    ),
                    Span::styled(
                        rest.to_string(),
                        Style::default().fg(theme.text_muted.to_color()),
                    ),
                ]);
            }
            // Checkbox lines: "✓ …", "● …", "○ …"
            if title == "Todos" {
                if let Some(rest) = line.strip_prefix("✓ ") {
                    return Line::from(vec![
                        Span::styled(
                            "✓ ".to_string(),
                            Style::default().fg(theme.success.to_color()),
                        ),
                        Span::styled(
                            rest.to_string(),
                            Style::default().fg(theme.text_muted.to_color()),
                        ),
                    ]);
                }
                if let Some(rest) = line.strip_prefix("● ") {
                    return Line::from(vec![
                        Span::styled(
                            "● ".to_string(),
                            Style::default()
                                .fg(theme.info.to_color())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(rest.to_string(), Style::default().fg(theme.text.to_color())),
                    ]);
                }
                if let Some(rest) = line.strip_prefix("○ ") {
                    return Line::from(vec![
                        Span::styled(
                            "○ ".to_string(),
                            Style::default().fg(theme.text_muted.to_color()),
                        ),
                        Span::styled(rest.to_string(), Style::default().fg(theme.text.to_color())),
                    ]);
                }
                if line.starts_with('+') {
                    return Line::from(Span::styled(
                        line.to_string(),
                        Style::default().fg(theme.text_muted.to_color()),
                    ));
                }
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

fn sidebar_value_style(title: &str, label: &str, value: &str, theme: &Theme) -> Style {
    let lower = value.to_lowercase();
    let color = if label == "State" {
        match lower.as_str() {
            "active" => theme.status_active.to_color(),
            "idle" => theme.status_idle.to_color(),
            "stopped" => theme.status_stopped.to_color(),
            _ => theme.text.to_color(),
        }
    } else if lower.contains("waiting") {
        theme.status_waiting.to_color()
    } else if lower.contains("thinking") || lower.contains("running tool") {
        theme.info.to_color()
    } else if title == "PR Triage" && (lower.contains("working") || lower.contains("running")) {
        theme.warning.to_color()
    } else if lower.contains("ready") {
        theme.success.to_color()
    } else if lower.contains("generating") {
        theme.info.to_color()
    } else if lower.contains("unavailable") || lower.contains("no summary yet") {
        theme.text_muted.to_color()
    } else if label == "Hint" {
        theme.info.to_color()
    } else if title == "Todos" {
        theme.success.to_color()
    } else if title == "Prompt" || title == "Summary" {
        theme.text.to_color()
    } else if label == "Usage" {
        theme.status_detail.to_color()
    } else {
        theme.text.to_color()
    };

    let mut style = Style::default().fg(color);
    if label == "State"
        || lower.contains("waiting")
        || lower.contains("thinking")
        || lower.contains("running tool")
        || lower.contains("ready")
        || lower.contains("generating")
        || label == "Hint"
        || (title == "PR Triage" && (lower.contains("working") || lower.contains("running")))
    {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn draw_leader_menu(
    frame: &mut Frame,
    content_area: Rect,
    theme: &Theme,
    compose_intercept: Option<bool>,
    next_prev_feature: (Option<char>, Option<char>),
) {
    let mut commands: Vec<(String, String)> = LEADER_COMMANDS
        .iter()
        .map(|(key, desc)| ((*key).to_string(), (*desc).to_string()))
        .collect();
    if let Some(active) = compose_intercept {
        let entry = if active {
            ("e".to_string(), "Direct input (compose off)".to_string())
        } else {
            ("e".to_string(), "Enable compose input".to_string())
        };
        let pos = commands
            .iter()
            .position(|(key, _)| key == "s")
            .map(|pos| pos + 1)
            .unwrap_or(commands.len());
        commands.insert(pos, entry);
    }

    // Next/prev feature has no default binding; only advertise it when the
    // user has configured a key for it.
    let (next_key, prev_key) = next_prev_feature;
    let feature_entry = match (next_key, prev_key) {
        (Some(n), Some(p)) => Some((format!("{n} / {p}"), "Next / prev feature".to_string())),
        (Some(n), None) => Some((n.to_string(), "Next feature".to_string())),
        (None, Some(p)) => Some((p.to_string(), "Prev feature".to_string())),
        (None, None) => None,
    };
    if let Some(entry) = feature_entry {
        let pos = commands
            .iter()
            .position(|(key, _)| key == "w")
            .map(|pos| pos + 1)
            .unwrap_or(commands.len());
        commands.insert(pos, entry);
    }

    let borrowed: Vec<(&str, &str)> = commands
        .iter()
        .map(|(key, desc)| (key.as_str(), desc.as_str()))
        .collect();

    draw_command_menu(
        frame,
        content_area,
        " Ctrl+Space commands ",
        borrowed.as_slice(),
        theme,
    );
}

pub(crate) fn draw_command_menu(
    frame: &mut Frame,
    content_area: Rect,
    title: &str,
    commands: &[(&str, &str)],
    theme: &Theme,
) {
    if content_area.width < 30 || content_area.height < 8 {
        return;
    }

    let longest_label = commands
        .iter()
        .map(|(key, desc)| key.len() + desc.len() + 4)
        .max()
        .unwrap_or(24) as u16;
    let width = (longest_label + 4).clamp(30, content_area.width.saturating_sub(2));
    let height = (commands.len() as u16 + 2).min(content_area.height.saturating_sub(1));
    let x = content_area.x + content_area.width.saturating_sub(width + 1);
    let y = content_area.y + content_area.height.saturating_sub(height + 1);
    let area = Rect::new(x, y, width, height);

    let lines: Vec<Line<'static>> = commands
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
                Span::styled(desc.to_string(), Style::default().fg(theme.text.to_color())),
            ])
        })
        .collect();

    let popup = Paragraph::new(lines).block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .style(Style::default().bg(theme.effective_bg()))
            .border_style(Style::default().fg(theme.info.to_color())),
    );

    frame.render_widget(Clear, area);
    frame.render_widget(popup, area);
}

fn scroll_content_to_lines_with_selection(
    content: &str,
    offset: usize,
    rows: u16,
    cols: u16,
    selection: &TextSelection,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let all_lines: Vec<&str> = content.lines().collect();
    let total_lines = all_lines.len();
    let start = offset.min(total_lines);
    let end = (start + rows as usize).min(total_lines);
    let (sel_start_row, sel_start_col, sel_end_row, sel_end_col) = selection.normalized();
    let sel_start_row = sel_start_row as usize;
    let sel_start_col = sel_start_col as usize;
    let sel_end_row = sel_end_row as usize;
    let sel_end_col = sel_end_col as usize;
    let has_selection = selection.has_selection;

    let mut lines = Vec::with_capacity(rows as usize);
    for (visible_row, i) in (start..end).enumerate() {
        let content_row = start + visible_row;
        let line_text = all_lines.get(i).unwrap_or(&"");
        let is_selected_row = has_selection && (sel_start_row..=sel_end_row).contains(&content_row);

        lines.push(render_ansi_line_with_selection(
            line_text,
            cols,
            content_row,
            is_selected_row,
            sel_start_col,
            sel_end_col,
            sel_start_row,
            sel_end_row,
            theme,
        ));
    }
    while lines.len() < rows as usize {
        lines.push(Line::raw(""));
    }
    lines
}

// Selection geometry travels as scalars from the mouse handler; see
// TextSelection::normalized().
#[allow(clippy::too_many_arguments)]
fn render_ansi_line_with_selection(
    line_text: &str,
    cols: u16,
    content_row: usize,
    is_selected_row: bool,
    sel_start_col: usize,
    sel_end_col: usize,
    sel_start_row: usize,
    sel_end_row: usize,
    theme: &Theme,
) -> Line<'static> {
    let measured_width = unicode_width::UnicodeWidthStr::width(line_text);
    let parser_cols = cols
        .max(2)
        .max(measured_width.saturating_add(2).min(u16::MAX as usize) as u16);
    let mut parser = vt100::Parser::new(1, parser_cols, 0);
    let normalized = line_text.replace('\n', "\r\n");
    parser.process(normalized.as_bytes());
    let screen = parser.screen();

    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut current_text = String::new();
    let mut current_style = Style::default();

    let line_len = cols as usize;
    let start_col = if is_selected_row && content_row == sel_start_row {
        sel_start_col.min(line_len)
    } else {
        0
    };
    let end_col = if is_selected_row && content_row == sel_end_row {
        sel_end_col.min(line_len)
    } else {
        line_len
    };

    for col in 0..cols {
        let Some(cell) = screen.cell(0, col) else {
            continue;
        };

        let mut style = vt100_cell_to_style(cell);
        let col = col as usize;
        if is_selected_row && col >= start_col && col < end_col {
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

    if spans.is_empty() {
        Line::raw("")
    } else {
        Line::from(spans)
    }
}

pub(crate) fn render_vt100_screen(
    screen: &vt100::Screen,
    cols: u16,
    rows: u16,
) -> Vec<Line<'static>> {
    let mut lines = Vec::with_capacity(rows as usize);
    for row in 0..rows {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut current_text = String::new();
        let mut current_style = Style::default();

        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
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

/// Normalize captured tmux pane text for vt100 parsing. capture-pane
/// output terminates the last line with a newline; processing that
/// final newline in a parser whose height equals the pane scrolls the
/// whole screen up one row, so strip it before converting line
/// endings.
pub(crate) fn normalize_captured_pane(raw: &str) -> String {
    let raw = raw.strip_suffix('\n').unwrap_or(raw);
    raw.replace('\n', "\r\n")
}

pub(crate) fn render_ansi_lines(raw: &str, cols: u16, rows: u16) -> Vec<Line<'static>> {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    let normalized = normalize_captured_pane(raw);
    parser.process(normalized.as_bytes());
    render_vt100_screen(parser.screen(), cols, rows)
}

fn ansi_to_ratatui_text_with_selection<'a>(
    raw: &str,
    cols: u16,
    rows: u16,
    selection: &TextSelection,
    theme: &Theme,
) -> Vec<Line<'a>> {
    let mut parser = vt100::Parser::new(rows, cols, 0);
    let normalized = normalize_captured_pane(raw);
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
        let (window, label) = match session_kind {
            crate::project::SessionKind::Claude => ("claude", "Claude"),
            crate::project::SessionKind::Codex => ("codex", "Codex"),
            crate::project::SessionKind::Opencode => ("opencode", "Opencode"),
            crate::project::SessionKind::Terminal => ("terminal", "Terminal"),
            crate::project::SessionKind::Nvim => ("nvim", "Nvim"),
            crate::project::SessionKind::Vscode => ("vscode", "VSCode"),
            crate::project::SessionKind::Pi => ("pi", "Pi"),
            crate::project::SessionKind::Custom => ("custom", "Custom"),
            crate::project::SessionKind::Todos => ("todos", "TODOs"),
        };
        ViewState::new(
            "proj".into(),
            "feat".into(),
            "amf-feat".into(),
            window.into(),
            label.into(),
            session_kind,
            VibeMode::Vibeless,
            false,
        )
    }

    fn context_snapshot(
        percentage: u8,
        band: crate::context_tracking::ContextBand,
        provenance: crate::context_tracking::ContextProvenance,
        freshness: crate::context_tracking::ContextFreshness,
    ) -> SessionContextSnapshot {
        SessionContextSnapshot {
            used_tokens: u64::from(percentage) * 1_000,
            context_limit: std::num::NonZeroU64::new(100_000).unwrap(),
            percentage: crate::context_tracking::ContextPercentage::clamped(i64::from(percentage)),
            band,
            provenance,
            freshness,
            sampled_at: chrono::Utc::now(),
            checked_at: chrono::Utc::now(),
            reset: crate::context_tracking::ContextResetMetadata::default(),
        }
    }

    #[test]
    fn claude_sidebar_width_is_reserved_when_view_is_wide_enough() {
        let width = viewing_main_width(&sample_view(crate::project::SessionKind::Claude), 120);
        assert_eq!(width, 88);
    }

    #[test]
    fn codex_sidebar_width_is_reserved_when_view_is_wide_enough() {
        let width = viewing_main_width(&sample_view(crate::project::SessionKind::Codex), 120);
        assert_eq!(width, 88);
    }

    #[test]
    fn non_sidebar_sessions_keep_full_width() {
        let width = viewing_main_width(&sample_view(crate::project::SessionKind::Terminal), 120);
        assert_eq!(width, 120);
    }

    #[test]
    fn active_todo_section_is_hidden_when_empty_and_keeps_completed_entries() {
        let mut data = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Claude,
            status_text: String::new(),
            model_text: None,
            prompt_text: String::new(),
            work_text: None,
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: String::new(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
        };
        assert!(
            sidebar_sections(&data, 30)
                .iter()
                .all(|section| section.title != "Active TODOs")
        );

        data.active_todos_text =
            Some("Project / Feature / TODO agent\nShip it · high · project · complete".to_string());
        let active = sidebar_sections(&data, 30)
            .into_iter()
            .find(|section| section.title == "Active TODOs")
            .expect("referenced TODOs should render a dedicated section");
        assert!(active.body.ends_with("complete"));
    }

    #[test]
    fn status_section_height_is_compact_for_short_status_text() {
        assert_eq!(status_section_height("Activity: Ready", 30), 3);
        assert_eq!(
            status_section_height("Activity: Ready\nInput: 1.2K tokens", 30),
            4
        );
    }

    #[test]
    fn status_section_height_grows_for_model_line() {
        assert_eq!(
            status_section_height(
                "Activity: Ready\nInput: 1.2K tokens\nOutput: 2.4K tokens\nModel: gpt-5.5",
                30,
            ),
            6
        );
    }

    #[test]
    fn work_section_height_is_compact_for_short_work_text() {
        assert_eq!(
            sidebar_section_height("State: running tool\nTool: Bash", 30, 2, 6),
            4
        );
    }

    #[test]
    fn work_section_uses_measured_height_in_sidebar_layout() {
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Codex,
            status_text: "Thinking\nUsage: 1.2K tokens".into(),
            model_text: None,
            prompt_text: "Preview: Continue the refactor.".into(),
            work_text: Some("State: running tool\nTool: cargo test".into()),
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Codex sidebar ready.".into(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
        };

        let sections = sidebar_sections(&sidebar, 30);
        let work = sections
            .iter()
            .find(|section| section.title == "Work")
            .unwrap();

        assert!(matches!(work.constraint, Constraint::Length(4)));
    }

    #[test]
    fn plan_is_a_dedicated_sidebar_section() {
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Codex,
            status_text: "Ready".into(),
            model_text: None,
            prompt_text: String::new(),
            work_text: None,
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: String::new(),
            pr_triage_text: None,
            plan_text: "Current: docs/accepted.md".into(),
            context_snapshot: None,
            context_hint_visible: false,
        };

        let sections = sidebar_sections(&sidebar, 30);
        let plan = sections
            .iter()
            .find(|section| section.title == "Plan")
            .expect("plan child row should be present");

        assert_eq!(plan.body, "Current: docs/accepted.md");
    }

    #[test]
    fn fresh_context_section_uses_the_shared_indicator_and_action_wording() {
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Claude,
            status_text: "Ready".into(),
            model_text: None,
            prompt_text: String::new(),
            work_text: None,
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: String::new(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: Some(context_snapshot(
                70,
                crate::context_tracking::ContextBand::Warning,
                crate::context_tracking::ContextProvenance::Direct,
                crate::context_tracking::ContextFreshness::Fresh,
            )),
            context_hint_visible: true,
        };

        let sections = sidebar_sections(&sidebar, 30);
        let context = sections
            .iter()
            .find(|section| section.title == "Fresh Context")
            .expect("eligible context should have a dedicated section");

        assert_eq!(
            context.body,
            "Usage: Ctx 70% WARNING · 70,000\nAction: Fresh context: <leader F>\nDismiss: <leader X>"
        );
        assert!(matches!(
            context.constraint,
            Constraint::Length(height) if height >= 4
        ));
    }

    #[test]
    fn fresh_context_section_wraps_without_disappearing_in_a_narrow_sidebar() {
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Claude,
            status_text: String::new(),
            model_text: None,
            prompt_text: String::new(),
            work_text: None,
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: String::new(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: Some(context_snapshot(
                85,
                crate::context_tracking::ContextBand::Critical,
                crate::context_tracking::ContextProvenance::Estimated,
                crate::context_tracking::ContextFreshness::Stale,
            )),
            context_hint_visible: true,
        };

        let sections = sidebar_sections(&sidebar, 16);
        let context = sections
            .iter()
            .find(|section| section.title == "Fresh Context")
            .expect("eligible context should remain present when wrapped");

        assert!(context.body.contains("Ctx ~85% CRITICAL STALE · 85,000"));
        assert!(context.body.contains("Fresh context: <leader F>"));
        assert!(matches!(
            context.constraint,
            Constraint::Length(height) if height >= 4
        ));
    }

    #[test]
    fn fresh_context_section_keeps_room_for_dismiss_at_default_sidebar_width() {
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Claude,
            status_text: String::new(),
            model_text: None,
            prompt_text: String::new(),
            work_text: None,
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: String::new(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: Some(context_snapshot(
                70,
                crate::context_tracking::ContextBand::Warning,
                crate::context_tracking::ContextProvenance::Direct,
                crate::context_tracking::ContextFreshness::Fresh,
            )),
            context_hint_visible: true,
        };

        let section_width = 32;
        let sections = sidebar_sections(&sidebar, section_width);
        let context = sections
            .iter()
            .find(|section| section.title == "Fresh Context")
            .expect("eligible context should have a dedicated section");

        // How many inner rows the body actually needs once wrapped at this
        // width -- the section must be tall enough to show every one of them,
        // borders included, or the last line (`Dismiss`) is clipped.
        let inner_width = usize::from(section_width - 2);
        let needed_inner_lines: u16 = context
            .body
            .lines()
            .map(|line| (line.chars().count().max(1)).div_ceil(inner_width) as u16)
            .sum();
        assert!(needed_inner_lines >= 5, "body should wrap past four lines");

        let Constraint::Length(height) = context.constraint else {
            panic!("fresh context section uses a fixed height");
        };
        assert!(
            height >= needed_inner_lines + 2,
            "height {height} clips a body needing {needed_inner_lines} inner lines"
        );
    }

    #[test]
    fn work_section_height_grows_with_more_visible_lines() {
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Codex,
            status_text: "Thinking\nUsage: 1.2K tokens".into(),
            model_text: None,
            prompt_text: "Preview: Continue the refactor.".into(),
            work_text: Some(
                "State: waiting for input\nRequest: Codex finished and is waiting for review on parser wiring.\nQueue: 2 pending\nTool: cargo test codex_sidebar -- --nocapture\nFile: src/ui/pane.rs"
                    .into(),
            ),
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Codex sidebar ready.".into(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
        };

        let sections = sidebar_sections(&sidebar, 30);
        let work = sections
            .iter()
            .find(|section| section.title == "Work")
            .unwrap();

        assert!(matches!(work.constraint, Constraint::Length(height) if height > 4));
    }

    #[test]
    fn prompt_section_height_is_compact() {
        assert_eq!(prompt_section_height("Preview: Continue", 30), 3);
    }

    #[test]
    fn prompt_section_height_stays_compact_without_work() {
        assert_eq!(prompt_section_height("Preview: Continue", 30), 3);
    }

    #[test]
    fn summary_section_height_is_compact() {
        assert_eq!(summary_section_height("Short summary", 30), 3);
    }

    #[test]
    fn summary_section_height_stays_compact_without_work() {
        assert_eq!(summary_section_height("Short summary", 30), 3);
    }

    #[test]
    fn claude_sidebar_shell_renders_in_view() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Claude);
        let theme = Theme::default();
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Claude,
            status_text: "Waiting for input\nUsage: 1.2K tokens".into(),
            model_text: None,
            prompt_text: "Preview: Resume the task.".into(),
            work_text: None,
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Sidebar ready.".into(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
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
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(rendered.contains("Claude Sidebar"));
        assert!(rendered.contains("Sidebar ready."));
        assert!(rendered.contains("Waiting for input"));
        assert!(rendered.contains("Resume the task."));
        assert!(!rendered.contains("Session"));
    }

    #[test]
    fn pr_triage_section_renders_in_sidebar_when_present() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Claude);
        let theme = Theme::default();
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Claude,
            status_text: "Waiting for input".into(),
            model_text: None,
            prompt_text: "Preview: Resume the task.".into(),
            work_text: None,
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Sidebar ready.".into(),
            pr_triage_text: Some("PR: #321 · 4 open\nStatus: Working".into()),
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
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
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(rendered.contains("PR Triage"));
        assert!(rendered.contains("PR: #321"));
        assert!(rendered.contains("4 open"));
        assert!(rendered.contains("Working"));
        assert!(rendered.contains("<leader G>"));
    }

    #[test]
    fn pr_triage_section_is_absent_from_sidebar_without_an_active_pr() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Claude);
        let theme = Theme::default();
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Claude,
            status_text: "Waiting for input".into(),
            model_text: None,
            prompt_text: "Preview: Resume the task.".into(),
            work_text: None,
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Sidebar ready.".into(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
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
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(!rendered.contains("PR Triage"));
    }

    #[test]
    fn codex_sidebar_shell_renders_in_view() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Codex);
        let theme = Theme::default();
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Codex,
            status_text: "Thinking\nInput: 1.2K tokens".into(),
            model_text: None,
            prompt_text: "Preview: Continue the refactor.".into(),
            work_text: None,
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Codex sidebar ready.".into(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
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
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(rendered.contains("Codex Sidebar"));
        assert!(rendered.contains("Codex sidebar ready."));
        assert!(rendered.contains("Continue the"));
    }

    #[test]
    fn codex_sidebar_renders_work_section_when_present() {
        let backend = TestBackend::new(120, 34);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Codex);
        let theme = Theme::default();
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Codex,
            status_text: "Thinking\nUsage: 1.2K tokens".into(),
            model_text: None,
            prompt_text: "Preview: Continue the refactor.".into(),
            work_text: Some("State: running tool\nTool: cargo test".into()),
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Codex sidebar ready.".into(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
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
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(rendered.contains("Work"));
        assert!(rendered.contains("cargo test"));
    }

    #[test]
    fn leader_menu_lists_sidebar_toggle_command() {
        let backend = TestBackend::new(120, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Claude);
        let theme = Theme::default();

        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &view,
                    "hello",
                    None,
                    true,
                    0,
                    None,
                    Some(false),
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(rendered.contains("Ctrl+Space commands"));
        assert!(rendered.contains("Show / hide sidebar"));
        assert!(rendered.contains("Bookmark picker"));
        assert!(rendered.contains("Open current plan"));
        assert!(rendered.contains("Fresh context"));
        assert!(rendered.contains("Jump to bookmark slot"));
        assert!(rendered.contains("Check pending diff revie"));
        // compose_intercept is Some(false): the menu offers the way
        // back out of direct mode.
        assert!(rendered.contains("Enable compose input"));
        // Next/prev feature is unbound by default, so it is hidden.
        assert!(!rendered.contains("Next / prev feature"));
        assert!(!rendered.contains("Next feature"));
    }

    #[test]
    fn leader_menu_shows_next_prev_feature_only_when_bound() {
        let theme = Theme::default();
        let view = sample_view(crate::project::SessionKind::Claude);

        let render = |next_prev: (Option<char>, Option<char>)| {
            let backend = TestBackend::new(120, 24);
            let mut terminal = Terminal::new(backend).unwrap();
            terminal
                .draw(|frame| {
                    draw(
                        frame, &view, "hello", None, true, 0, None, None, next_prev, &theme,
                    );
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            buffer
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>()
        };

        // Both bound: a combined row using the configured keys.
        let both = render((Some('n'), Some('p')));
        assert!(both.contains("Next / prev feature"));
        assert!(both.contains("n / p"));

        // Only next bound: single row.
        let next_only = render((Some('j'), None));
        assert!(next_only.contains("Next feature"));
        assert!(!next_only.contains("Next / prev feature"));

        // Neither bound: hidden entirely.
        let none = render((None, None));
        assert!(!none.contains("Next feature"));
        assert!(!none.contains("Next / prev feature"));
    }

    #[test]
    fn codex_sidebar_places_work_above_prompt_when_present() {
        let backend = TestBackend::new(120, 34);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Codex);
        let theme = Theme::default();
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Codex,
            status_text: "Thinking\nUsage: 1.2K tokens".into(),
            model_text: None,
            prompt_text: "Preview: Continue the refactor.".into(),
            work_text: Some("State: running tool\nTool: cargo test".into()),
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Codex sidebar ready.".into(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
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
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let work_index = rendered.find("Work").unwrap();
        let prompt_index = rendered.find("Prompt").unwrap();

        assert!(work_index < prompt_index);
    }
    #[test]
    fn codex_sidebar_places_summary_above_prompt_when_present() {
        let backend = TestBackend::new(120, 34);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Codex);
        let theme = Theme::default();
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Codex,
            status_text: "Thinking\nUsage: 1.2K tokens".into(),
            model_text: None,
            prompt_text: "Preview: Continue the refactor.".into(),
            work_text: Some("State: running tool\nTool: cargo test".into()),
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Codex sidebar ready.".into(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
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
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        let summary_index = rendered.find("Summary").unwrap();
        let prompt_index = rendered.find("Prompt").unwrap();

        assert!(summary_index < prompt_index);
    }

    #[test]
    fn codex_sidebar_lets_work_expand_and_keeps_summary_compact() {
        let backend = TestBackend::new(120, 38);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Codex);
        let theme = Theme::default();
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Codex,
            status_text: "Thinking\nUsage: 1.2K tokens".into(),
            model_text: None,
            prompt_text: "Preview: Continue the refactor.".into(),
            work_text: Some(
                "State: waiting for input\nRequest: Codex finished and is waiting for review on parser wiring.\nQueue: 2 pending\nTool: cargo test codex_sidebar -- --nocapture\nFile: src/ui/pane.rs"
                    .into(),
            ),
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Small summary.".into(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
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
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(rendered.contains("Queue"));
        assert!(rendered.contains("cargo test"));
        assert!(rendered.contains("Small summary."));
    }

    #[test]
    fn codex_sidebar_skips_empty_status_section() {
        let backend = TestBackend::new(120, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Codex);
        let theme = Theme::default();
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Codex,
            status_text: String::new(),
            model_text: None,
            prompt_text: "Preview: Continue the refactor.".into(),
            work_text: Some("State: waiting for input\nRequest: Need approval.".into()),
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Codex sidebar ready.".into(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
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
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(!rendered.contains("Status"));
        assert!(rendered.contains("Prompt"));
        assert!(rendered.contains("Work"));
    }

    #[test]
    fn codex_sidebar_skips_empty_summary_section() {
        let backend = TestBackend::new(120, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Codex);
        let theme = Theme::default();
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Codex,
            status_text: "Input: 1.2K tokens".into(),
            model_text: None,
            prompt_text: "Preview: Continue the refactor.".into(),
            work_text: Some("State: waiting for input\nRequest: Need approval.".into()),
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: String::new(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
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
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(!rendered.contains("Summary"));
        assert!(rendered.contains("Prompt"));
        assert!(rendered.contains("Work"));
    }

    #[test]
    fn codex_sidebar_skips_empty_prompt_section() {
        let backend = TestBackend::new(120, 28);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Codex);
        let theme = Theme::default();
        let sidebar = AgentSidebarData {
            agent_kind: crate::project::SessionKind::Codex,
            status_text: "Input: 1.2K tokens".into(),
            model_text: None,
            prompt_text: String::new(),
            work_text: Some("State: waiting for input\nRequest: Need approval.".into()),
            todos_text: None,
            active_todos_text: None,
            active_todo_affordance: false,
            summary_text: "Codex sidebar ready.".into(),
            pr_triage_text: None,
            plan_text: String::new(),
            context_snapshot: None,
            context_hint_visible: false,
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
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(!rendered.contains("Prompt"));
        assert!(rendered.contains("Status"));
        assert!(rendered.contains("Work"));
    }

    #[test]
    fn codex_sidebar_without_data_keeps_codex_title_and_skips_placeholder_sections() {
        let backend = TestBackend::new(120, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let view = sample_view(crate::project::SessionKind::Codex);
        let theme = Theme::default();

        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &view,
                    "hello",
                    None,
                    false,
                    0,
                    None,
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();

        assert!(rendered.contains("Codex Sidebar"));
        assert!(!rendered.contains("Claude Sidebar"));
        assert!(!rendered.contains("Status"));
        assert!(!rendered.contains("Prompt"));
        assert!(!rendered.contains("Summary"));
    }

    #[test]
    fn scroll_selection_highlights_visible_slice() {
        let theme = Theme::default();
        let selection = TextSelection {
            start_row: 2,
            start_col: 1,
            end_row: 2,
            end_col: 3,
            has_selection: true,
            ..TextSelection::default()
        };

        let lines = scroll_content_to_lines_with_selection(
            "zero\none\ntwo\nthree",
            1,
            3,
            8,
            &selection,
            &theme,
        );

        assert_eq!(lines.len(), 3);
        let line = &lines[1];
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "two");
        assert!(
            line.spans
                .iter()
                .any(|span| span.style.bg == Some(theme.effective_selection_bg()))
        );
    }

    #[test]
    fn scroll_mode_renders_scrollbar_arrows() {
        let backend = TestBackend::new(20, 6);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut view = sample_view(crate::project::SessionKind::Terminal);
        view.scroll_mode = true;
        view.scroll_content = "one\ntwo\nthree\nfour\nfive".into();
        view.scroll_total_lines = 5;
        view.scroll_offset = 1;
        let theme = Theme::default();

        terminal
            .draw(|frame| {
                draw(
                    frame,
                    &view,
                    "",
                    None,
                    false,
                    0,
                    None,
                    None,
                    (None, None),
                    &theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let rendered: String = buffer.content().iter().map(|cell| cell.symbol()).collect();
        assert!(rendered.contains("↑"));
        assert!(rendered.contains("↓"));
    }

    #[test]
    fn scroll_renderer_handles_narrow_content_without_panicking() {
        let theme = Theme::default();
        let selection = TextSelection::default();
        let lines = scroll_content_to_lines_with_selection("😊", 0, 1, 1, &selection, &theme);

        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn full_pane_capture_with_trailing_newline_does_not_scroll() {
        // capture-pane terminates the last line with '\n'; when the
        // capture fills the parser exactly, that newline must not
        // scroll the screen (it would shift content one row above the
        // separately-reported cursor position).
        let lines = render_ansi_lines("top\nmid\nbottom\n", 10, 3);

        assert_eq!(lines.len(), 3);
        let row_text =
            |line: &Line<'_>| -> String { line.spans.iter().map(|s| s.content.clone()).collect() };
        assert_eq!(row_text(&lines[0]), "top");
        assert_eq!(row_text(&lines[1]), "mid");
        assert_eq!(row_text(&lines[2]), "bottom");
    }

    #[test]
    fn scroll_renderer_handles_long_colored_lines_without_panicking() {
        let theme = Theme::default();
        let selection = TextSelection::default();
        let lines = scroll_content_to_lines_with_selection(
            "\u{1b}[31mthis is a very long colored line that should not wrap or panic when rendered in scroll mode\u{1b}[0m",
            0,
            1,
            8,
            &selection,
            &theme,
        );

        assert_eq!(lines.len(), 1);
    }
}
