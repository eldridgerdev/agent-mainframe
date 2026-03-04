use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};

use crate::app::{App, AppMode, CreateFeatureStep, RenameReturnTo};

pub fn draw(frame: &mut Frame, app: &mut App) {
    if let AppMode::Viewing(view) = &app.mode {
        let area = frame.area();
        super::pane::draw(
            frame,
            view,
            &app.pane_content,
            app.leader_active,
            app.pending_inputs.len(),
            app.tmux_cursor,
            &app.theme,
        );
        // Show transient message (e.g. "Copied N chars") on the bottom line
        if let Some(ref msg) = app.message {
            let msg_area = Rect::new(
                area.x,
                area.y + area.height.saturating_sub(1),
                area.width,
                1,
            );
            let color = if msg.starts_with("Error:") {
                ratatui::style::Color::Red
            } else {
                ratatui::style::Color::Green
            };
            let paragraph = ratatui::widgets::Paragraph::new(
                ratatui::text::Span::styled(
                    format!(" {}", msg),
                    ratatui::style::Style::default().fg(color),
                ),
            );
            frame.render_widget(paragraph, msg_area);
        }
        return;
    }

    if let AppMode::SessionSwitcher(state) = &app.mode {
        let temp_view = crate::app::ViewState::new(
            state.project_name.clone(),
            state.feature_name.clone(),
            state.tmux_session.clone(),
            state.return_window.clone(),
            state.return_label.clone(),
            state.vibe_mode.clone(),
            state.review,
        );
        super::pane::draw(
            frame,
            &temp_view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
            app.tmux_cursor,
            &app.theme,
        );
        super::picker::draw_session_switcher(
            frame,
            state,
            app.config.nerd_font,
        );
        return;
    }

    if let AppMode::Help(Some(view)) = &app.mode {
        super::pane::draw(
            frame,
            view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
            app.tmux_cursor,
            &app.theme,
        );
        super::dialogs::draw_help(frame);
        return;
    }

    if let AppMode::NotificationPicker(selected, Some(view)) =
        &app.mode
    {
        super::pane::draw(
            frame,
            view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
            app.tmux_cursor,
            &app.theme,
        );
        super::picker::draw_notification_picker(
            frame,
            &app.pending_inputs,
            *selected,
        );
        return;
    }

    if let AppMode::LatestPrompt(prompt, view) = &app.mode {
        super::pane::draw(
            frame,
            view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
            app.tmux_cursor,
            &app.theme,
        );
        super::dialogs::draw_latest_prompt_dialog(frame, prompt);
        return;
    }

    if let AppMode::CommandPicker(state) = &app.mode
        && state.from_view.is_some()
    {
        let view = state.from_view.as_ref().unwrap();
        super::pane::draw(
            frame,
            view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
            app.tmux_cursor,
            &app.theme,
        );
        super::picker::draw_command_picker(frame, state);
        return;
    }

    if let AppMode::RenamingSession(state) = &app.mode
        && let RenameReturnTo::SessionSwitcher(ref sw) =
            state.return_to
    {
        let temp_view = crate::app::ViewState::new(
            sw.project_name.clone(),
            sw.feature_name.clone(),
            sw.tmux_session.clone(),
            sw.return_window.clone(),
            sw.return_label.clone(),
            sw.vibe_mode.clone(),
            sw.review,
        );
        super::pane::draw(
            frame,
            &temp_view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
            app.tmux_cursor,
            &app.theme,
        );
        super::dialogs::draw_rename_session_dialog(frame, state);
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

    super::header::draw(frame, chunks[0], &std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), app.pending_inputs.len(), &app.theme);
    super::list::draw(frame, app, chunks[1]);
    super::status::draw(frame, app, chunks[2]);

    match &app.mode {
        AppMode::CreatingProject(state) => {
            super::dialogs::draw_create_project_dialog(frame, state);
        }
        AppMode::CreatingFeature(state) => {
            if state.step == CreateFeatureStep::ConfirmSuperVibe {
                super::dialogs::draw_confirm_supervibe_dialog(frame);
            } else {
                let presets =
                    app.active_extension.feature_presets.as_slice();
                super::dialogs::draw_create_feature_dialog(
                    frame, state, presets,
                );
            }
        }
        AppMode::DeletingProject(name) => {
            super::dialogs::draw_delete_project_confirm(frame, name);
        }
        AppMode::DeletingFeature(
            project_name,
            feature_name,
        ) => {
            super::dialogs::draw_delete_feature_confirm(
                frame,
                project_name,
                feature_name,
            );
        }
        AppMode::BrowsingPath(state) => {
            super::dialogs::draw_browse_path_dialog(frame, state);
        }
        _ => {}
    }

    if let AppMode::RenamingSession(state) = &app.mode {
        super::dialogs::draw_rename_session_dialog(frame, state);
    }

    if matches!(app.mode, AppMode::Help(None)) {
        super::dialogs::draw_help(frame);
    }

    if let AppMode::NotificationPicker(selected, None) = &app.mode {
        super::picker::draw_notification_picker(
            frame,
            &app.pending_inputs,
            *selected,
        );
    }

    if let AppMode::CommandPicker(state) = &app.mode {
        super::picker::draw_command_picker(frame, state);
    }

    if let AppMode::Searching(state) = &app.mode {
        super::dialogs::draw_search_dialog(frame, state);
    }

    if let AppMode::OpencodeSessionPicker(state) = &app.mode {
        super::picker::draw_opencode_session_picker(frame, state);
    }

    if matches!(app.mode, AppMode::ConfirmingOpencodeSession { .. }) {
        super::picker::draw_opencode_session_confirm(frame);
    }

    if let AppMode::ClaudeSessionPicker(state) = &app.mode {
        super::picker::draw_claude_session_picker(frame, state);
    }

    if matches!(app.mode, AppMode::ConfirmingClaudeSession { .. }) {
        super::picker::draw_claude_session_confirm(frame);
    }

    if let AppMode::SessionPicker(state) = &app.mode {
        super::picker::draw_session_picker(frame, state, app.config.nerd_font);
    }

    if let AppMode::ChangeReasonPrompt(state) = &app.mode {
        super::dialogs::draw_change_reason_dialog(frame, state);
    }

    if let AppMode::RunningHook(state) = &app.mode {
        super::dialogs::draw_running_hook_dialog(frame, state, &app.throbber_state);
    }

    if let AppMode::DeletingFeatureInProgress(state) = &app.mode {
        super::dialogs::draw_deleting_feature_dialog(frame, state, &app.throbber_state);
    }

    if let AppMode::HookPrompt(state) = &app.mode {
        super::dialogs::draw_hook_prompt_dialog(frame, state);
    }

    if let AppMode::ForkingFeature(state) = &app.mode {
        super::dialogs::draw_fork_feature_dialog(frame, state);
    }

    if let AppMode::ThemePicker(state) = &app.mode {
        super::dialogs::draw_theme_picker(
            frame, state, &app.config.theme,
        );
    }

    if let AppMode::DebugLog(state) = &app.mode {
        super::dialogs::draw_debug_log(frame, &app.debug_log, state.scroll_offset);
    }
}

pub fn centered_rect(
    percent_x: u16,
    percent_y: u16,
    area: Rect,
) -> Rect {
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
}
