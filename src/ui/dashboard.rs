use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, Paragraph},
};

use crate::app::util::{ClaudeTaskState, read_claude_task_state};
use crate::app::{App, AppMode, CreateFeatureStep, RenameReturnTo};
use crate::project::{
    Feature, FeatureSession, Project, SessionKind, TokenUsageSourceMatch, VibeMode,
};
use crate::token_tracking::{TokenUsageProvider, TokenUsageSource};

const SIDEBAR_PROMPT_PREVIEW_COLS: usize = 32;
const SIDEBAR_PROMPT_PREVIEW_LINES: usize = 2;
const SIDEBAR_SUMMARY_PREVIEW_COLS: usize = 32;
const SIDEBAR_SUMMARY_PREVIEW_LINES: usize = 3;
const SIDEBAR_WORK_VALUE_CHARS: usize = 28;
const DASHBOARD_LEADER_COMMANDS: &[(&str, &str)] = &[
    ("i", "Pending inputs"),
    ("?", "Help"),
    ("/", "Command picker"),
    ("a", "Local commands"),
    ("A", "Manage harnesses"),
    ("c", "Config wizard"),
    ("h", "Bookmarks"),
    ("H", "Bookmark current session"),
    ("M", "Remove bookmark"),
    ("1-9", "Jump to bookmark"),
    ("r", "Refresh statuses"),
];

/// The ambient `[PR #N · M open]` badge span shown in the top-right corner
/// of a live session — shared by both `AppMode::Viewing` and
/// `AppMode::Compose` (composing is just Viewing plus an input overlay; the
/// session underneath, and its PR, don't change). Shown as a sidebar box
/// instead (`pr_triage_sidebar_text`) once the sidebar is visible, so the
/// two never compete for the same header space.
fn pr_triage_badge_span(app: &App, view: &crate::app::ViewState) -> Option<Span<'static>> {
    if view.sidebar_visible {
        return None;
    }
    let feature = app.feature_for_view(view)?;
    let pr = app.active_pr_for_feature(&feature.id)?;
    let working = app
        .dedicated_review_session_working_for_workdir(&feature.workdir)
        .unwrap_or(false);
    let ai_review_running = app.ai_review_running_for_workdir(&feature.workdir);
    let mut label = match pr.unresolved_threads {
        Some(0) => format!(" [PR #{} · 0 open", pr.number),
        Some(count) => format!(" [PR #{} · {} open", pr.number, count),
        None => format!(" [PR #{}", pr.number),
    };
    if working {
        label.push_str(" · ● working");
    }
    if ai_review_running {
        label.push_str(" · AI review");
    }
    label.push_str("] ");
    let color = if working || ai_review_running {
        app.theme.warning.to_color()
    } else if pr.unresolved_threads == Some(0) {
        app.theme.success.to_color()
    } else {
        app.theme.info.to_color()
    };
    Some(Span::styled(
        label,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ))
}

/// Render a right-aligned row of badge spans flush with the top of `area`.
fn draw_badge_row(frame: &mut Frame, area: Rect, badge_spans: Vec<Span<'static>>) {
    if badge_spans.is_empty() {
        return;
    }
    let total: u16 = badge_spans
        .iter()
        .map(|s| s.content.chars().count() as u16)
        .sum();
    let label_width = total.min(area.width);
    let badge_area = Rect::new(
        area.x + area.width.saturating_sub(label_width),
        area.y,
        label_width,
        1,
    );
    frame.render_widget(Paragraph::new(Line::from(badge_spans)), badge_area);
}

/// Sidebar-box counterpart of the ambient Viewing-mode PR badge (see
/// `draw()`'s `AppMode::Viewing` arm) — shown instead of the badge when the
/// sidebar is visible, so the two never compete for the same header space.
fn pr_triage_sidebar_text(app: &App, feature: &Feature) -> Option<String> {
    let pr = app.active_pr_for_feature(&feature.id)?;
    let mut lines = vec![match pr.unresolved_threads {
        Some(count) => format!("PR: #{} · {count} open", pr.number),
        None => format!("PR: #{}", pr.number),
    }];
    if app
        .dedicated_review_session_working_for_workdir(&feature.workdir)
        .unwrap_or(false)
    {
        lines.push("Status: Working".to_string());
    }
    if app.ai_review_running_for_workdir(&feature.workdir) {
        lines.push("AI review: Running".to_string());
    }
    Some(lines.join("\n"))
}

fn build_agent_sidebar_data(
    app: &App,
    view: &crate::app::ViewState,
) -> Option<super::pane::AgentSidebarData> {
    let sidebar_kind = view.sidebar_session_kind()?;

    let (project, feature) = app.store.projects.iter().find_map(|project| {
        project
            .features
            .iter()
            .find(|feature| feature.tmux_session == view.session)
            .map(|feature| (project, feature))
    })?;

    let session = feature
        .sessions
        .iter()
        .find(|session| session.tmux_window == view.window)
        .or_else(|| {
            feature
                .sessions
                .iter()
                .find(|session| session.kind == sidebar_kind)
        });

    let waiting_count = app
        .pending_inputs
        .iter()
        .filter(|input| {
            input.session_id == view.session
                || (input.project_name.as_deref() == Some(project.name.as_str())
                    && input.feature_name.as_deref() == Some(feature.name.as_str()))
        })
        .count();
    let status_line = match waiting_count {
        0 => "Ready".to_string(),
        1 => "Waiting for 1 input".to_string(),
        n => format!("Waiting for {n} inputs"),
    };

    match sidebar_kind {
        SessionKind::Opencode => {
            build_opencode_sidebar_data(app, project, feature, session, view, status_line)
        }
        SessionKind::Claude => {
            build_claude_sidebar_data(app, project, feature, session, view, status_line)
        }
        SessionKind::Codex => {
            build_codex_sidebar_data(app, project, feature, session, view, status_line)
        }
        _ => None,
    }
}

fn build_opencode_sidebar_data(
    app: &App,
    project: &Project,
    feature: &Feature,
    session: Option<&FeatureSession>,
    view: &crate::app::ViewState,
    status_line: String,
) -> Option<super::pane::AgentSidebarData> {
    let opencode_sidebar = app.opencode_sidebar_cache.get(&feature.tmux_session);
    let usage_line = session
        .and_then(|session| session.status_text.as_deref())
        .map(format_sidebar_usage)
        .filter(|line| line != "Usage: unavailable");
    let prompt_text = opencode_sidebar_prompt_text(
        opencode_sidebar
            .and_then(|sidebar| sidebar.latest_prompt.as_deref())
            .or_else(|| app.latest_prompt_for_session(&feature.tmux_session)),
    );
    let work_text = opencode_sidebar_work_text(opencode_sidebar);
    let todos_text = opencode_sidebar_todos_text(opencode_sidebar);
    let summary_text = opencode_sidebar_summary_text(
        app.summary_state.generating.contains(&feature.tmux_session),
        feature.summary.as_deref(),
        opencode_sidebar,
    );
    let model_text = app.sidebar_model_cache.get(&feature.tmux_session).cloned();
    let activity_line = if pending_diff_review_work_text(app, project, feature).is_some() {
        "Waiting for diff review".to_string()
    } else if opencode_sidebar
        .and_then(|sidebar| sidebar.pending_permission.as_ref())
        .is_some()
    {
        "Waiting on permission".to_string()
    } else if app.ipc_tool_sessions.contains(&feature.tmux_session) {
        "Running tool".to_string()
    } else if app.is_feature_thinking(&feature.tmux_session) {
        "Thinking".to_string()
    } else {
        status_line
    };

    Some(super::pane::AgentSidebarData {
        agent_kind: SessionKind::Opencode,
        status_text: append_model_status_line(
            opencode_sidebar_status_text(activity_line, usage_line, opencode_sidebar),
            model_text.as_deref(),
        ),
        model_text,
        prompt_text,
        work_text: pending_diff_review_work_text(app, project, feature)
            .or(work_text)
            .or_else(|| fallback_sidebar_work_text(app, project, feature, view)),
        todos_text,
        summary_text,
        pr_triage_text: pr_triage_sidebar_text(app, feature),
    })
}

fn build_claude_sidebar_data(
    app: &App,
    project: &Project,
    feature: &Feature,
    session: Option<&FeatureSession>,
    view: &crate::app::ViewState,
    status_line: String,
) -> Option<super::pane::AgentSidebarData> {
    let usage_line = session
        .and_then(|session| session.status_text.as_deref())
        .map(format_sidebar_usage);
    let prompt_text =
        sidebar_prompt_text(None, app.latest_prompt_for_session(&feature.tmux_session));
    let summary_text = if app.summary_state.generating.contains(&feature.tmux_session) {
        Some("Generating summary...".to_string())
    } else {
        feature.summary.clone()
    };
    let work_text = pending_diff_review_work_text(app, project, feature)
        .or_else(|| fallback_sidebar_work_text(app, project, feature, view));
    let summary_text = compose_sidebar_summary_text(None, summary_text);
    let activity_line = sidebar_status_activity_text(work_text.is_some(), status_line);
    let model_text = app.sidebar_model_cache.get(&feature.tmux_session).cloned();
    let status_text = append_model_status_line(
        compose_sidebar_status_text(activity_line, usage_line, None),
        model_text.as_deref(),
    );
    let claude_session_id = session.and_then(|s| s.claude_session_id.as_deref());
    let todos_text = read_claude_task_state(&feature.workdir, claude_session_id)
        .as_ref()
        .and_then(claude_sidebar_todos_text);

    Some(super::pane::AgentSidebarData {
        agent_kind: SessionKind::Claude,
        status_text,
        model_text,
        prompt_text,
        work_text,
        todos_text,
        summary_text,
        pr_triage_text: pr_triage_sidebar_text(app, feature),
    })
}

fn build_codex_sidebar_data(
    app: &App,
    project: &Project,
    feature: &Feature,
    session: Option<&FeatureSession>,
    view: &crate::app::ViewState,
    status_line: String,
) -> Option<super::pane::AgentSidebarData> {
    let usage_line = session
        .and_then(|session| session.status_text.as_deref())
        .map(format_sidebar_usage);
    let prompt_text = sidebar_prompt_text(
        codex_sidebar_source(&SessionKind::Codex, session)
            .and_then(|source| app.cached_codex_session_prompt(&feature.workdir, &source.id)),
        app.latest_prompt_for_session(&feature.tmux_session),
    );
    let summary_text = if app.summary_state.generating.contains(&feature.tmux_session) {
        Some("Generating summary...".to_string())
    } else {
        feature.summary.clone()
    };
    let codex_live = app.codex_live_thread(&feature.tmux_session);
    let work_text = pending_diff_review_work_text(app, project, feature)
        .or_else(|| codex_live.and_then(|live| live.sidebar_work_text()))
        .or_else(|| fallback_sidebar_work_text(app, project, feature, view));
    let summary_text = compose_sidebar_summary_text(
        codex_live.and_then(|live| live.summary_prefix()),
        summary_text,
    );
    let activity_line = sidebar_status_activity_text(work_text.is_some(), status_line);
    let usage_confidence = format_codex_usage_source_confidence(&SessionKind::Codex, session);
    let model_text = codex_sidebar_source(&SessionKind::Codex, session)
        .and_then(|source| app.cached_codex_session_model(&feature.workdir, &source.id))
        .map(ToOwned::to_owned)
        .or_else(|| app.sidebar_model_cache.get(&feature.tmux_session).cloned())
        .or_else(codex_configured_model_text);
    let status_text = append_model_status_line(
        compose_sidebar_status_text(activity_line, usage_line, usage_confidence),
        model_text.as_deref(),
    );

    Some(super::pane::AgentSidebarData {
        agent_kind: SessionKind::Codex,
        status_text,
        model_text,
        prompt_text,
        work_text,
        todos_text: None,
        summary_text,
        pr_triage_text: pr_triage_sidebar_text(app, feature),
    })
}

fn append_model_status_line(mut status_text: String, model_text: Option<&str>) -> String {
    let Some(model_text) = model_text.map(str::trim).filter(|line| !line.is_empty()) else {
        return status_text;
    };
    if !status_text.is_empty() {
        status_text.push('\n');
    }
    status_text.push_str(model_text);
    status_text
}

fn codex_configured_model_text() -> Option<String> {
    crate::codex_config::configured_model()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .map(|model| format!("Model: {model}"))
}

fn opencode_sidebar_status_text(
    activity_line: String,
    usage_line: Option<String>,
    opencode_sidebar: Option<&crate::app::opencode_storage::OpencodeSidebarData>,
) -> String {
    let mut lines = vec![format!("Activity: {activity_line}")];
    if let Some(usage_line) = usage_line {
        lines.push(usage_line);
    }
    if let Some(reasoning_tokens) = opencode_sidebar
        .and_then(|sidebar| sidebar.reasoning_tokens)
        .filter(|tokens| *tokens > 0)
    {
        lines.push(format!(
            "Reasoning: {}",
            crate::token_tracking::format_token_count(reasoning_tokens)
        ));
    }
    if let Some(change_line) = opencode_sidebar.and_then(|sidebar| sidebar.change_summary_line()) {
        lines.push(change_line);
    }
    lines.join("\n")
}

fn opencode_sidebar_work_text(
    opencode_sidebar: Option<&crate::app::opencode_storage::OpencodeSidebarData>,
) -> Option<String> {
    let mut lines = Vec::new();
    if let Some(status) = opencode_sidebar
        .and_then(|sidebar| sidebar.status.as_deref())
        .filter(|status| !status.is_empty())
    {
        lines.push(format!("State: {status}"));
    }
    if let Some(tool) = opencode_sidebar
        .and_then(|sidebar| sidebar.last_tool.as_deref())
        .filter(|tool| !tool.is_empty())
    {
        lines.push(format!("Tool: {tool}"));
    }
    if let Some(permission) = opencode_sidebar
        .and_then(|sidebar| sidebar.pending_permission.as_deref())
        .filter(|permission| !permission.is_empty())
    {
        lines.push(format!(
            "Permission: {}",
            compact_sidebar_text(permission, SIDEBAR_WORK_VALUE_CHARS)
        ));
    }
    if let Some(lsp_summary) = opencode_sidebar
        .and_then(|sidebar| sidebar.lsp_summary.as_deref())
        .filter(|summary| !summary.is_empty())
    {
        lines.push(format!(
            "LSP: {}",
            compact_sidebar_text(lsp_summary, SIDEBAR_WORK_VALUE_CHARS)
        ));
    }
    if let Some(error) = opencode_sidebar
        .and_then(|sidebar| sidebar.last_error.as_deref())
        .filter(|error| !error.is_empty())
    {
        lines.push(format!(
            "Error: {}",
            compact_sidebar_text(error, SIDEBAR_WORK_VALUE_CHARS)
        ));
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn opencode_sidebar_todos_text(
    opencode_sidebar: Option<&crate::app::opencode_storage::OpencodeSidebarData>,
) -> Option<String> {
    let sidebar = opencode_sidebar?;
    let todo_count = sidebar
        .todo_count
        .unwrap_or(sidebar.todo_preview.len() as u64) as usize;
    if todo_count == 0 && sidebar.todo_preview.is_empty() {
        return None;
    }

    // Opencode only provides open items — no completed count — so skip the
    // progress bar and render all preview items as pending (○).
    const MAX_SHOWN: usize = 5;
    let mut lines: Vec<String> = sidebar
        .todo_preview
        .iter()
        .take(MAX_SHOWN)
        .map(|item| format!("○ {item}"))
        .collect();

    let remaining = todo_count.saturating_sub(lines.len());
    if remaining > 0 {
        lines.push(format!("+{remaining} more"));
    }

    Some(lines.join("\n"))
}

fn claude_sidebar_todos_text(task_state: &ClaudeTaskState) -> Option<String> {
    let total = task_state.tasks.len();
    if total == 0 {
        return None;
    }
    let completed = task_state.completed_count();

    // Progress bar: 8 filled/empty blocks + " X/Y"
    let bar_width = 8usize;
    let filled = (completed * bar_width).checked_div(total).unwrap_or(0);
    let empty = bar_width - filled;
    let mut lines = vec![format!(
        "{}{} {completed}/{total}",
        "█".repeat(filled),
        "░".repeat(empty),
    )];

    // Sliding window: show up to 5 tasks anchored at the first non-completed
    // task, with one completed item above it for context.
    let (window_start, window_end) = if total <= 5 {
        (0, total)
    } else {
        let first_active = task_state
            .tasks
            .iter()
            .position(|t| t.status != "completed")
            .unwrap_or(total);
        let start = first_active.saturating_sub(1);
        let end = (start + 5).min(total);
        // Re-anchor so we always fill the window when near the end.
        let start = end.saturating_sub(5);
        (start, end)
    };

    for task in &task_state.tasks[window_start..window_end] {
        let label = task.active_form.as_deref().unwrap_or(task.subject.as_str());
        let prefix = match task.status.as_str() {
            "completed" => "✓",
            "in_progress" => "●",
            _ => "○",
        };
        lines.push(format!("{prefix} {label}"));
    }

    let remaining = total - window_end;
    if remaining > 0 {
        lines.push(format!("+{remaining} more"));
    }

    Some(lines.join("\n"))
}

fn opencode_sidebar_summary_text(
    generating: bool,
    feature_summary: Option<&str>,
    opencode_sidebar: Option<&crate::app::opencode_storage::OpencodeSidebarData>,
) -> String {
    if generating {
        return "Generating summary...".to_string();
    }

    if let Some(summary) = opencode_sidebar
        .and_then(|sidebar| sidebar.live_summary.as_deref())
        .filter(|summary| !summary.is_empty())
    {
        return compact_sidebar_block(
            summary,
            SIDEBAR_SUMMARY_PREVIEW_COLS,
            SIDEBAR_SUMMARY_PREVIEW_LINES,
        );
    }

    feature_summary
        .map(|summary| {
            compact_sidebar_block(
                summary,
                SIDEBAR_SUMMARY_PREVIEW_COLS,
                SIDEBAR_SUMMARY_PREVIEW_LINES,
            )
        })
        .unwrap_or_default()
}

fn opencode_sidebar_prompt_text(prompt: Option<&str>) -> String {
    prompt
        .map(|prompt| {
            compact_sidebar_block(
                prompt,
                SIDEBAR_PROMPT_PREVIEW_COLS,
                SIDEBAR_PROMPT_PREVIEW_LINES,
            )
        })
        .unwrap_or_default()
}

fn sidebar_status_activity_text(has_work_text: bool, idle_text: String) -> Option<String> {
    if has_work_text { None } else { Some(idle_text) }
}

fn compose_sidebar_status_text(
    activity_line: Option<String>,
    usage_line: Option<String>,
    usage_confidence: Option<String>,
) -> String {
    let mut status_lines = Vec::new();
    if let Some(activity) = activity_line {
        status_lines.push(format!("Activity: {activity}"));
    }
    if let Some(usage_line) = usage_line {
        status_lines.push(usage_line);
    }
    if let Some(confidence) = usage_confidence {
        status_lines.push(confidence);
    }
    status_lines.join("\n")
}

fn compose_sidebar_summary_text(
    reasoning_text: Option<String>,
    summary_text: Option<String>,
) -> String {
    match (reasoning_text, summary_text) {
        (Some(reasoning), Some(summary)) => compact_sidebar_text(
            &format!(
                "Reasoning: {}\n\n{}",
                compact_sidebar_text(&reasoning, 160),
                summary
            ),
            80,
        ),
        (Some(reasoning), None) => compact_sidebar_text(
            &format!("Reasoning: {}", compact_sidebar_text(&reasoning, 160)),
            80,
        ),
        (None, Some(summary)) => compact_sidebar_text(&summary, 80),
        (None, None) => String::new(),
    }
}

fn codex_sidebar_source<'a>(
    sidebar_kind: &SessionKind,
    session: Option<&'a FeatureSession>,
) -> Option<&'a TokenUsageSource> {
    if *sidebar_kind != SessionKind::Codex {
        return None;
    }

    session
        .and_then(|session| session.token_usage_source.as_ref())
        .filter(|source| source.provider == TokenUsageProvider::Codex)
}

fn format_codex_usage_source_confidence(
    sidebar_kind: &SessionKind,
    session: Option<&FeatureSession>,
) -> Option<String> {
    if *sidebar_kind != SessionKind::Codex {
        return None;
    }

    let match_kind = session?.token_usage_source_match.as_ref()?;
    match match_kind {
        TokenUsageSourceMatch::Exact => None,
        TokenUsageSourceMatch::Inferred => Some("Usage source: inferred workdir match".to_string()),
    }
}

fn sidebar_prompt_text(session_prompt: Option<&str>, fallback_prompt: Option<&str>) -> String {
    let prompt = select_sidebar_prompt(session_prompt, fallback_prompt);
    prompt
        .map(|prompt| compact_sidebar_text(&prompt, 48))
        .unwrap_or_default()
}

fn select_sidebar_prompt(
    session_prompt: Option<&str>,
    fallback_prompt: Option<&str>,
) -> Option<String> {
    session_prompt
        .map(ToOwned::to_owned)
        .or_else(|| fallback_prompt.map(ToOwned::to_owned))
}

fn compact_sidebar_text(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }

    let truncated: String = compact.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}

fn compact_sidebar_block(text: &str, max_cols: usize, max_lines: usize) -> String {
    if max_cols == 0 || max_lines == 0 {
        return String::new();
    }

    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        return String::new();
    }

    let words: Vec<&str> = compact.split(' ').collect();
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut index = 0;

    while index < words.len() && lines.len() < max_lines {
        let word = words[index];
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };

        if candidate.chars().count() <= max_cols {
            current = candidate;
            index += 1;
            continue;
        }

        if current.is_empty() {
            lines.push(compact_sidebar_text(word, max_cols));
            index += 1;
        } else {
            lines.push(current);
            current = String::new();
        }
    }

    if lines.len() < max_lines && !current.is_empty() {
        lines.push(current);
    }

    if index < words.len()
        && let Some(last) = lines.pop()
    {
        let trimmed = compact_sidebar_text(&last, max_cols.saturating_sub(1));
        lines.push(format!("{trimmed}…"));
    }

    lines.join("\n")
}

fn fallback_sidebar_work_text(
    app: &App,
    project: &Project,
    feature: &Feature,
    view: &crate::app::ViewState,
) -> Option<String> {
    let matching_inputs = app
        .pending_inputs
        .iter()
        .filter(|input| {
            input.session_id == view.session
                || (input.project_name.as_deref() == Some(project.name.as_str())
                    && input.feature_name.as_deref() == Some(feature.name.as_str()))
        })
        .collect::<Vec<_>>();

    if let Some(first) = matching_inputs.first() {
        let message = first.message.trim();
        let mut text = format!(
            "State: waiting for input\nRequest: {}",
            if message.is_empty() {
                "Harness is waiting for input"
            } else {
                message
            }
        );
        if matching_inputs.len() > 1 {
            text.push_str(&format!("\nQueue: {} pending", matching_inputs.len()));
        }
        return Some(text);
    }

    if app.ipc_tool_sessions.contains(&feature.tmux_session) {
        return Some("State: running tool".to_string());
    }

    if app.is_feature_thinking(&feature.tmux_session) {
        return Some("State: thinking".to_string());
    }

    None
}

fn pending_diff_review_work_text(
    app: &App,
    project: &Project,
    feature: &Feature,
) -> Option<String> {
    let matching_inputs = app
        .pending_inputs
        .iter()
        .filter(|input| {
            matches!(
                input.notification_type.as_str(),
                "diff-review" | "change-reason"
            ) && (input.session_id == feature.tmux_session
                || (input.project_name.as_deref() == Some(project.name.as_str())
                    && input.feature_name.as_deref() == Some(feature.name.as_str())))
        })
        .collect::<Vec<_>>();

    let first = matching_inputs.first()?;
    let message = first.message.trim();
    let (state, default_request) = match first.notification_type.as_str() {
        "change-reason" => (
            "waiting for change reason",
            "Explain why this change is needed.",
        ),
        _ => (
            "waiting for diff review",
            "Review the proposed change before continuing.",
        ),
    };
    let mut text = format!(
        "State: {state}\nRequest: {}",
        if message.is_empty() {
            default_request
        } else {
            message
        }
    );
    if matching_inputs.len() > 1 {
        text.push_str(&format!("\nQueue: {} pending", matching_inputs.len()));
    }
    text.push_str("\nHint: use leader V if the review prompt is not appearing.");
    Some(text)
}

fn format_sidebar_usage(status: &str) -> String {
    let mut input = None;
    let mut output = None;
    let mut effective = None;
    let mut cost = None;

    for part in status
        .split(" · ")
        .map(str::trim)
        .filter(|part| !part.is_empty())
    {
        if let Some(value) = part.strip_suffix(" in") {
            input = Some(value.to_string());
        } else if let Some(value) = part.strip_suffix(" out") {
            output = Some(value.to_string());
        } else if let Some(value) = part.strip_suffix(" eff") {
            effective = Some(value.to_string());
        } else if part.starts_with('$') || part.starts_with("<$") {
            cost = Some(part.to_string());
        }
    }

    let mut lines = Vec::new();
    if let Some(value) = input {
        lines.push(format!("Input: {value} tokens"));
    }
    if let Some(value) = output {
        lines.push(format!("Output: {value} tokens"));
    }
    if let Some(value) = effective {
        lines.push(format!("Effective: {value} tokens"));
    }
    if let Some(cost_value) = cost {
        lines.push(format!("Cost: {cost_value}"));
    }

    if lines.is_empty() {
        format!("Usage: {status}")
    } else {
        lines.join("\n")
    }
}

fn draw_view_pane(
    frame: &mut Frame,
    app: &App,
    view: &crate::app::ViewState,
    leader_active: bool,
    show_tmux_cursor: bool,
) {
    let sidebar_data = build_agent_sidebar_data(app, view);
    let tmux_cursor = if show_tmux_cursor {
        app.tmux_cursor
    } else {
        None
    };
    let compose_intercept = view
        .session_kind
        .is_agent_harness()
        .then(|| app.compose_intercept_active(view));

    let next_prev_feature = (
        app.active_extension
            .keybindings
            .get("next_feature")
            .copied(),
        app.active_extension
            .keybindings
            .get("prev_feature")
            .copied(),
    );

    super::pane::draw_with_lines(
        frame,
        view,
        &app.pane_content,
        &app.pane_lines,
        sidebar_data.as_ref(),
        leader_active,
        app.pending_inputs.len(),
        tmux_cursor,
        compose_intercept,
        next_prev_feature,
        &app.throbber_state,
        &app.theme,
    );
}

fn draw_view_context_bar(
    frame: &mut Frame,
    view: &crate::app::ViewState,
    theme: &crate::theme::Theme,
) {
    draw_context_bar(
        frame,
        &view.project_name,
        &view.feature_name,
        Some(&view.session_label),
        Some(&view.vibe_mode),
        view.review,
        theme,
    );
}

fn draw_feature_context_bar(
    frame: &mut Frame,
    project_name: &str,
    feature_name: &str,
    theme: &crate::theme::Theme,
) {
    draw_context_bar(frame, project_name, feature_name, None, None, false, theme);
}

fn draw_mode_context_bar(frame: &mut Frame, mode: &AppMode, theme: &crate::theme::Theme) {
    if let Some(view) = mode_view_context(mode) {
        draw_view_context_bar(frame, view, theme);
    }
}

fn mode_view_context(mode: &AppMode) -> Option<&crate::app::ViewState> {
    match mode {
        AppMode::Viewing(view) => Some(view),
        AppMode::Help(state) => state.from_view.as_ref(),
        AppMode::NotificationPicker(_, from_view) => from_view.as_ref(),
        AppMode::CommandPicker(state) => state.from_view.as_ref(),
        AppMode::BookmarkPicker(state) => state.from_view.as_ref(),
        AppMode::DiffPicker(state) => Some(&state.from_view),
        AppMode::DiffViewerLoading(state) | AppMode::DiffViewer(state) => Some(&state.from_view),
        AppMode::SteeringPrompt(state) => Some(&state.view),
        AppMode::Compose(state) => Some(&state.view),
        AppMode::TodoQuickCapture(state) => Some(&state.view),
        AppMode::SessionPicker(state) => state.from_view.as_ref(),
        AppMode::DiffReviewPrompt(state) => state.return_to_view.as_ref(),
        AppMode::LatestPrompt(state) => Some(&state.view),
        AppMode::PromptLibrary(state) => state.from_view.as_ref(),
        AppMode::PromptEditor(state) => mode_view_context(state.return_to.as_ref()),
        AppMode::PlaceholderFill(state) => state.from_view.as_ref(),
        AppMode::SkillPicker(state) => mode_view_context(state.return_to.as_ref()),
        AppMode::DebugLog(state) => state.from_view.as_ref(),
        AppMode::MarkdownLoading(state) => state.from_view.as_ref(),
        AppMode::MarkdownViewer(state) => state.from_view.as_ref(),
        AppMode::MarkdownFilePicker(state) => state.from_view.as_ref(),
        _ => None,
    }
}

fn draw_context_bar(
    frame: &mut Frame,
    project_name: &str,
    feature_name: &str,
    session_label: Option<&str>,
    vibe_mode: Option<&VibeMode>,
    review: bool,
    theme: &crate::theme::Theme,
) {
    let viewport = frame.area();
    if viewport.width == 0 || viewport.height == 0 {
        return;
    }

    let area = Rect::new(viewport.x, viewport.y, viewport.width, 1);
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.effective_bg())),
        area,
    );

    let mut spans = vec![Span::raw("  ")];
    spans.push(Span::styled(
        project_name,
        Style::default()
            .fg(theme.project_title.to_color())
            .add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::styled(
        format!(" / {feature_name}"),
        Style::default()
            .fg(theme.warning.to_color())
            .add_modifier(Modifier::BOLD),
    ));
    if let Some(session_label) = session_label {
        spans.push(Span::styled(
            format!(" / {session_label}"),
            Style::default().fg(theme.text.to_color()),
        ));
    }
    if let Some(vibe_mode) = vibe_mode {
        let (label, color) = match vibe_mode {
            VibeMode::Vibeless => (" [vibeless]", theme.mode_vibeless.to_color()),
            VibeMode::Vibe => (" [vibe]", theme.mode_vibe.to_color()),
            VibeMode::SuperVibe => (" [supervibe]", theme.mode_supervibe.to_color()),
        };
        spans.push(Span::styled(label, Style::default().fg(color)));
    }
    if review {
        spans.push(Span::styled(
            " [review]",
            Style::default().fg(theme.mode_review.to_color()),
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    frame.render_widget(Clear, frame.area());
    frame.render_widget(
        Block::default().style(Style::default().bg(app.theme.effective_bg())),
        frame.area(),
    );

    // The PR-review family of full-screen modes (loading/picker/pane/running
    // views) each `return` before reaching the shared `draw_toasts` call near
    // the end of this function, so every one of them draws its own — a
    // success/warning/error toast pushed while any of these is showing
    // (e.g. the AI review's "found N findings" / error result landing while
    // the user is back in the pane) would otherwise never appear, silently
    // swallowed until the mode changes.
    if let AppMode::PrReviewLoading(state) = &app.mode {
        super::dialogs::draw_pr_review_loading(frame, state, &app.throbber_state, &app.theme);
        super::draw_toasts(frame, &app.toasts, &app.theme);
        return;
    }
    if matches!(app.mode, AppMode::PrReview(_)) {
        let fix_session_usage = app.pr_review_fix_session_usage();
        let triage_session_usage = app.pr_review_triage_session_usage();
        let dedicated_session_working = app.pr_review_dedicated_session_working();
        let ai_review_running = match &app.mode {
            AppMode::PrReview(state) => app.ai_review_running_for_workdir(&state.workdir),
            _ => false,
        };
        if let AppMode::PrReview(state) = &mut app.mode {
            super::dialogs::draw_pr_review(
                frame,
                state,
                &app.theme,
                super::dialogs::PrReviewUsage {
                    cumulative: fix_session_usage.as_ref(),
                    visit: triage_session_usage.as_ref(),
                    pricing: &app.config.token_pricing,
                },
                dedicated_session_working,
                ai_review_running,
            );
        }
        super::draw_toasts(frame, &app.toasts, &app.theme);
        return;
    }
    if matches!(app.mode, AppMode::AiReview(_)) {
        let ai_review_running = app.ai_review_bg.is_some();
        if let AppMode::AiReview(state) = &mut app.mode {
            super::dialogs::draw_ai_review(
                frame,
                state,
                &app.theme,
                ai_review_running,
                &app.throbber_state,
            );
        }
        super::draw_toasts(frame, &app.toasts, &app.theme);
        return;
    }
    if let AppMode::PrPicker(state) = &app.mode {
        let repo = app.repo_for_project_path(&state.workdir);
        let memory_path = crate::app::review_memory::review_memory_path(
            &repo,
            app.configured_review_memory_path(&repo).as_deref(),
        );
        super::dialogs::draw_pr_picker(frame, state, &app.theme, &memory_path);
        super::draw_toasts(frame, &app.toasts, &app.theme);
        return;
    }
    if let AppMode::ReviewMemoryBootstrapRunning(state) = &app.mode {
        super::dialogs::draw_review_memory_bootstrap_running(
            frame,
            state,
            &app.throbber_state,
            &app.theme,
        );
        super::draw_toasts(frame, &app.toasts, &app.theme);
        return;
    }
    if let AppMode::ReviewMemoryCompactRunning(state) = &app.mode {
        super::dialogs::draw_review_memory_compact_running(
            frame,
            state,
            &app.throbber_state,
            &app.theme,
        );
        super::draw_toasts(frame, &app.toasts, &app.theme);
        return;
    }
    if matches!(app.mode, AppMode::ReviewMemoryCompactReview(_)) {
        if let AppMode::ReviewMemoryCompactReview(state) = &mut app.mode {
            super::dialogs::draw_review_memory_compact_review(frame, state, &app.theme);
        }
        super::draw_toasts(frame, &app.toasts, &app.theme);
        return;
    }
    if let AppMode::AiReviewRunning(state) = &app.mode {
        super::dialogs::draw_ai_review_running(frame, state, &app.throbber_state, &app.theme);
        super::draw_toasts(frame, &app.toasts, &app.theme);
        return;
    }

    if let AppMode::Todos(state) = &app.mode {
        super::dialogs::draw_todos_view(frame, state, &app.theme, app.config.nerd_font);
        super::draw_toasts(frame, &app.toasts, &app.theme);
        return;
    }

    if let AppMode::Viewing(view) = &app.mode {
        let area = frame.area();
        draw_view_pane(frame, app, view, app.leader_active, true);
        // Top-right status badges for agent sessions. Remote Control is
        // Claude-only; the direct-input hint applies to every harness.
        if view.session_kind.is_agent_harness() {
            use ratatui::style::{Modifier, Style};
            use ratatui::text::Span;
            let mut badge_spans: Vec<Span> = Vec::new();
            if view.session_kind == SessionKind::Claude
                && crate::app::remote_control::detect_remote_control(&app.pane_content).active
            {
                badge_spans.push(Span::styled(
                    " [remote ●] ",
                    Style::default()
                        .fg(app.theme.success.to_color())
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if !app.compose_intercept_active(view) {
                badge_spans.push(Span::styled(
                    " [direct input — leader+e: compose] ",
                    Style::default()
                        .fg(app.theme.warning.to_color())
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if let Some(stash) = &app.pr_review_return
                && stash.session == view.session
                && stash.window == view.window
            {
                badge_spans.push(Span::styled(
                    " [Ctrl+Space P: back to PR Triage] ",
                    Style::default()
                        .fg(app.theme.info.to_color())
                        .add_modifier(Modifier::BOLD),
                ));
            }
            if let Some(span) = pr_triage_badge_span(app, view) {
                badge_spans.push(span);
            }
            draw_badge_row(frame, area, badge_spans);
        }
        // Show transient message (e.g. "Copied N chars") on the bottom line
        if let Some(ref msg) = app.message {
            let msg_area = Rect::new(
                area.x,
                area.y + area.height.saturating_sub(1),
                area.width,
                1,
            );
            let color = if msg.starts_with("Error:") {
                app.theme.danger.to_color()
            } else {
                app.theme.success.to_color()
            };
            let paragraph = ratatui::widgets::Paragraph::new(ratatui::text::Span::styled(
                format!(" {}", msg),
                ratatui::style::Style::default().fg(color),
            ));
            frame.render_widget(paragraph, msg_area);
        }
        super::draw_toasts(frame, &app.toasts, &app.theme);
        return;
    }

    if let AppMode::SessionSwitcher(state) = &app.mode {
        let return_kind = state
            .sessions
            .iter()
            .find(|entry| entry.tmux_window == state.return_window)
            .map(|entry| entry.kind.clone())
            .unwrap_or(crate::project::SessionKind::Terminal);
        let temp_view = crate::app::ViewState::new(
            state.project_name.clone(),
            state.feature_name.clone(),
            state.tmux_session.clone(),
            state.return_window.clone(),
            state.return_label.clone(),
            return_kind,
            state.vibe_mode.clone(),
            state.review,
        );
        draw_view_pane(frame, app, &temp_view, false, false);
        super::picker::draw_session_switcher(frame, state, app.config.nerd_font, &app.theme);
        return;
    }

    if let AppMode::Help(state) = &app.mode
        && let Some(view) = &state.from_view
    {
        let scroll = state.scroll_offset;
        draw_view_pane(frame, app, view, false, false);
        super::dialogs::draw_help(frame, scroll, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    if let AppMode::NotificationPicker(selected, Some(view)) = &app.mode {
        draw_view_pane(frame, app, view, false, false);
        super::picker::draw_notification_picker(frame, &app.pending_inputs, *selected, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    if let AppMode::LatestPrompt(state) = &app.mode {
        draw_view_pane(frame, app, &state.view, false, false);
        super::dialogs::draw_latest_prompt_dialog(frame, state, app.message.as_deref(), &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    if let AppMode::PromptLibrary(state) = &app.mode
        && let Some(view) = &state.from_view
    {
        draw_view_pane(frame, app, view, false, false);
        super::dialogs::draw_prompt_library(frame, state, app.message.as_deref(), &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    if let AppMode::DiffPicker(state) = &app.mode {
        draw_view_pane(frame, app, &state.from_view, false, false);
        super::dialogs::draw_diff_picker(frame, state, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    let diff_from_view = if let AppMode::DiffViewer(state) = &app.mode {
        Some(state.from_view.clone())
    } else {
        None
    };
    if let Some(view) = diff_from_view.as_ref() {
        draw_view_pane(frame, app, view, false, false);
    }
    if let AppMode::DiffViewer(state) = &mut app.mode {
        super::dialogs::draw_diff_viewer(frame, state, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    if let AppMode::DiffViewerLoading(state) = &app.mode {
        draw_view_pane(frame, app, &state.from_view, false, false);
        super::dialogs::draw_diff_viewer_loading(frame, state, &app.throbber_state, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    if let AppMode::MarkdownLoading(state) = &app.mode {
        if let Some(view) = state.from_view.clone() {
            draw_view_pane(frame, app, &view, false, false);
        }
        super::dialogs::draw_markdown_loading(frame, state, &app.throbber_state, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    let markdown_from_view = if let AppMode::MarkdownViewer(state) = &app.mode {
        state.from_view.clone()
    } else {
        None
    };
    if let Some(view) = markdown_from_view.as_ref() {
        draw_view_pane(frame, app, view, false, false);
    }
    if let AppMode::MarkdownViewer(state) = &mut app.mode {
        super::dialogs::draw_markdown_viewer(frame, state, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }
    let compose_from_view = if let AppMode::Compose(state) = &app.mode {
        Some(state.view.clone())
    } else {
        None
    };
    if let Some(view) = compose_from_view.as_ref() {
        draw_view_pane(frame, app, view, false, false);
    }
    if let AppMode::Compose(state) = &mut app.mode {
        super::dialogs::draw_compose_dialog(frame, state, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        // `draw_mode_context_bar` clears and redraws the whole top row for
        // its breadcrumb, so the ambient PR badge — the compose box itself
        // lives at the bottom of the frame, leaving the top-right corner
        // free — has to be drawn after it, not before, or it gets wiped.
        // Composing doesn't change which session (or PR) is underneath, so
        // reuse the same badge `Viewing` shows.
        if let Some(view) = compose_from_view.as_ref()
            && view.session_kind.is_agent_harness()
            && let Some(span) = pr_triage_badge_span(app, view)
        {
            draw_badge_row(frame, frame.area(), vec![span]);
        }
        return;
    }

    let quick_capture_from_view = if let AppMode::TodoQuickCapture(state) = &app.mode {
        Some(state.view.clone())
    } else {
        None
    };
    if let Some(view) = quick_capture_from_view.as_ref() {
        draw_view_pane(frame, app, view, false, false);
    }
    if let AppMode::TodoQuickCapture(state) = &app.mode {
        super::dialogs::draw_todo_quick_capture_dialog(frame, state, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    let steering_from_view = if let AppMode::SteeringPrompt(state) = &app.mode {
        Some(state.view.clone())
    } else {
        None
    };
    if let Some(view) = steering_from_view.as_ref() {
        draw_view_pane(frame, app, view, false, false);
    }
    if let AppMode::SteeringPrompt(state) = &mut app.mode {
        super::dialogs::draw_steering_prompt_dialog(frame, state, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    if let AppMode::CommandPicker(state) = &app.mode
        && state.from_view.is_some()
    {
        let view = state.from_view.as_ref().unwrap();
        draw_view_pane(frame, app, view, false, false);
        super::picker::draw_command_picker(frame, state, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    if let AppMode::SyntaxLanguagePicker(state) = &app.mode {
        super::picker::draw_syntax_language_picker(frame, state, &app.throbber_state, &app.theme);
        return;
    }

    if let AppMode::MarkdownFilePicker(state) = &app.mode
        && state.from_view.is_some()
    {
        let view = state.from_view.as_ref().unwrap();
        draw_view_pane(frame, app, view, false, false);
        super::picker::draw_markdown_file_picker(frame, state, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    if let AppMode::BookmarkPicker(state) = &app.mode
        && state.from_view.is_some()
    {
        let view = state.from_view.as_ref().unwrap();
        draw_view_pane(frame, app, view, false, false);
        let rows = app.bookmark_picker_rows();
        super::picker::draw_bookmark_picker(frame, state, &rows, &app.theme);
        draw_mode_context_bar(frame, &app.mode, &app.theme);
        return;
    }

    if let AppMode::RenamingSession(state) = &app.mode
        && let RenameReturnTo::SessionSwitcher(ref sw) = state.return_to
    {
        let return_kind = sw
            .sessions
            .iter()
            .find(|entry| entry.tmux_window == sw.return_window)
            .map(|entry| entry.kind.clone())
            .unwrap_or(crate::project::SessionKind::Terminal);
        let temp_view = crate::app::ViewState::new(
            sw.project_name.clone(),
            sw.feature_name.clone(),
            sw.tmux_session.clone(),
            sw.return_window.clone(),
            sw.return_label.clone(),
            return_kind,
            sw.vibe_mode.clone(),
            sw.review,
        );
        draw_view_pane(frame, app, &temp_view, false, false);
        super::dialogs::draw_rename_session_dialog(frame, state, &app.theme);
        draw_view_context_bar(frame, &temp_view, &app.theme);
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(5),
            Constraint::Length(3),
        ])
        .split(frame.area());

    super::header::draw(
        frame,
        chunks[0],
        &std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        env!("CARGO_PKG_VERSION"),
        app.pending_inputs.len(),
        &app.theme,
    );
    super::list::draw(frame, app, chunks[1]);
    super::status::draw(frame, app, chunks[2]);

    if app.leader_active {
        super::pane::draw_command_menu(
            frame,
            frame.area(),
            " Ctrl+Space commands ",
            DASHBOARD_LEADER_COMMANDS,
            &app.theme,
        );
    }

    match &app.mode {
        AppMode::CreatingProject(state) => {
            let allowed_agents =
                app.allowed_agents_for_project_path(&std::path::PathBuf::from(&state.path));
            super::dialogs::draw_create_project_dialog(
                frame,
                state,
                allowed_agents.as_slice(),
                &app.theme,
            );
        }
        AppMode::CreatingFeature(state) => {
            if state.step == CreateFeatureStep::ConfirmSuperVibe {
                super::dialogs::draw_confirm_supervibe_dialog(frame, &app.theme);
            } else {
                super::dialogs::draw_create_feature_dialog(
                    frame,
                    state,
                    state.feature_presets.as_slice(),
                    state.allowed_agents.as_slice(),
                    &app.theme,
                );
            }
        }
        AppMode::PlanInterview(state) => {
            super::dialogs::draw_plan_interview_dialog(
                frame,
                state,
                app.message.as_deref(),
                &app.theme,
            );
        }
        AppMode::CreatingBatchFeatures(state) => {
            super::dialogs::draw_create_batch_features_dialog(frame, state, &app.theme);
        }
        AppMode::DeletingProject(name) => {
            super::dialogs::draw_delete_project_confirm(frame, name, &app.theme);
        }
        AppMode::DeletingFeature(project_name, feature_name) => {
            super::dialogs::draw_delete_feature_confirm(
                frame,
                project_name,
                feature_name,
                &app.theme,
            );
        }
        AppMode::BrowsingPath(state) => {
            super::dialogs::draw_browse_path_dialog(frame, state, &app.theme);
        }
        _ => {}
    }

    if let AppMode::PrNumberPrompt(state) = &app.mode {
        super::dialogs::draw_pr_number_prompt(frame, state, &app.theme);
    }

    if let AppMode::TodosHostReassign(state) = &app.mode {
        super::dialogs::draw_todos_host_reassign_dialog(frame, state, &app.theme);
    }

    if let AppMode::RenamingSession(state) = &app.mode {
        super::dialogs::draw_rename_session_dialog(frame, state, &app.theme);
    }

    if let AppMode::RenamingFeature(state) = &app.mode {
        super::dialogs::draw_rename_feature_dialog(frame, state, &app.theme);
    }

    if let AppMode::SessionConfig(state) = &app.mode {
        super::dialogs::draw_session_config_dialog(frame, state, &app.theme);
    }

    if let AppMode::ProjectAgentConfig(state) = &app.mode {
        super::dialogs::draw_project_agent_config_dialog(frame, state, &app.theme);
    }

    if let AppMode::Help(state) = &app.mode
        && state.from_view.is_none()
    {
        let scroll = state.scroll_offset;
        super::dialogs::draw_help(frame, scroll, &app.theme);
    }

    if let AppMode::NotificationPicker(selected, None) = &app.mode {
        super::picker::draw_notification_picker(frame, &app.pending_inputs, *selected, &app.theme);
    }

    if let AppMode::CommandPicker(state) = &app.mode {
        super::picker::draw_command_picker(frame, state, &app.theme);
    }

    if let AppMode::Searching(state) = &app.mode {
        super::dialogs::draw_search_dialog(frame, state, &app.theme);
    }

    if let AppMode::OpencodeSessionPicker(state) = &app.mode {
        super::picker::draw_opencode_session_picker(frame, state, &app.theme);
    }

    if matches!(app.mode, AppMode::ConfirmingOpencodeSession { .. }) {
        super::picker::draw_opencode_session_confirm(frame, &app.theme);
    }

    if let AppMode::ClaudeSessionPicker(state) = &app.mode {
        super::picker::draw_claude_session_picker(frame, state, &app.theme);
    }

    if matches!(app.mode, AppMode::ConfirmingClaudeSession { .. }) {
        super::picker::draw_claude_session_confirm(frame, &app.theme);
    }

    if let AppMode::CodexSessionPicker(state) = &app.mode {
        super::picker::draw_codex_session_picker(frame, state, &app.theme);
    }

    if matches!(app.mode, AppMode::ConfirmingCodexSession { .. }) {
        super::picker::draw_codex_session_confirm(frame, &app.theme);
    }

    if let AppMode::SessionPicker(state) = &app.mode {
        super::picker::draw_session_picker(frame, state, app.config.nerd_font, &app.theme);
    }

    if let AppMode::NamingNewSession(state) = &app.mode {
        super::dialogs::draw_new_session_name_dialog(frame, state, &app.theme);
    }

    if let AppMode::BookmarkPicker(state) = &app.mode {
        let rows = app.bookmark_picker_rows();
        super::picker::draw_bookmark_picker(frame, state, &rows, &app.theme);
    }

    if let AppMode::DiffReviewPrompt(state) = &app.mode {
        super::dialogs::draw_diff_review_dialog(frame, state, &app.throbber_state, &app.theme);
    }

    if let AppMode::RunningHook(state) = &app.mode {
        super::dialogs::draw_running_hook_dialog(frame, state, &app.throbber_state, &app.theme);
    }

    if let AppMode::DeletingFeatureInProgress(state) = &app.mode {
        super::dialogs::draw_deleting_feature_dialog(frame, state, &app.throbber_state, &app.theme);
    }

    if let AppMode::HookPrompt(state) = &app.mode {
        super::dialogs::draw_hook_prompt_dialog(frame, state, &app.theme);
    }

    if let AppMode::ForkingFeature(state) = &app.mode {
        let allowed_agents = app.active_extension.allowed_agents();
        super::dialogs::draw_fork_feature_dialog(
            frame,
            state,
            allowed_agents.as_slice(),
            &app.theme,
        );
    }

    if let AppMode::ThemePicker(state) = &app.mode {
        super::dialogs::draw_theme_picker(
            frame,
            state,
            &app.config.theme,
            &app.theme,
            app.config.transparent_background,
        );
    }

    if let AppMode::ReviewHarnessPick(state) = &app.mode {
        super::dialogs::draw_review_harness_pick(frame, state, &app.theme);
    }

    if let AppMode::DebugLog(state) = &app.mode {
        super::dialogs::draw_debug_log(
            frame,
            &app.debug_log,
            state.scroll_offset,
            state.hide_perf_logs,
            &app.theme,
        );
    }

    if let AppMode::HarnessSetup(state) = &app.mode {
        super::dialogs::draw_harness_setup_dialog(frame, state, &app.throbber_state, &app.theme);
    }

    if let AppMode::PromptLibrary(state) = &app.mode {
        super::dialogs::draw_prompt_library(frame, state, app.message.as_deref(), &app.theme);
    }

    if let AppMode::PromptEditor(state) = &app.mode {
        super::dialogs::draw_prompt_editor(frame, state, &app.theme);
    }

    if let AppMode::PlaceholderFill(state) = &app.mode {
        super::dialogs::draw_placeholder_fill(frame, state, &app.theme);
    }

    if let AppMode::SkillPicker(state) = &app.mode {
        super::dialogs::draw_skill_picker(frame, state, &app.theme);
    }

    if let AppMode::ConfigWizard(state) = &mut app.mode {
        super::dialogs::draw_config_wizard_dialog(frame, state, &app.theme);
    }

    draw_mode_context_bar(frame, &app.mode, &app.theme);
    match &app.mode {
        AppMode::DeletingFeatureInProgress(state) => {
            draw_feature_context_bar(frame, &state.project_name, &state.feature_name, &app.theme);
        }
        AppMode::RunningHook(state) => {
            draw_feature_context_bar(frame, &state.project_name, &state.branch, &app.theme);
        }
        AppMode::HookPrompt(state) => match &state.next {
            crate::app::HookNext::WorktreeCreated {
                project_name,
                branch,
                ..
            } => draw_feature_context_bar(frame, project_name, branch, &app.theme),
            crate::app::HookNext::StartFeature { pi, fi }
            | crate::app::HookNext::StopFeature { pi, fi } => {
                if let Some(project) = app.store.projects.get(*pi)
                    && let Some(feature) = project.features.get(*fi)
                {
                    draw_feature_context_bar(frame, &project.name, &feature.name, &app.theme);
                }
            }
        },
        _ => {}
    }

    super::draw_toasts(frame, &app.toasts, &app.theme);
}

pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{App, PendingInput, ViewState};
    use crate::project::FeatureSession;
    use crate::project::{
        AgentKind, Feature, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode,
    };
    use crate::token_tracking::{TokenUsageProvider, TokenUsageSource};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use ratatui::layout::Rect;
    use ratatui::{Terminal, backend::TestBackend};
    use std::collections::HashMap;
    use std::path::PathBuf;

    // ── centered_rect ─────────────────────────────────────────

    #[test]
    fn centered_rect_50_percent() {
        let area = Rect::new(0, 0, 100, 100);
        let result = centered_rect(50, 50, area);
        // Middle slice should be 50% of 100 = 50 in each dim
        assert_eq!(result.width, 50);
        assert_eq!(result.height, 50);
        // Should start at 25% offset
        assert_eq!(result.x, 25);
        assert_eq!(result.y, 25);
    }

    #[test]
    fn centered_rect_80_60_percent() {
        let area = Rect::new(0, 0, 100, 100);
        let result = centered_rect(80, 60, area);
        assert_eq!(result.width, 80);
        assert_eq!(result.height, 60);
        assert_eq!(result.x, 10);
        assert_eq!(result.y, 20);
    }

    #[test]
    fn centered_rect_fits_within_area() {
        let area = Rect::new(10, 5, 80, 40);
        let result = centered_rect(60, 50, area);
        // Result must be inside the original area
        assert!(result.x >= area.x);
        assert!(result.y >= area.y);
        assert!(result.x + result.width <= area.x + area.width);
        assert!(result.y + result.height <= area.y + area.height);
    }

    #[test]
    fn centered_rect_100_percent_fills_area() {
        let area = Rect::new(0, 0, 80, 40);
        let result = centered_rect(100, 100, area);
        assert_eq!(result.width, area.width);
        assert_eq!(result.height, area.height);
    }

    #[test]
    fn sidebar_usage_is_split_into_labeled_lines() {
        assert_eq!(
            format_sidebar_usage("16.0k in · 2.0k out · 21.8k eff · $0.07"),
            "Input: 16.0k tokens\nOutput: 2.0k tokens\nEffective: 21.8k tokens\nCost: $0.07"
        );
    }

    #[test]
    fn sidebar_usage_falls_back_when_format_is_unknown() {
        assert_eq!(
            format_sidebar_usage("tokens unavailable"),
            "Usage: tokens unavailable"
        );
    }

    #[test]
    fn expired_toasts_do_not_leave_stale_cells_behind() {
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.toasts.push(crate::app::Toast::new(
            "toast should disappear",
            crate::app::toast::ToastKind::Info,
        ));

        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("first draw");
        let first_rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(first_rendered.contains("toast should disappear"));

        app.toasts.clear();
        terminal
            .draw(|frame| draw(frame, &mut app))
            .expect("second draw");
        let second_rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(!second_rendered.contains("toast should disappear"));
    }

    fn codex_feature_session(session_id: &str) -> FeatureSession {
        FeatureSession {
            id: "session-1".into(),
            kind: SessionKind::Codex,
            label: "Codex".into(),
            tmux_window: "codex".into(),
            claude_session_id: None,
            token_usage_source: Some(TokenUsageSource {
                provider: TokenUsageProvider::Codex,
                id: session_id.into(),
            }),
            token_usage_source_match: Some(TokenUsageSourceMatch::Exact),
            created_at: chrono::Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
        }
    }

    fn sidebar_usage_session(
        id: &str,
        kind: SessionKind,
        window: &str,
        label: &str,
        status_text: &str,
    ) -> FeatureSession {
        FeatureSession {
            id: id.into(),
            kind,
            label: label.into(),
            tmux_window: window.into(),
            claude_session_id: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: chrono::Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: Some(status_text.into()),
            token_usage: None,
        }
    }

    fn sidebar_usage_app(kind: SessionKind) -> App {
        let now = chrono::Utc::now();
        let feature = Feature {
            id: "feat-1".into(),
            name: "feature".into(),
            branch: "feature".into(),
            workdir: PathBuf::from("/tmp/demo"),
            is_worktree: false,
            tmux_session: "amf-feature".into(),
            sessions: vec![
                sidebar_usage_session(
                    "session-1",
                    kind.clone(),
                    "agent-1",
                    "Agent 1",
                    "1.0k in · 100 out · 1.5k eff · $0.01",
                ),
                sidebar_usage_session(
                    "session-2",
                    kind.clone(),
                    "agent-2",
                    "Agent 2",
                    "2.0k in · 200 out · 3.0k eff · $0.02",
                ),
            ],
            collapsed: false,
            mode: VibeMode::Vibeless,
            review: false,
            plan_mode: false,
            agent: AgentKind::Claude,
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Idle,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
        };
        App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![Project {
                    id: "proj-1".into(),
                    name: "demo".into(),
                    repo: PathBuf::from("/tmp/demo"),
                    collapsed: false,
                    features: vec![feature],
                    created_at: now,
                    preferred_agent: AgentKind::Claude,
                    is_git: false,
                }],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        )
    }

    fn sidebar_usage_view(kind: SessionKind, window: &str, label: &str) -> ViewState {
        ViewState::new(
            "demo".into(),
            "feature".into(),
            "amf-feature".into(),
            window.into(),
            label.into(),
            kind,
            VibeMode::Vibeless,
            false,
        )
    }

    #[test]
    fn select_sidebar_prompt_prefers_session_specific_prompt() {
        assert_eq!(
            select_sidebar_prompt(Some("session prompt"), Some("fallback prompt")),
            Some("session prompt".to_string())
        );
    }

    #[test]
    fn sidebar_prompt_text_falls_back_when_codex_session_prompt_is_missing() {
        let prompt = sidebar_prompt_text(None, Some("fallback prompt"));

        assert!(prompt.contains("fallback prompt"));
        assert!(!prompt.contains("leader l"));
        assert!(!prompt.contains("Preview:"));
    }

    #[test]
    fn sidebar_prompt_text_is_empty_when_no_prompt_is_available() {
        assert_eq!(sidebar_prompt_text(None, None), "");
    }

    #[test]
    fn sidebar_prompt_text_truncates_long_prompt_preview() {
        let prompt = sidebar_prompt_text(
            Some(
                "This is a much longer prompt preview that should be shortened once it crosses the sidebar limit for prompt text.",
            ),
            None,
        );

        assert_eq!(prompt, "This is a much longer prompt preview that shoul…");
    }

    #[test]
    fn compact_sidebar_text_truncates_summary_text() {
        let compacted = compact_sidebar_text(
            "This is a longer summary that should be shortened once it crosses the sidebar limit.",
            40,
        );

        assert_eq!(compacted, "This is a longer summary that should be…");
    }

    #[test]
    fn sidebar_status_activity_text_omits_activity_when_work_is_present() {
        assert_eq!(sidebar_status_activity_text(true, "Ready".into()), None);
        assert_eq!(
            sidebar_status_activity_text(false, "Ready".into()),
            Some("Ready".to_string())
        );
    }

    #[test]
    fn compose_sidebar_status_text_omits_missing_usage_lines() {
        assert_eq!(compose_sidebar_status_text(None, None, None), "");
        assert_eq!(
            compose_sidebar_status_text(Some("Ready".into()), None, None),
            "Activity: Ready"
        );
        assert_eq!(
            compose_sidebar_status_text(None, Some("Input: 1.2K tokens".into()), None),
            "Input: 1.2K tokens"
        );
    }

    #[test]
    fn append_model_status_line_adds_model_when_present() {
        assert_eq!(
            append_model_status_line("Activity: Ready".into(), Some("Model: gpt-5.5")),
            "Activity: Ready\nModel: gpt-5.5"
        );
        assert_eq!(
            append_model_status_line("Activity: Ready".into(), None),
            "Activity: Ready"
        );
    }

    #[test]
    fn compose_sidebar_summary_text_omits_missing_summary() {
        assert_eq!(compose_sidebar_summary_text(None, None), "");
        assert_eq!(
            compose_sidebar_summary_text(None, Some("Short summary".into())),
            "Short summary"
        );
    }

    #[test]
    fn format_codex_usage_source_confidence_omits_exact_match_label() {
        let session = codex_feature_session("sess-current");
        assert_eq!(
            format_codex_usage_source_confidence(&SessionKind::Codex, Some(&session)),
            None
        );
    }

    #[test]
    fn sidebar_usage_follows_selected_agent_window() {
        for kind in [
            SessionKind::Claude,
            SessionKind::Codex,
            SessionKind::Opencode,
        ] {
            let app = sidebar_usage_app(kind.clone());
            let first = build_agent_sidebar_data(
                &app,
                &sidebar_usage_view(kind.clone(), "agent-1", "Agent 1"),
            )
            .unwrap();
            let second =
                build_agent_sidebar_data(&app, &sidebar_usage_view(kind, "agent-2", "Agent 2"))
                    .unwrap();

            assert!(first.status_text.contains("Input: 1.0k tokens"));
            assert!(!first.status_text.contains("Input: 2.0k tokens"));
            assert!(second.status_text.contains("Input: 2.0k tokens"));
            assert!(!second.status_text.contains("Input: 1.0k tokens"));
        }
    }

    #[test]
    fn format_codex_usage_source_confidence_uses_inferred_match_label() {
        let mut session = codex_feature_session("sess-current");
        session.token_usage_source_match = Some(TokenUsageSourceMatch::Inferred);

        assert_eq!(
            format_codex_usage_source_confidence(&SessionKind::Codex, Some(&session)),
            Some("Usage source: inferred workdir match".to_string())
        );
    }

    fn store_with_claude_feature() -> (ProjectStore, Feature) {
        let now = chrono::Utc::now();
        let mut feature = Feature {
            id: "feat-1".into(),
            name: "feature".into(),
            branch: "feature".into(),
            workdir: PathBuf::from("/tmp/demo"),
            is_worktree: false,
            tmux_session: "amf-feature".into(),
            sessions: vec![],
            collapsed: false,
            mode: VibeMode::Vibeless,
            review: false,
            plan_mode: false,
            agent: AgentKind::Claude,
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Active,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
        };
        feature.add_session_named(SessionKind::Claude, "Claude 1".to_string());
        let project = Project {
            id: "proj-1".into(),
            name: "demo".into(),
            repo: PathBuf::from("/tmp/demo"),
            collapsed: false,
            features: vec![feature.clone()],
            created_at: now,
            preferred_agent: AgentKind::Claude,
            is_git: false,
        };
        (
            ProjectStore {
                version: 5,
                projects: vec![project],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            feature,
        )
    }

    #[test]
    fn pr_triage_sidebar_text_is_none_without_an_active_pr() {
        let (store, feature) = store_with_claude_feature();
        let app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );

        assert!(pr_triage_sidebar_text(&app, &feature).is_none());
    }

    #[test]
    fn pr_triage_sidebar_text_reports_pr_open_count_and_no_line_without_a_count() {
        let (store, feature) = store_with_claude_feature();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.active_prs.insert(
            feature.id.clone(),
            crate::app::ActivePrStatus {
                branch: "feature".to_string(),
                head_sha: "abc123".to_string(),
                number: 321,
                unresolved_threads: Some(4),
            },
        );

        assert_eq!(
            pr_triage_sidebar_text(&app, &feature),
            Some("PR: #321 · 4 open".to_string())
        );

        app.active_prs
            .get_mut(&feature.id)
            .unwrap()
            .unresolved_threads = None;
        assert_eq!(
            pr_triage_sidebar_text(&app, &feature),
            Some("PR: #321".to_string())
        );
    }

    #[test]
    fn pr_triage_sidebar_text_adds_working_and_ai_review_lines() {
        let (mut store, _) = store_with_claude_feature();
        let dedicated_id = store.projects[0].features[0]
            .add_session_named(
                SessionKind::Claude,
                crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
            )
            .id
            .clone();
        let feature = store.projects[0].features[0].clone();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.active_prs.insert(
            feature.id.clone(),
            crate::app::ActivePrStatus {
                branch: "feature".to_string(),
                head_sha: "abc123".to_string(),
                number: 321,
                unresolved_threads: Some(4),
            },
        );

        app.handle_ipc_message_value(serde_json::json!({
            "type": "thinking-start",
            "session_id": feature.tmux_session,
            "amf_feature_session_id": dedicated_id,
        }));
        assert_eq!(
            pr_triage_sidebar_text(&app, &feature),
            Some("PR: #321 · 4 open\nStatus: Working".to_string())
        );

        let pr = crate::github::PrRef {
            number: 321,
            head_sha: "abc123".to_string(),
            url: "https://github.com/o/r/pull/321".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            head_ref: "main".to_string(),
        };
        let origin = crate::app::AiReviewState {
            workdir: feature.workdir.clone(),
            pr,
            findings: Vec::new(),
            summary: None,
            selected: 0,
            detail_scroll: 0,
            detail_content_lines: 0,
            last_run: None,
            harness: None,
            harness_pick: None,
            model: None,
            model_picked: false,
            model_pick: None,
            finding_editor: None,
            post_confirm: None,
        };
        let (_tx, rx) = std::sync::mpsc::channel();
        app.ai_review_bg = Some(rx);
        app.ai_review_pending = Some(origin);
        assert_eq!(
            pr_triage_sidebar_text(&app, &feature),
            Some("PR: #321 · 4 open\nStatus: Working\nAI review: Running".to_string())
        );
    }

    fn render_viewing_mode_with_active_pr(sidebar_visible: bool) -> String {
        let (store, feature) = store_with_claude_feature();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.active_prs.insert(
            feature.id.clone(),
            crate::app::ActivePrStatus {
                branch: "feature".to_string(),
                head_sha: "abc123".to_string(),
                number: 321,
                unresolved_threads: Some(4),
            },
        );
        let mut view = ViewState::new(
            "demo".to_string(),
            "feature".to_string(),
            feature.tmux_session.clone(),
            "claude".to_string(),
            "Claude".to_string(),
            SessionKind::Claude,
            VibeMode::default(),
            false,
        );
        view.sidebar_visible = sidebar_visible;
        app.mode = crate::app::AppMode::Viewing(view);

        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    #[test]
    fn viewing_mode_shows_ambient_pr_badge_when_sidebar_is_hidden() {
        let rendered = render_viewing_mode_with_active_pr(false);

        assert!(rendered.contains("PR #321"));
        assert!(rendered.contains("4 open"));
        assert!(!rendered.contains("PR Triage"));
    }

    #[test]
    fn viewing_mode_shows_pr_triage_sidebar_box_instead_of_badge_when_sidebar_is_visible() {
        let rendered = render_viewing_mode_with_active_pr(true);

        assert!(rendered.contains("PR Triage"));
        assert!(rendered.contains("PR: #321"));
        assert!(rendered.contains("4 open"));
        assert!(!rendered.contains("[PR #321"));
    }

    fn pr_review_state_with_branches(
        pr_head_ref: &str,
        checked_out: &str,
    ) -> crate::app::PrReviewState {
        let pr = crate::github::PrRef {
            number: 321,
            head_sha: "abc123".to_string(),
            url: "https://github.com/o/r/pull/321".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            head_ref: pr_head_ref.to_string(),
        };
        let review = crate::app::pr_review::normalize(pr, vec![], vec![], vec![], vec![]);
        crate::app::PrReviewState {
            workdir: PathBuf::from("/tmp/demo"),
            review,
            selected: 0,
            detail_scroll: 0,
            detail_content_lines: 0,
            hide_resolved: false,
            sort_mode: crate::app::pr_review::PrSortMode::default(),
            fix_target: crate::app::pr_review::FixTarget::default(),
            fix_target_picked: false,
            usage_baselines: HashMap::new(),
            review_harness: None,
            harness_pick: None,
            fix_confirm: None,
            fix_vim_enabled: false,
            mark_pick: None,
            reply_kind_pick: None,
            reply: None,
            memory_add: None,
            marked: std::collections::HashSet::new(),
            pending_batch: false,
            checked_out_branch: Some(checked_out.to_string()),
            pending_ai_review_findings: 0,
        }
    }

    #[test]
    fn pr_review_pane_shows_ai_review_running_badge_for_its_own_workdir() {
        let (store, _feature) = store_with_claude_feature();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        let state = pr_review_state_with_branches("main", "main");
        let workdir = state.workdir.clone();
        app.mode = crate::app::AppMode::PrReview(state);

        let pr = crate::github::PrRef {
            number: 321,
            head_sha: "abc123".to_string(),
            url: "https://github.com/o/r/pull/321".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            head_ref: "main".to_string(),
        };
        let (_tx, rx) = std::sync::mpsc::channel();
        app.ai_review_bg = Some(rx);
        app.ai_review_pending = Some(crate::app::AiReviewState {
            workdir,
            pr,
            findings: Vec::new(),
            summary: None,
            selected: 0,
            detail_scroll: 0,
            detail_content_lines: 0,
            last_run: None,
            harness: None,
            harness_pick: None,
            model: None,
            model_picked: false,
            model_pick: None,
            finding_editor: None,
            post_confirm: None,
        });

        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(rendered.contains("AI review running"));
    }

    #[test]
    fn pr_review_pane_omits_ai_review_badge_when_nothing_is_running() {
        let (store, _feature) = store_with_claude_feature();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.mode = crate::app::AppMode::PrReview(pr_review_state_with_branches("main", "main"));

        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(!rendered.contains("AI review running"));
        assert!(!rendered.contains("AI review pending"));
    }

    #[test]
    fn pr_review_pane_shows_completed_pending_ai_review_count() {
        let (store, _feature) = store_with_claude_feature();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        let mut state = pr_review_state_with_branches("main", "main");
        state.pending_ai_review_findings = 3;
        app.mode = crate::app::AppMode::PrReview(state);

        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(rendered.contains("AI review pending: 3"));
    }

    #[test]
    fn pr_review_pane_shows_branch_mismatch_banner_when_workdir_branch_differs() {
        let (store, _feature) = store_with_claude_feature();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.mode =
            crate::app::AppMode::PrReview(pr_review_state_with_branches("main", "other-branch"));

        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(rendered.contains("reviewing PR for branch"));
        assert!(rendered.contains("other-branch"));
    }

    #[test]
    fn pr_review_pane_hides_branch_mismatch_banner_when_branches_match() {
        let (store, _feature) = store_with_claude_feature();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.mode = crate::app::AppMode::PrReview(pr_review_state_with_branches("main", "main"));

        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(!rendered.contains("reviewing PR for branch"));
    }

    #[test]
    fn compose_mode_still_shows_the_ambient_pr_badge_with_sidebar_hidden() {
        let (store, feature) = store_with_claude_feature();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.active_prs.insert(
            feature.id.clone(),
            crate::app::ActivePrStatus {
                branch: "feature".to_string(),
                head_sha: "abc123".to_string(),
                number: 321,
                unresolved_threads: Some(4),
            },
        );
        let mut view = ViewState::new(
            "demo".to_string(),
            "feature".to_string(),
            feature.tmux_session.clone(),
            "claude".to_string(),
            "Claude".to_string(),
            SessionKind::Claude,
            VibeMode::default(),
            false,
        );
        view.sidebar_visible = false;
        assert!(
            pr_triage_badge_span(&app, &view).is_some(),
            "expected a badge span before entering Compose mode"
        );
        app.mode = crate::app::AppMode::Compose(crate::app::ComposeState::new(
            view,
            feature.workdir.clone(),
            String::new(),
            Vec::new(),
        ));

        let backend = TestBackend::new(140, 30);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| super::draw(frame, &mut app)).unwrap();
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(rendered.contains("PR #321"));
        assert!(rendered.contains("4 open"));
    }

    #[test]
    fn fallback_sidebar_work_text_prefers_pending_input_message() {
        let now = chrono::Utc::now();
        let feature = Feature {
            id: "feat-1".into(),
            name: "feature".into(),
            branch: "feature".into(),
            workdir: PathBuf::from("/tmp/demo"),
            is_worktree: false,
            tmux_session: "amf-feature".into(),
            sessions: vec![codex_feature_session("sess-current")],
            collapsed: false,
            mode: VibeMode::Vibeless,
            review: false,
            plan_mode: false,
            agent: AgentKind::Codex,
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Idle,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
        };
        let project = Project {
            id: "proj-1".into(),
            name: "demo".into(),
            repo: PathBuf::from("/tmp/demo"),
            collapsed: false,
            features: vec![feature.clone()],
            created_at: now,
            preferred_agent: AgentKind::Codex,
            is_git: false,
        };
        let mut app = App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![project.clone()],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.pending_inputs.push(PendingInput {
            session_id: "amf-feature".into(),
            cwd: "/tmp/demo".into(),
            message: "Need approval before applying the patch.".into(),
            notification_type: "input-request".into(),
            file_path: PathBuf::new(),
            target_file_path: None,
            relative_path: None,
            change_id: None,
            tool: None,
            old_snippet: None,
            new_snippet: None,
            original_file: None,
            proposed_file: None,
            is_new_file: None,
            reason: None,
            response_file: None,
            project_name: Some("demo".into()),
            feature_name: Some("feature".into()),
            proceed_signal: None,
            request_id: None,
            reply_socket: None,
        });

        let view = ViewState::new(
            "demo".into(),
            "feature".into(),
            "amf-feature".into(),
            "codex".into(),
            "Codex".into(),
            SessionKind::Codex,
            VibeMode::Vibeless,
            false,
        );

        assert_eq!(
            fallback_sidebar_work_text(&app, &project, &feature, &view).as_deref(),
            Some("State: waiting for input\nRequest: Need approval before applying the patch.")
        );
    }

    #[test]
    fn pending_diff_review_updates_claude_sidebar_work_text() {
        let now = chrono::Utc::now();
        let feature = Feature {
            id: "feat-1".into(),
            name: "feature".into(),
            branch: "feature".into(),
            workdir: PathBuf::from("/tmp/demo"),
            is_worktree: false,
            tmux_session: "amf-feature".into(),
            sessions: vec![FeatureSession {
                id: "session-1".into(),
                kind: SessionKind::Claude,
                label: "Claude".into(),
                tmux_window: "claude".into(),
                claude_session_id: Some("claude-session".into()),
                token_usage_source: None,
                token_usage_source_match: None,
                created_at: now,
                command: None,
                on_stop: None,
                pre_check: None,
                status_text: None,
                token_usage: None,
            }],
            collapsed: false,
            mode: VibeMode::Vibeless,
            review: false,
            plan_mode: false,
            agent: AgentKind::Claude,
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Idle,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
        };
        let project = Project {
            id: "proj-1".into(),
            name: "demo".into(),
            repo: PathBuf::from("/tmp/demo"),
            collapsed: false,
            features: vec![feature.clone()],
            created_at: now,
            preferred_agent: AgentKind::Claude,
            is_git: false,
        };
        let mut app = App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![project],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.pending_inputs.push(PendingInput {
            session_id: "amf-feature".into(),
            cwd: "/tmp/demo".into(),
            message: "Review the change before continuing.".into(),
            notification_type: "diff-review".into(),
            file_path: PathBuf::new(),
            target_file_path: Some("src/main.rs".into()),
            relative_path: Some("src/main.rs".into()),
            change_id: None,
            tool: Some("Edit".into()),
            old_snippet: None,
            new_snippet: None,
            original_file: None,
            proposed_file: None,
            is_new_file: None,
            reason: None,
            response_file: None,
            project_name: Some("demo".into()),
            feature_name: Some("feature".into()),
            proceed_signal: None,
            request_id: None,
            reply_socket: None,
        });

        let view = ViewState::new(
            "demo".into(),
            "feature".into(),
            "amf-feature".into(),
            "claude".into(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            false,
        );

        let sidebar = build_agent_sidebar_data(&app, &view).unwrap();
        assert_eq!(
            sidebar.work_text.as_deref(),
            Some(
                "State: waiting for diff review\nRequest: Review the change before continuing.\nHint: use leader V if the review prompt is not appearing."
            )
        );
    }

    #[test]
    fn pending_change_reason_updates_claude_sidebar_work_text() {
        let now = chrono::Utc::now();
        let feature = Feature {
            id: "feat-1".into(),
            name: "feature".into(),
            branch: "feature".into(),
            workdir: PathBuf::from("/tmp/demo"),
            is_worktree: false,
            tmux_session: "amf-feature".into(),
            sessions: vec![FeatureSession {
                id: "session-1".into(),
                kind: SessionKind::Claude,
                label: "Claude".into(),
                tmux_window: "claude".into(),
                claude_session_id: Some("claude-session".into()),
                token_usage_source: None,
                token_usage_source_match: None,
                created_at: now,
                command: None,
                on_stop: None,
                pre_check: None,
                status_text: None,
                token_usage: None,
            }],
            collapsed: false,
            mode: VibeMode::Vibeless,
            review: false,
            plan_mode: false,
            agent: AgentKind::Claude,
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Idle,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
        };
        let project = Project {
            id: "proj-1".into(),
            name: "demo".into(),
            repo: PathBuf::from("/tmp/demo"),
            collapsed: false,
            features: vec![feature.clone()],
            created_at: now,
            preferred_agent: AgentKind::Claude,
            is_git: false,
        };
        let mut app = App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![project],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.pending_inputs.push(PendingInput {
            session_id: "amf-feature".into(),
            cwd: "/tmp/demo".into(),
            message: "".into(),
            notification_type: "change-reason".into(),
            file_path: PathBuf::new(),
            target_file_path: Some("src/main.rs".into()),
            relative_path: Some("src/main.rs".into()),
            change_id: None,
            tool: Some("Edit".into()),
            old_snippet: None,
            new_snippet: None,
            original_file: None,
            proposed_file: None,
            is_new_file: None,
            reason: None,
            response_file: None,
            project_name: Some("demo".into()),
            feature_name: Some("feature".into()),
            proceed_signal: None,
            request_id: None,
            reply_socket: None,
        });

        let view = ViewState::new(
            "demo".into(),
            "feature".into(),
            "amf-feature".into(),
            "claude".into(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            false,
        );

        let sidebar = build_agent_sidebar_data(&app, &view).unwrap();
        assert_eq!(
            sidebar.work_text.as_deref(),
            Some(
                "State: waiting for change reason\nRequest: Explain why this change is needed.\nHint: use leader V if the review prompt is not appearing."
            )
        );
    }

    #[test]
    fn pending_diff_review_updates_opencode_sidebar_work_text() {
        let now = chrono::Utc::now();
        let feature = Feature {
            id: "feat-1".into(),
            name: "feature".into(),
            branch: "feature".into(),
            workdir: PathBuf::from("/tmp/demo"),
            is_worktree: false,
            tmux_session: "amf-feature".into(),
            sessions: vec![FeatureSession {
                id: "session-1".into(),
                kind: SessionKind::Opencode,
                label: "Opencode".into(),
                tmux_window: "opencode".into(),
                claude_session_id: None,
                token_usage_source: None,
                token_usage_source_match: None,
                created_at: now,
                command: None,
                on_stop: None,
                pre_check: None,
                status_text: None,
                token_usage: None,
            }],
            collapsed: false,
            mode: VibeMode::Vibeless,
            review: false,
            plan_mode: false,
            agent: AgentKind::Opencode,
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Idle,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
        };
        let project = Project {
            id: "proj-1".into(),
            name: "demo".into(),
            repo: PathBuf::from("/tmp/demo"),
            collapsed: false,
            features: vec![feature.clone()],
            created_at: now,
            preferred_agent: AgentKind::Opencode,
            is_git: false,
        };
        let mut app = App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![project],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.pending_inputs.push(PendingInput {
            session_id: "amf-feature".into(),
            cwd: "/tmp/demo".into(),
            message: "Review the change before continuing.".into(),
            notification_type: "diff-review".into(),
            file_path: PathBuf::new(),
            target_file_path: Some("src/main.rs".into()),
            relative_path: Some("src/main.rs".into()),
            change_id: None,
            tool: Some("Edit".into()),
            old_snippet: None,
            new_snippet: None,
            original_file: None,
            proposed_file: None,
            is_new_file: None,
            reason: None,
            response_file: None,
            project_name: Some("demo".into()),
            feature_name: Some("feature".into()),
            proceed_signal: None,
            request_id: None,
            reply_socket: None,
        });

        let view = ViewState::new(
            "demo".into(),
            "feature".into(),
            "amf-feature".into(),
            "opencode".into(),
            "Opencode".into(),
            SessionKind::Opencode,
            VibeMode::Vibeless,
            false,
        );

        let sidebar = build_agent_sidebar_data(&app, &view).unwrap();
        assert_eq!(
            sidebar.work_text.as_deref(),
            Some(
                "State: waiting for diff review\nRequest: Review the change before continuing.\nHint: use leader V if the review prompt is not appearing."
            )
        );
    }

    #[test]
    fn build_agent_sidebar_data_still_builds_for_codex_with_plan_sources_present() {
        let now = chrono::Utc::now();
        let feature = Feature {
            id: "feat-1".into(),
            name: "feature".into(),
            branch: "feature".into(),
            workdir: PathBuf::from("/tmp/demo"),
            is_worktree: false,
            tmux_session: "amf-feature".into(),
            sessions: vec![codex_feature_session("sess-current")],
            collapsed: false,
            mode: VibeMode::Vibeless,
            review: false,
            plan_mode: false,
            agent: AgentKind::Codex,
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Idle,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
        };
        let project = Project {
            id: "proj-1".into(),
            name: "demo".into(),
            repo: PathBuf::from("/tmp/demo"),
            collapsed: false,
            features: vec![feature],
            created_at: now,
            preferred_agent: AgentKind::Codex,
            is_git: false,
        };
        let mut app = App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![project],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.sidebar_model_cache
            .insert("amf-feature".into(), "Model: gpt-5.5".into());
        app.sidebar_plan_cache
            .insert("amf-feature".into(), "Plan\n1. Inspect\n2. Patch".into());
        app.apply_codex_live_event(
            "amf-feature",
            &serde_json::json!({
                "type": "plan",
                "payload": { "text": "1. Inspect\n2. Patch" }
            }),
        );

        let view = ViewState::new(
            "demo".into(),
            "feature".into(),
            "amf-feature".into(),
            "codex".into(),
            "Codex".into(),
            SessionKind::Codex,
            VibeMode::Vibeless,
            false,
        );

        let sidebar = build_agent_sidebar_data(&app, &view).unwrap();
        assert_eq!(sidebar.agent_kind, SessionKind::Codex);
        assert!(sidebar.status_text.contains("Model: gpt-5.5"));
    }
}
