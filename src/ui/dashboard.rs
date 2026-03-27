use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::Block,
};

use crate::app::{App, AppMode, CreateFeatureStep, RenameReturnTo};
use crate::project::{Feature, FeatureSession, Project, SessionKind, TokenUsageSourceMatch};
use crate::token_tracking::{TokenUsageProvider, TokenUsageSource};

const SIDEBAR_PROMPT_PREVIEW_COLS: usize = 32;
const SIDEBAR_PROMPT_PREVIEW_LINES: usize = 2;
const SIDEBAR_SUMMARY_PREVIEW_COLS: usize = 32;
const SIDEBAR_SUMMARY_PREVIEW_LINES: usize = 3;
const SIDEBAR_WORK_VALUE_CHARS: usize = 28;
const SIDEBAR_TODO_VALUE_CHARS: usize = 26;

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

    if sidebar_kind == SessionKind::Opencode {
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
        let activity_line = if opencode_sidebar
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

        return Some(super::pane::AgentSidebarData {
            agent_kind: SessionKind::Opencode,
            status_text: opencode_sidebar_status_text(activity_line, usage_line, opencode_sidebar),
            prompt_text,
            work_text,
            todos_text,
            summary_text,
        });
    }

    match sidebar_kind {
        SessionKind::Claude | SessionKind::Codex => {}
        _ => return None,
    };

    let usage_line = session
        .and_then(|session| session.status_text.as_deref())
        .map(format_sidebar_usage);
    let prompt_text = sidebar_prompt_text(
        codex_sidebar_source(&sidebar_kind, session)
            .and_then(|source| app.cached_codex_session_prompt(&feature.workdir, &source.id)),
        app.latest_prompt_for_session(&feature.tmux_session),
    );
    let summary_text = if app.summary_state.generating.contains(&feature.tmux_session) {
        Some("Generating summary...".to_string())
    } else {
        feature.summary.clone()
    };
    let codex_live = if sidebar_kind == SessionKind::Codex {
        app.codex_live_thread(&feature.tmux_session)
    } else {
        None
    };
    let work_text = codex_live
        .and_then(|live| live.sidebar_work_text())
        .or_else(|| fallback_sidebar_work_text(app, project, feature, view));
    let summary_text = compose_sidebar_summary_text(
        codex_live.and_then(|live| live.summary_prefix()),
        summary_text,
    );
    let activity_line = sidebar_status_activity_text(work_text.is_some(), status_line);
    let usage_confidence = format_codex_usage_source_confidence(&sidebar_kind, session);

    let status_text = compose_sidebar_status_text(activity_line, usage_line, usage_confidence);

    Some(super::pane::AgentSidebarData {
        agent_kind: sidebar_kind,
        status_text,
        prompt_text,
        work_text,
        todos_text: None,
        summary_text,
    })
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
        .unwrap_or_else(|| sidebar.todo_preview.len() as u64);
    if todo_count == 0 && sidebar.todo_preview.is_empty() {
        return None;
    }

    let preview_count = sidebar.todo_preview.len().min(2) as u64;
    let mut lines = vec![format!(
        "Open: {todo_count} item{}",
        if todo_count == 1 { "" } else { "s" }
    )];
    if let Some(first) = sidebar.todo_preview.first() {
        lines.push(format!(
            "Next: {}",
            compact_sidebar_text(first, SIDEBAR_TODO_VALUE_CHARS)
        ));
    }
    if let Some(second) = sidebar.todo_preview.get(1) {
        lines.push(format!(
            "Then: {}",
            compact_sidebar_text(second, SIDEBAR_TODO_VALUE_CHARS)
        ));
    }
    if todo_count > preview_count {
        let hidden_count = todo_count - preview_count;
        lines.push(format!(
            "More: {hidden_count} more item{}",
            if hidden_count == 1 { "" } else { "s" }
        ));
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
        .map(|prompt| format!("leader l\nPreview: {}", compact_sidebar_text(&prompt, 48)))
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
                "Agent is waiting for input"
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

fn draw_view_pane(frame: &mut Frame, app: &App, view: &crate::app::ViewState, leader_active: bool) {
    let sidebar_data = build_agent_sidebar_data(app, view);
    super::pane::draw(
        frame,
        view,
        &app.pane_content,
        sidebar_data.as_ref(),
        leader_active,
        app.pending_inputs.len(),
        app.tmux_cursor,
        &app.theme,
    );
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    frame.render_widget(
        Block::default().style(Style::default().bg(app.theme.effective_bg())),
        frame.area(),
    );

    if let AppMode::Viewing(view) = &app.mode {
        let area = frame.area();
        draw_view_pane(frame, app, view, app.leader_active);
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
        draw_view_pane(frame, app, &temp_view, false);
        super::picker::draw_session_switcher(frame, state, app.config.nerd_font, &app.theme);
        return;
    }

    if let AppMode::Help(Some(view)) = &app.mode {
        draw_view_pane(frame, app, view, false);
        super::dialogs::draw_help(frame, &app.theme);
        return;
    }

    if let AppMode::NotificationPicker(selected, Some(view)) = &app.mode {
        draw_view_pane(frame, app, view, false);
        super::picker::draw_notification_picker(frame, &app.pending_inputs, *selected, &app.theme);
        return;
    }

    if let AppMode::LatestPrompt(state) = &app.mode {
        draw_view_pane(frame, app, &state.view, false);
        super::dialogs::draw_latest_prompt_dialog(frame, state, app.message.as_deref(), &app.theme);
        return;
    }

    if let AppMode::DiffViewer(state) = &app.mode {
        draw_view_pane(frame, app, &state.from_view, false);
        super::dialogs::draw_diff_viewer(frame, state, &app.theme);
        return;
    }

    let markdown_from_view = if let AppMode::MarkdownViewer(state) = &app.mode {
        state.from_view.clone()
    } else {
        None
    };
    if let Some(view) = markdown_from_view.as_ref() {
        draw_view_pane(frame, app, view, false);
    }
    if let AppMode::MarkdownViewer(state) = &mut app.mode {
        super::dialogs::draw_markdown_viewer(frame, state, &app.theme);
        return;
    }
    let steering_from_view = if let AppMode::SteeringPrompt(state) = &app.mode {
        Some(state.view.clone())
    } else {
        None
    };
    if let Some(view) = steering_from_view.as_ref() {
        draw_view_pane(frame, app, view, false);
    }
    if let AppMode::SteeringPrompt(state) = &mut app.mode {
        super::dialogs::draw_steering_prompt_dialog(frame, state, &app.theme);
        return;
    }

    if let AppMode::CommandPicker(state) = &app.mode
        && state.from_view.is_some()
    {
        let view = state.from_view.as_ref().unwrap();
        draw_view_pane(frame, app, view, false);
        super::picker::draw_command_picker(frame, state, &app.theme);
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
        draw_view_pane(frame, app, view, false);
        super::picker::draw_markdown_file_picker(frame, state, &app.theme);
        return;
    }

    if let AppMode::BookmarkPicker(state) = &app.mode
        && state.from_view.is_some()
    {
        let view = state.from_view.as_ref().unwrap();
        draw_view_pane(frame, app, view, false);
        let rows = app.bookmark_picker_rows();
        super::picker::draw_bookmark_picker(frame, state, &rows, &app.theme);
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
        draw_view_pane(frame, app, &temp_view, false);
        super::dialogs::draw_rename_session_dialog(frame, state, &app.theme);
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
        app.pending_inputs.len(),
        &app.theme,
    );
    super::list::draw(frame, app, chunks[1]);
    super::status::draw(frame, app, chunks[2]);

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
                let presets = app.active_extension.allowed_feature_presets();
                let allowed_agents = app.active_extension.allowed_agents();
                super::dialogs::draw_create_feature_dialog(
                    frame,
                    state,
                    presets.as_slice(),
                    allowed_agents.as_slice(),
                    &app.theme,
                );
            }
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

    if matches!(app.mode, AppMode::Help(None)) {
        super::dialogs::draw_help(frame, &app.theme);
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

    if let AppMode::DebugLog(state) = &app.mode {
        super::dialogs::draw_debug_log(frame, &app.debug_log, state.scroll_offset, &app.theme);
    }
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
        }
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

        assert!(prompt.contains("leader l"));
        assert!(prompt.contains("fallback prompt"));
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

        assert_eq!(
            prompt,
            "leader l\nPreview: This is a much longer prompt preview that shoul…"
        );
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
    fn format_codex_usage_source_confidence_uses_inferred_match_label() {
        let mut session = codex_feature_session("sess-current");
        session.token_usage_source_match = Some(TokenUsageSourceMatch::Inferred);

        assert_eq!(
            format_codex_usage_source_confidence(&SessionKind::Codex, Some(&session)),
            Some("Usage source: inferred workdir match".to_string())
        );
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
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
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
    }
}
