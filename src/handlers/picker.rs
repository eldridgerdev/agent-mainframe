use anyhow::Result;
use crossterm::event::KeyCode;

use crate::app::{App, AppMode, Selection};
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
        KeyCode::Char('x') | KeyCode::Delete => {
            if let AppMode::NotificationPicker(ref mut idx, _) =
                app.mode
            {
                let i = *idx;
                if i < app.pending_inputs.len() {
                    let input = app.pending_inputs.remove(i);
                    let _ =
                        std::fs::remove_file(&input.file_path);
                    app.message =
                        Some("Input request deleted".into());
                    if app.pending_inputs.is_empty() {
                        let from_view =
                            match std::mem::replace(
                                &mut app.mode,
                                AppMode::Normal,
                            ) {
                                AppMode::NotificationPicker(
                                    _,
                                    v,
                                ) => v,
                                other => {
                                    app.mode = other;
                                    return Ok(());
                                }
                            };
                        if let Some(view) = from_view {
                            app.mode =
                                AppMode::Viewing(view);
                        }
                    } else if *idx >= app.pending_inputs.len()
                    {
                        *idx = app.pending_inputs.len() - 1;
                    }
                }
            }
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

pub fn handle_claude_session_picker_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_claude_session_picker();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::ClaudeSessionPicker(ref mut state) =
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
            if let AppMode::ClaudeSessionPicker(ref mut state) =
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
            app.confirm_claude_session();
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_claude_session_confirm_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('n') => {
            app.cancel_claude_session_confirm();
        }
        KeyCode::Char('y') => {
            app.confirm_and_start_claude()?;
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
                        // Resolve working directory for the
                        // pre_check (same logic as session_ops).
                        let check_dir = app
                            .store
                            .projects
                            .get(state.pi)
                            .and_then(|p| p.features.get(state.fi))
                            .map(|f| {
                                cfg.working_dir
                                    .as_ref()
                                    .map(|rel| f.workdir.join(rel))
                                    .unwrap_or_else(|| {
                                        f.workdir.clone()
                                    })
                            });
                        let pre_ok = match &check_dir {
                            Some(dir) => cfg.run_pre_check(dir),
                            None => Ok(()),
                        };
                        if let Err(reason) = pre_ok {
                            app.message =
                                Some(format!("{}: {}", cfg.name, reason));
                        } else {
                            match app.add_custom_session_type(
                                state.pi,
                                state.fi,
                                &cfg,
                            ) {
                                Ok(autolaunch) => {
                                    app.message = Some(format!(
                                        "Added '{}'",
                                        cfg.name
                                    ));
                                    if autolaunch {
                                        // Point selection to the newly added session
                                        // (last in the sessions list).
                                        if let Some(feature) = app.store.projects
                                            .get(state.pi)
                                            .and_then(|p| p.features.get(state.fi))
                                        {
                                            let si = feature.sessions.len().saturating_sub(1);
                                            app.selection = Selection::Session(state.pi, state.fi, si);
                                        }
                                        let _ = app.enter_view();
                                    }
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

pub fn handle_bookmark_picker_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let old_mode = std::mem::replace(
                &mut app.mode,
                AppMode::Normal,
            );
            if let AppMode::BookmarkPicker(state) = old_mode
                && let Some(view) = state.from_view
            {
                app.mode = AppMode::Viewing(view);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::BookmarkPicker(ref mut state) =
                app.mode
            {
                let len = app.store.session_bookmarks.len();
                if len > 0 {
                    state.selected =
                        (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::BookmarkPicker(ref mut state) =
                app.mode
            {
                let len = app.store.session_bookmarks.len();
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
            let slot = if let AppMode::BookmarkPicker(state) =
                &app.mode
            {
                if app.store.session_bookmarks.is_empty() {
                    app.message =
                        Some("No bookmarks yet".into());
                    return Ok(());
                }
                state.selected + 1
            } else {
                return Ok(());
            };
            app.jump_to_bookmark(slot)?;
        }
        KeyCode::Char('d') | KeyCode::Delete => {
            let slot = if let AppMode::BookmarkPicker(state) =
                &app.mode
            {
                if app.store.session_bookmarks.is_empty() {
                    app.message =
                        Some("No bookmarks to remove".into());
                    return Ok(());
                }
                state.selected + 1
            } else {
                return Ok(());
            };
            app.remove_bookmark_slot(slot)?;
            if let AppMode::BookmarkPicker(state) = &mut app.mode {
                let len = app.store.session_bookmarks.len();
                if len == 0 {
                    state.selected = 0;
                } else if state.selected >= len {
                    state.selected = len - 1;
                }
            }
        }
        KeyCode::Char(c @ '1'..='9') => {
            let slot = (c as u8 - b'0') as usize;
            app.jump_to_bookmark(slot)?;
        }
        _ => {}
    }
    Ok(())
}
