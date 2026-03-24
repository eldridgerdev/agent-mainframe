use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::Block,
};

use crate::app::{App, AppMode, CreateFeatureStep, RenameReturnTo};
use crate::project::{FeatureSession, SessionKind};
use crate::token_tracking::TokenUsageProvider;

fn build_agent_sidebar_data(
    app: &App,
    view: &crate::app::ViewState,
) -> Option<super::pane::AgentSidebarData> {
    let sidebar_kind = view.sidebar_session_kind()?;

    let (agent_label, fallback_session_label) = match sidebar_kind {
        SessionKind::Claude => ("Claude", "Claude"),
        SessionKind::Codex => ("Codex", "Codex"),
        _ => return None,
    };

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
        .or_else(|| feature.sessions.iter().find(|session| session.kind == sidebar_kind));

    let feature_label = feature.nickname.as_deref().unwrap_or(&feature.name);
    let mode_label = if view.review {
        "Review".to_string()
    } else if feature.plan_mode {
        format!("{} + Plan", view.vibe_mode.display_name())
    } else {
        view.vibe_mode.display_name().to_string()
    };
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
    let activity_line = if app.ipc_tool_sessions.contains(&feature.tmux_session) {
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
    let session_line = session_sidebar_line(session, fallback_session_label);
    let prompt_text = app
        .latest_prompt_for_session(&feature.tmux_session)
        .map(|prompt| format!("Preview: {}", compact_sidebar_text(prompt, 120)))
        .unwrap_or_else(|| "No recent prompt.\nUse leader+l to open prompt history.".to_string());
    let summary_text = if app.summary_state.generating.contains(&feature.tmux_session) {
        "Generating summary...".to_string()
    } else {
        feature
            .summary
            .clone()
            .unwrap_or_else(|| "No summary yet. Use leader+g to generate one.".to_string())
    };
    let session_metadata = format_codex_session_metadata(&sidebar_kind, session, &feature.workdir);
    let session_text = match session_metadata {
        Some(metadata) => format!(
            "Target: {}/{}\nAgent: {}\nSession: {}\n{}\nMode: {}\nBranch: {}",
            project.name, feature_label, agent_label, session_line, metadata, mode_label, feature.branch
        ),
        None => format!(
            "Target: {}/{}\nAgent: {}\nSession: {}\nMode: {}\nBranch: {}",
            project.name, feature_label, agent_label, session_line, mode_label, feature.branch
        ),
    };

    Some(super::pane::AgentSidebarData {
        agent_kind: sidebar_kind,
        session_text,
        status_text: format!("Activity: {}\n{}", activity_line, usage_line),
        prompt_text,
        summary_text,
    })
}

fn session_sidebar_line(session: Option<&FeatureSession>, fallback_label: &str) -> String {
    session
        .map(|session| session.label.clone())
        .unwrap_or_else(|| fallback_label.to_string())
}

fn format_codex_session_metadata(
    sidebar_kind: &SessionKind,
    session: Option<&FeatureSession>,
    workdir: &std::path::Path,
) -> Option<String> {
    if *sidebar_kind != SessionKind::Codex {
        return None;
    }

    let source = session
        .and_then(|session| session.token_usage_source.as_ref())
        .filter(|source| source.provider == TokenUsageProvider::Codex)?;
    let short_id = shorten_session_id(&source.id);

    let title = crate::app::codex_session_info_for_workdir(workdir, &source.id)
        .ok()
        .flatten()
        .map(|info| info.title)
        .filter(|title| !title.trim().is_empty() && title != "Untitled");

    Some(match title {
        Some(title) => format!("Thread: {short_id}\nTitle: {title}"),
        None => format!("Thread: {short_id}"),
    })
}

fn shorten_session_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
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
    use crate::token_tracking::{TokenUsageProvider, TokenUsageSource};
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
    fn shorten_session_id_truncates_long_ids() {
        assert_eq!(shorten_session_id("1234567890abcdef"), "12345678");
    }

    #[test]
    fn format_codex_session_metadata_returns_none_for_non_codex_sidebar() {
        let session = FeatureSession {
            id: "session-1".into(),
            kind: SessionKind::Codex,
            label: "Codex".into(),
            tmux_window: "codex".into(),
            claude_session_id: None,
            token_usage_source: Some(TokenUsageSource {
                provider: TokenUsageProvider::Codex,
                id: "1234567890abcdef".into(),
            }),
            created_at: chrono::Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
        };

        assert_eq!(
            format_codex_session_metadata(
                &SessionKind::Claude,
                Some(&session),
                std::path::Path::new("/tmp/unused")
            ),
            None
        );
    }
}
