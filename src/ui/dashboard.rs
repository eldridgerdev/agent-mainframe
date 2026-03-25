use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::Block,
};

use crate::app::{App, AppMode, CreateFeatureStep, RenameReturnTo};
use crate::project::SessionKind;

fn build_sidebar_data(app: &App, view: &crate::app::ViewState) -> Option<super::pane::SidebarData> {
    if !view.has_sidebar() {
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
                .find(|session| session.kind == view.session_kind)
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
    let opencode_sidebar = app.opencode_sidebar_cache.get(&feature.tmux_session);
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
    let usage_line = session
        .and_then(|session| session.status_text.as_deref())
        .map(format_sidebar_usage)
        .unwrap_or_else(|| "Usage: unavailable".to_string());
    let prompt_text = match view.session_kind {
        SessionKind::Opencode => opencode_sidebar
            .and_then(|sidebar| sidebar.latest_prompt.as_deref())
            .or_else(|| app.latest_prompt_for_session(&feature.tmux_session))
            .map(|prompt| format!("Preview: {}", compact_sidebar_text(prompt, 120)))
            .unwrap_or_else(|| "No recent prompt available yet.".to_string()),
        _ => app
            .latest_prompt_for_session(&feature.tmux_session)
            .map(|prompt| format!("Preview: {}", compact_sidebar_text(prompt, 120)))
            .unwrap_or_else(|| {
                "No recent prompt.\nUse leader+l to open prompt history.".to_string()
            }),
    };
    let summary_text = sidebar_summary_text(
        &view.session_kind,
        app.summary_state.generating.contains(&feature.tmux_session),
        feature.summary.as_deref(),
        opencode_sidebar,
    );

    Some(super::pane::SidebarData {
        title: sidebar_title(&view.session_kind),
        title_color: sidebar_title_color(&view.session_kind, &app.theme),
        status_text: status_text(
            activity_line.as_str(),
            usage_line.as_str(),
            opencode_sidebar,
        ),
        work_text: work_text(opencode_sidebar),
        prompt_text,
        todos_text: todos_text(opencode_sidebar),
        summary_text,
    })
}

fn status_text(
    activity_line: &str,
    usage_line: &str,
    opencode_sidebar: Option<&crate::app::opencode_storage::OpencodeSidebarData>,
) -> String {
    let mut lines = vec![format!("Activity: {activity_line}")];
    if usage_line != "Usage: unavailable" {
        lines.push(usage_line.to_string());
    }
    if let Some(reasoning_tokens) = opencode_sidebar
        .and_then(|sidebar| sidebar.reasoning_tokens)
        .filter(|tokens| *tokens > 0)
    {
        lines.push(format!(
            "Reasoning: {} tokens",
            crate::token_tracking::format_token_count(reasoning_tokens)
        ));
    }
    if let Some(change_line) = opencode_sidebar.and_then(|sidebar| sidebar.change_summary_line()) {
        lines.push(change_line);
    }
    lines.join("\n")
}

fn work_text(
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
        lines.push(format!("Permission: {permission}"));
    }
    if let Some(lsp_summary) = opencode_sidebar
        .and_then(|sidebar| sidebar.lsp_summary.as_deref())
        .filter(|summary| !summary.is_empty())
    {
        lines.push(format!("LSP: {}", compact_sidebar_text(lsp_summary, 72)));
    }
    if let Some(error) = opencode_sidebar
        .and_then(|sidebar| sidebar.last_error.as_deref())
        .filter(|error| !error.is_empty())
    {
        lines.push(format!("Error: {}", compact_sidebar_text(error, 72)));
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

fn todos_text(
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
        lines.push(format!("Next: {}", compact_sidebar_text(first, 20)));
    }
    if let Some(second) = sidebar.todo_preview.get(1) {
        lines.push(format!("Then: {}", compact_sidebar_text(second, 20)));
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

fn sidebar_summary_text(
    session_kind: &SessionKind,
    generating: bool,
    feature_summary: Option<&str>,
    opencode_sidebar: Option<&crate::app::opencode_storage::OpencodeSidebarData>,
) -> Option<String> {
    if generating {
        return Some("Generating summary...".to_string());
    }

    if *session_kind == SessionKind::Opencode
        && let Some(summary) = opencode_sidebar
            .and_then(|sidebar| sidebar.live_summary.as_deref())
            .filter(|summary| !summary.is_empty())
    {
        return Some(compact_sidebar_text(summary, 80));
    }

    feature_summary
        .map(str::to_string)
}

fn sidebar_title(session_kind: &SessionKind) -> &'static str {
    match session_kind {
        SessionKind::Opencode => " Opencode Sidebar ",
        _ => " Claude Sidebar ",
    }
}

fn sidebar_title_color(
    session_kind: &SessionKind,
    theme: &crate::theme::Theme,
) -> ratatui::style::Color {
    match session_kind {
        SessionKind::Opencode => theme.session_icon_opencode.to_color(),
        _ => theme.session_icon_claude.to_color(),
    }
}

fn compact_sidebar_text(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        return compact;
    }

    let truncated: String = compact.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{truncated}…")
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
    let sidebar_data = build_sidebar_data(app, view);
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
    fn sidebar_usage_falls_back_when_format_is_unknown() {
        assert_eq!(
            format_sidebar_usage("tokens unavailable"),
            "Usage: tokens unavailable"
        );
    }

    #[test]
    fn opencode_status_text_includes_reasoning_tokens() {
        let status = status_text(
            "Thinking",
            "Input: 16.0k tokens",
            Some(&crate::app::opencode_storage::OpencodeSidebarData {
                session_id: "ses-1".into(),
                title: None,
                latest_prompt: None,
                status: None,
                last_tool: None,
                todo_count: None,
                todo_preview: Vec::new(),
                pending_permission: None,
                last_error: None,
                lsp_summary: None,
                live_summary: None,
                reasoning_tokens: Some(4200),
                additions: Some(10),
                deletions: Some(3),
                files: Some(2),
            }),
        );

        assert_eq!(
            status,
            "Activity: Thinking\nInput: 16.0k tokens\nReasoning: 4.2k tokens\nChanges: 2 files · +10 / -3"
        );
    }

    #[test]
    fn opencode_work_text_shows_live_sidecar_details() {
        let work = work_text(Some(&crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "ses-1".into(),
            title: None,
            latest_prompt: None,
            status: Some("busy".into()),
            last_tool: Some("edit".into()),
            todo_count: Some(3),
            todo_preview: Vec::new(),
            pending_permission: Some("edit".into()),
            lsp_summary: Some("ready · 2 warnings".into()),
            last_error: Some("patch failed".into()),
            live_summary: None,
            reasoning_tokens: None,
            additions: None,
            deletions: None,
            files: None,
        }));

        assert_eq!(
            work.as_deref(),
            Some("State: busy\nTool: edit\nPermission: edit\nLSP: ready · 2 warnings\nError: patch failed")
        );
    }

    #[test]
    fn opencode_todos_text_renders_count() {
        let todos = todos_text(Some(&crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "ses-1".into(),
            title: None,
            latest_prompt: None,
            status: None,
            last_tool: None,
            todo_count: Some(3),
            todo_preview: vec!["finish parser".into(), "wire UI".into()],
            pending_permission: None,
            last_error: None,
            lsp_summary: None,
            live_summary: None,
            reasoning_tokens: None,
            additions: None,
            deletions: None,
            files: None,
        }));

        assert_eq!(
            todos.as_deref(),
            Some("Open: 3 items\nNext: finish parser\nThen: wire UI\nMore: 1 more item")
        );
    }

    #[test]
    fn opencode_todos_text_keeps_full_count_with_short_preview() {
        let todos = todos_text(Some(&crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "ses-1".into(),
            title: None,
            latest_prompt: None,
            status: None,
            last_tool: None,
            todo_count: Some(5),
            todo_preview: vec!["finish parser".into(), "wire UI".into(), "add tests".into()],
            pending_permission: None,
            last_error: None,
            lsp_summary: None,
            live_summary: None,
            reasoning_tokens: None,
            additions: None,
            deletions: None,
            files: None,
        }));

        assert_eq!(
            todos.as_deref(),
            Some("Open: 5 items\nNext: finish parser\nThen: wire UI\nMore: 3 more items")
        );
    }

    #[test]
    fn opencode_summary_prefers_live_sidecar_summary() {
        let live = crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "ses-1".into(),
            title: None,
            latest_prompt: None,
            status: None,
            last_tool: None,
            todo_count: None,
            todo_preview: Vec::new(),
            pending_permission: None,
            last_error: None,
            lsp_summary: None,
            live_summary: Some("Live assistant summary".into()),
            reasoning_tokens: None,
            additions: None,
            deletions: None,
            files: None,
        };

        assert_eq!(
            sidebar_summary_text(
                &SessionKind::Opencode,
                false,
                Some("Persisted AMF summary"),
                Some(&live),
            ),
            Some("Live assistant summary".into())
        );
    }

    #[test]
    fn summary_text_falls_back_to_feature_summary_without_live_opencode_summary() {
        assert_eq!(
            sidebar_summary_text(&SessionKind::Opencode, false, Some("Persisted AMF summary"), None),
            Some("Persisted AMF summary".into())
        );
    }

    #[test]
    fn summary_text_prioritizes_generating_state_over_live_summary() {
        let live = crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "ses-1".into(),
            title: None,
            latest_prompt: None,
            status: None,
            last_tool: None,
            todo_count: None,
            todo_preview: Vec::new(),
            pending_permission: None,
            last_error: None,
            lsp_summary: None,
            live_summary: Some("Live assistant summary".into()),
            reasoning_tokens: None,
            additions: None,
            deletions: None,
            files: None,
        };

        assert_eq!(
            sidebar_summary_text(
                &SessionKind::Opencode,
                true,
                Some("Persisted AMF summary"),
                Some(&live),
            ),
            Some("Generating summary...".into())
        );
    }

    #[test]
    fn summary_text_is_absent_without_live_or_persisted_summary() {
        assert_eq!(
            sidebar_summary_text(&SessionKind::Opencode, false, None, None),
            None
        );
    }

    #[test]
    fn status_text_omits_unavailable_usage_line() {
        assert_eq!(status_text("Ready", "Usage: unavailable", None), "Activity: Ready");
    }
}
