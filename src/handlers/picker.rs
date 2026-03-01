use anyhow::Result;
use crossterm::event::KeyCode;

use crate::app::{App, AppMode};
use crate::project::SessionKind;
use crate::tmux::TmuxManager;

pub fn handle_command_picker_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let old_mode = std::mem::replace(
                &mut app.mode,
                AppMode::Normal,
            );
            if let AppMode::CommandPicker(state) = old_mode
                && let Some(view) = state.from_view
            {
                app.mode = AppMode::Viewing(view);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::CommandPicker(ref mut state) =
                app.mode
            {
                let len = state.commands.len();
                if len > 0 {
                    state.selected =
                        (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::CommandPicker(ref mut state) =
                app.mode
            {
                let len = state.commands.len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
        }
        KeyCode::Enter => {
            let old_mode = std::mem::replace(
                &mut app.mode,
                AppMode::Normal,
            );
            if let AppMode::CommandPicker(state) = old_mode {
                let selected_name = state
                    .commands
                    .get(state.selected)
                    .map(|c| c.name.clone());

                if let Some(name) = selected_name {
                    let command_text =
                        format!("/{}", name);

                    let tmux_info =
                        if let Some(ref view) =
                            state.from_view
                        {
                            Some((
                                view.session.clone(),
                                view.window.clone(),
                            ))
                        } else if let Some((_, feature)) =
                            app.selected_feature()
                        {
                            let window = feature
                                .sessions
                                .iter()
                                .find(|s| {
                                    s.kind
                                        == SessionKind::Claude
                                })
                                .map(|s| {
                                    s.tmux_window.clone()
                                })
                                .unwrap_or_else(|| {
                                    "claude".into()
                                });
                            Some((
                                feature
                                    .tmux_session
                                    .clone(),
                                window,
                            ))
                        } else {
                            None
                        };

                    if let Some((session, window)) =
                        &tmux_info
                    {
                        let _ =
                            TmuxManager::send_literal(
                                session,
                                window,
                                &command_text,
                            );
                        let _ =
                            TmuxManager::send_key_name(
                                session,
                                window,
                                "Enter",
                            );
                        app.message = Some(format!(
                            "Sent '{}'",
                            command_text
                        ));
                    } else {
                        app.message = Some(
                            "No active session to send to"
                                .into(),
                        );
                    }
                }

                if let Some(view) = state.from_view {
                    app.mode = AppMode::Viewing(view);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_notification_picker_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let from_view = match std::mem::replace(
                &mut app.mode,
                AppMode::Normal,
            ) {
                AppMode::NotificationPicker(_, v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            if let Some(view) = from_view {
                app.mode = AppMode::Viewing(view);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::NotificationPicker(ref mut idx, _) =
                app.mode
            {
                let len = app.pending_inputs.len();
                if len > 0 {
                    *idx = (*idx + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::NotificationPicker(ref mut idx, _) =
                app.mode
            {
                let len = app.pending_inputs.len();
                if len > 0 {
                    *idx =
                        if *idx == 0 { len - 1 } else { *idx - 1 };
                }
            }
        }
        KeyCode::Enter => {
            app.handle_notification_select()?;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_session_switcher_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_session_switcher();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::SessionSwitcher(ref mut state) =
                app.mode
            {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected =
                        (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::SessionSwitcher(ref mut state) =
                app.mode
            {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
        }
        KeyCode::Enter => {
            app.switch_from_switcher();
        }
        KeyCode::Char('r') => {
            app.start_rename_from_switcher();
        }
        KeyCode::Char('s') => {
            app.open_session_picker_from_switcher()?;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_opencode_session_picker_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_opencode_session_picker();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::OpencodeSessionPicker(ref mut state) =
                app.mode
            {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected =
                        (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::OpencodeSessionPicker(ref mut state) =
                app.mode
            {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
        }
        KeyCode::Enter => {
            app.confirm_opencode_session();
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_opencode_session_confirm_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('n') => {
            app.cancel_opencode_session_confirm();
        }
        KeyCode::Char('y') => {
            app.confirm_and_start_opencode()?;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_session_picker_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let old_mode = std::mem::replace(
                &mut app.mode,
                AppMode::Normal,
            );
            if let AppMode::SessionPicker(state) = old_mode
                && let Some(view) = state.from_view
            {
                app.mode = AppMode::Viewing(view);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::SessionPicker(ref mut state) = app.mode
            {
                let total = state.builtin_sessions.len()
                    + state.custom_sessions.len();
                if total > 0 {
                    let start = state.selected;
                    loop {
                        state.selected = (state.selected + 1) % total;
                        if state.selected == start {
                            break;
                        }
                        if state.selected < state.builtin_sessions.len() {
                            if state.builtin_sessions[state.selected].disabled.is_none() {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::SessionPicker(ref mut state) = app.mode
            {
                let total = state.builtin_sessions.len()
                    + state.custom_sessions.len();
                if total > 0 {
                    let start = state.selected;
                    loop {
                        state.selected = if state.selected == 0 {
                            total - 1
                        } else {
                            state.selected - 1
                        };
                        if state.selected == start {
                            break;
                        }
                        if state.selected < state.builtin_sessions.len() {
                            if state.builtin_sessions[state.selected].disabled.is_none() {
                                break;
                            }
                        } else {
                            break;
                        }
                    }
                }
            }
        }
        KeyCode::Enter => {
            let old_mode = std::mem::replace(
                &mut app.mode,
                AppMode::Normal,
            );
            if let AppMode::SessionPicker(state) = old_mode {
                let builtin_len = state.builtin_sessions.len();
                if state.selected < builtin_len {
                    let builtin = &state.builtin_sessions[state.selected];
                    if let Some(ref reason) = builtin.disabled {
                        app.message = Some(format!("Cannot start: {}", reason));
                        app.mode = AppMode::SessionPicker(state);
                        return Ok(());
                    }
                    match app.add_builtin_session(
                        state.pi,
                        state.fi,
                        builtin.kind.clone(),
                    ) {
                        Ok(()) => {
                            app.message = Some(format!(
                                "Added '{}'",
                                builtin.label
                            ));
                        }
                        Err(e) => {
                            app.message = Some(format!(
                                "Error: {}",
                                e
                            ));
                        }
                    }
                } else {
                    let custom_idx = state.selected - builtin_len;
                    if let Some(cfg) =
                        state.custom_sessions.get(custom_idx).cloned()
                    {
                        match app.add_custom_session_type(
                            state.pi,
                            state.fi,
                            &cfg,
                        ) {
                            Ok(()) => {
                                app.message = Some(format!(
                                    "Added '{}'",
                                    cfg.name
                                ));
                            }
                            Err(e) => {
                                app.message = Some(format!(
                                    "Error: {}",
                                    e
                                ));
                            }
                        }
                    }
                }
                if let Some(view) = state.from_view {
                    app.mode = AppMode::Viewing(view);
                }
            }
        }
        _ => {}
    }
    Ok(())
}
