use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::Block,
};

use crate::app::{App, AppMode, CreateFeatureStep, RenameReturnTo};
use chrono::{DateTime, Datelike, Utc};

fn build_claude_sidebar_data(
    app: &App,
    view: &crate::app::ViewState,
) -> Option<super::pane::ClaudeSidebarData> {
    if !view.has_claude_sidebar() {
        return None;
    }

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
                .find(|session| matches!(session.kind, crate::project::SessionKind::Claude))
        });
    let matching_inputs: Vec<_> = app
        .pending_inputs
        .iter()
        .filter(|input| {
            input.session_id == view.session
                || (input.project_name.as_deref() == Some(project.name.as_str())
                    && input.feature_name.as_deref() == Some(feature.name.as_str()))
        })
        .collect();
    let waiting_count = matching_inputs.len();
    let status_line = match waiting_count {
        0 => "Ready".to_string(),
        1 => "Waiting for 1 input".to_string(),
        n => format!("Waiting for {n} inputs"),
    };
    let activity_line = if app.ipc_tool_sessions.contains(&feature.tmux_session) {
        "Running tool".to_string()
    } else if app.is_feature_thinking(&feature.tmux_session) {
        "Thinking".to_string()
    } else {
        status_line
    };
    let tool_line = app
        .active_tool_for_session(&feature.tmux_session)
        .map(|tool| format!("Tool: {tool}"));
    let request_line = matching_inputs.iter().find_map(|input| {
        let message = input.message.trim();
        if message.is_empty() {
            None
        } else {
            Some(format!("Request: {}", compact_sidebar_text(message, 60)))
        }
    });
    let usage_line = session
        .and_then(|session| session.status_text.as_deref())
        .map(format_sidebar_usage)
        .unwrap_or_else(|| "Usage: unavailable".to_string());
    let task_text = app
        .task_state_for_session(&feature.tmux_session)
        .map(format_sidebar_task)
        .unwrap_or_else(|| "No task data yet.".to_string());
    let prompt_text = app
        .latest_prompt_for_session(&feature.tmux_session)
        .map(|prompt| {
            format!(
                "At: {}\nPreview:\n{}",
                format_prompt_timestamp(prompt.timestamp),
                format_prompt_preview(&prompt.text, 24, 4)
            )
        })
        .unwrap_or_else(|| "No recent prompt.\nUse leader+l to open prompt history.".to_string());
    Some(super::pane::ClaudeSidebarData {
        status_text: format_sidebar_status(
            &activity_line,
            tool_line.as_deref(),
            request_line.as_deref(),
            &usage_line,
        ),
        task_text,
        prompt_text,
    })
}

fn compact_sidebar_text(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }

    let truncated: String = compact.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
}

fn format_prompt_timestamp(timestamp: Option<i64>) -> String {
    timestamp
        .and_then(|ts| DateTime::<Utc>::from_timestamp(ts, 0))
        .map(|dt| {
            if dt.year() == Utc::now().year() {
                dt.format("%b %-d %-I:%M %p").to_string()
            } else {
                dt.format("%b %-d %Y %-I:%M %p").to_string()
            }
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn format_prompt_preview(text: &str, width: usize, max_lines: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() || width == 0 || max_lines == 0 {
        return String::new();
    }

    let words: Vec<&str> = compact.split(' ').collect();
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut index = 0;

    while index < words.len() {
        let word = words[index];
        let separator = if current.is_empty() { 0 } else { 1 };

        if current.len() + separator + word.len() <= width {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
            index += 1;
            continue;
        }

        if current.is_empty() {
            let mut chunk = word
                .chars()
                .take(width.saturating_sub(1))
                .collect::<String>();
            chunk.push('…');
            lines.push(chunk);
            return lines.join("\n");
        }

        lines.push(current);
        current = String::new();
        if lines.len() == max_lines {
            break;
        }
    }

    if !current.is_empty() && lines.len() < max_lines {
        lines.push(current);
    }

    if index < words.len()
        && let Some(last) = lines.last_mut()
    {
        if last.chars().count() >= width && width > 1 {
            last.pop();
        }
        if !last.ends_with('…') {
            last.push('…');
        }
    }

    lines.join("\n")
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

fn format_sidebar_task(task_state: &crate::app::util::ClaudeTaskState) -> String {
    let total = task_state.tasks.len();
    if total == 0 {
        return "No task data yet.".to_string();
    }

    let completed = task_state.completed_count();
    let active = usize::from(task_state.current_task().is_some());
    let pending = task_state.pending_count();

    let mut lines = vec![format!(
        "{total} tasks ({completed} done, {active} active, {pending} open)"
    )];

    for task in &task_state.tasks {
        let prefix = match task.status.as_str() {
            "completed" => "[x]",
            "in_progress" => "[>]",
            "pending" => "[ ]",
            _ => "[?]",
        };
        lines.push(format!(
            "{prefix} {}",
            compact_sidebar_text(&task.subject, 46)
        ));
        if task.status == "in_progress"
            && let Some(active_form) = task.active_form.as_deref()
        {
            lines.push(format!("    {}", compact_sidebar_text(active_form, 44)));
        }
    }

    lines.join("\n")
}

fn format_sidebar_status(
    activity: &str,
    tool: Option<&str>,
    request: Option<&str>,
    usage: &str,
) -> String {
    let mut lines = vec![format!("Activity: {activity}")];
    if let Some(tool) = tool.filter(|tool| !tool.trim().is_empty()) {
        lines.push(tool.to_string());
    }
    if let Some(request) = request.filter(|request| !request.trim().is_empty()) {
        lines.push(request.to_string());
    }
    lines.extend(usage.lines().map(str::to_string));
    lines.join("\n")
}

fn draw_view_pane(frame: &mut Frame, app: &App, view: &crate::app::ViewState, leader_active: bool) {
    let sidebar_data = build_claude_sidebar_data(app, view);
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
    if let AppMode::SteeringPrompt(state) = &app.mode {
        draw_view_pane(frame, app, &state.view, false);
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
    use ratatui::layout::Rect;

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
    fn prompt_timestamp_is_formatted_as_utc_time() {
        assert_eq!(format_prompt_timestamp(Some(0)), "Jan 1 1970 12:00 AM");
    }

    #[test]
    fn prompt_timestamp_handles_missing_values() {
        assert_eq!(format_prompt_timestamp(None), "unknown");
    }

    #[test]
    fn sidebar_usage_falls_back_when_format_is_unknown() {
        assert_eq!(
            format_sidebar_usage("tokens unavailable"),
            "Usage: tokens unavailable"
        );
    }

    #[test]
    fn prompt_preview_wraps_before_truncating() {
        assert_eq!(
            format_prompt_preview("call a tool so i can see this wrap better", 12, 4),
            "call a tool\nso i can see\nthis wrap\nbetter"
        );
    }

    #[test]
    fn sidebar_status_includes_active_tool_line() {
        assert_eq!(
            format_sidebar_status(
                "Running tool",
                Some("Tool: Edit"),
                None,
                "Input: 16.0k tokens\nOutput: 2.0k tokens"
            ),
            "Activity: Running tool\nTool: Edit\nInput: 16.0k tokens\nOutput: 2.0k tokens"
        );
    }

    #[test]
    fn sidebar_status_includes_request_line() {
        assert_eq!(
            format_sidebar_status(
                "Waiting for 1 input",
                None,
                Some("Request: Need user answer"),
                "Input: 16.0k tokens"
            ),
            "Activity: Waiting for 1 input\nRequest: Need user answer\nInput: 16.0k tokens"
        );
    }

    #[test]
    fn sidebar_task_renders_todo_list_with_active_item() {
        let task_state = crate::app::util::ClaudeTaskState {
            tasks: vec![
                crate::app::util::ClaudeTask {
                    id: "1".into(),
                    subject: "Explore sidebar".into(),
                    description: None,
                    active_form: None,
                    status: "completed".into(),
                },
                crate::app::util::ClaudeTask {
                    id: "2".into(),
                    subject: "Implement task sidebar".into(),
                    description: None,
                    active_form: Some("Updating sidebar rendering".into()),
                    status: "in_progress".into(),
                },
                crate::app::util::ClaudeTask {
                    id: "3".into(),
                    subject: "Run tests".into(),
                    description: None,
                    active_form: None,
                    status: "pending".into(),
                },
            ],
        };

        assert_eq!(
            format_sidebar_task(&task_state),
            "3 tasks (1 done, 1 active, 1 open)\n[x] Explore sidebar\n[>] Implement task sidebar\n    Updating sidebar rendering\n[ ] Run tests"
        );
    }
}
