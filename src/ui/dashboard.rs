use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    Frame,
};

use crate::app::{App, AppMode, CreateFeatureStep, RenameReturnTo};

pub fn draw(frame: &mut Frame, app: &App) {
    if let AppMode::Viewing(view) = &app.mode {
        super::pane::draw(
            frame,
            view,
            &app.pane_content,
            app.leader_active,
            app.pending_inputs.len(),
        );
        return;
    }

    if let AppMode::SessionSwitcher(state) = &app.mode {
        let temp_view = crate::app::ViewState {
            project_name: state.project_name.clone(),
            feature_name: state.feature_name.clone(),
            session: state.tmux_session.clone(),
            window: state.return_window.clone(),
            session_label: state.return_label.clone(),
            vibe_mode: state.vibe_mode.clone(),
        };
        super::pane::draw(
            frame,
            &temp_view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
        );
        super::picker::draw_session_switcher(
            frame,
            state,
            app.config.nerd_font,
        );
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
        );
        super::picker::draw_command_picker(frame, state);
        return;
    }

    if let AppMode::RenamingSession(state) = &app.mode
        && let RenameReturnTo::SessionSwitcher(ref sw) =
            state.return_to
    {
        let temp_view = crate::app::ViewState {
            project_name: sw.project_name.clone(),
            feature_name: sw.feature_name.clone(),
            session: sw.tmux_session.clone(),
            window: sw.return_window.clone(),
            session_label: sw.return_label.clone(),
            vibe_mode: sw.vibe_mode.clone(),
        };
        super::pane::draw(
            frame,
            &temp_view,
            &app.pane_content,
            false,
            app.pending_inputs.len(),
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

    super::header::draw(frame, chunks[0], app.pending_inputs.len());
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
                super::dialogs::draw_create_feature_dialog(frame, state);
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

    if matches!(app.mode, AppMode::Help) {
        super::dialogs::draw_help(frame);
    }

    if let AppMode::NotificationPicker(selected) = &app.mode
    {
        super::picker::draw_notification_picker(
            frame,
            &app.pending_inputs,
            *selected,
        );
    }

    if let AppMode::CommandPicker(state) = &app.mode {
        super::picker::draw_command_picker(frame, state);
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
