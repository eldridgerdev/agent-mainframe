use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, CreateFeatureStep, CreateProjectStep};
use crate::project::{AgentKind, VibeMode};

pub fn handle_create_project_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('b') {
        let is_path_step = matches!(
            &app.mode,
            AppMode::CreatingProject(s)
                if matches!(s.step, CreateProjectStep::Path)
        );
        if is_path_step {
            let browse = std::mem::replace(&mut app.mode, AppMode::Normal);
            if let AppMode::CreatingProject(state) = browse {
                app.start_browse_path(state);
            }
            return Ok(());
        }
    }

    match key.code {
        KeyCode::Esc => {
            app.cancel_create();
        }
        KeyCode::Enter => {
            let step = match &app.mode {
                AppMode::CreatingProject(state) => state.step.clone(),
                _ => return Ok(()),
            };

            match step {
                CreateProjectStep::Name => {
                    if let AppMode::CreatingProject(state) = &mut app.mode {
                        state.step = CreateProjectStep::Path;
                    }
                }
                CreateProjectStep::Path => {
                    app.create_project()?;
                }
            }
        }
        KeyCode::Tab => {
            if let AppMode::CreatingProject(state) = &mut app.mode {
                state.step = match state.step {
                    CreateProjectStep::Name => CreateProjectStep::Path,
                    CreateProjectStep::Path => CreateProjectStep::Name,
                };
            }
        }
        KeyCode::Backspace => {
            if let AppMode::CreatingProject(state) = &mut app.mode {
                match state.step {
                    CreateProjectStep::Name => {
                        state.name.pop();
                    }
                    CreateProjectStep::Path => {
                        state.path.pop();
                    }
                }
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::CreatingProject(state) = &mut app.mode {
                match state.step {
                    CreateProjectStep::Name => state.name.push(c),
                    CreateProjectStep::Path => state.path.push(c),
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_create_feature_key(app: &mut App, key: KeyCode) -> Result<()> {
    let step = match &app.mode {
        AppMode::CreatingFeature(state) => state.step.clone(),
        _ => return Ok(()),
    };

    match step {
        CreateFeatureStep::Source => {
            // Max source options:
            //   0 = new branch
            //   1 = existing worktree
            //   2 = use preset (only if presets exist)
            let preset_count =
                app.active_extension.feature_presets.len();
            let source_options = if preset_count > 0 { 3 } else { 2 };
            match key {
                KeyCode::Esc => {
                    app.cancel_create();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let AppMode::CreatingFeature(state) =
                        &mut app.mode
                    {
                        state.source_index = (state.source_index
                            + 1)
                            % source_options;
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let AppMode::CreatingFeature(state) =
                        &mut app.mode
                    {
                        state.source_index =
                            if state.source_index == 0 {
                                source_options - 1
                            } else {
                                state.source_index - 1
                            };
                    }
                }
                KeyCode::Enter => {
                    if let AppMode::CreatingFeature(state) =
                        &mut app.mode
                    {
                        state.step = match state.source_index {
                            0 => CreateFeatureStep::Branch,
                            1 => CreateFeatureStep::ExistingWorktree,
                            _ => CreateFeatureStep::SelectPreset,
                        };
                    }
                }
                _ => {}
            }
        }
        CreateFeatureStep::ExistingWorktree => match key {
            KeyCode::Esc => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.step = CreateFeatureStep::Source;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    let len = state.worktrees.len();
                    if len > 0 {
                        state.worktree_index = (state.worktree_index + 1) % len;
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    let len = state.worktrees.len();
                    if len > 0 {
                        state.worktree_index = if state.worktree_index == 0 {
                            len - 1
                        } else {
                            state.worktree_index - 1
                        };
                    }
                }
            }
            KeyCode::Enter => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    if state.worktrees.is_empty() {
                        app.message = Some("No available worktrees".into());
                    } else if let Some(wt) = state.worktrees.get(state.worktree_index) {
                        state.branch = wt.branch.clone().unwrap_or_else(|| {
                            wt.path
                                .file_name()
                                .map(|n: &std::ffi::OsStr| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        });
                        state.step = CreateFeatureStep::Mode;
                    }
                }
            }
            _ => {}
        },
        CreateFeatureStep::SelectPreset => match key {
            KeyCode::Esc => {
                if let AppMode::CreatingFeature(state) =
                    &mut app.mode
                {
                    state.step = CreateFeatureStep::Source;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::CreatingFeature(state) =
                    &mut app.mode
                {
                    let len = app
                        .active_extension
                        .feature_presets
                        .len();
                    if len > 0 {
                        state.preset_index =
                            (state.preset_index + 1) % len;
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let AppMode::CreatingFeature(state) =
                    &mut app.mode
                {
                    let len = app
                        .active_extension
                        .feature_presets
                        .len();
                    if len > 0 {
                        state.preset_index =
                            if state.preset_index == 0 {
                                len - 1
                            } else {
                                state.preset_index - 1
                            };
                    }
                }
            }
            KeyCode::Enter => {
                let preset_index = match &app.mode {
                    AppMode::CreatingFeature(s) => {
                        s.preset_index
                    }
                    _ => return Ok(()),
                };
                let preset = app
                    .active_extension
                    .feature_presets
                    .get(preset_index)
                    .cloned();
                if let Some(preset) = preset
                    && let AppMode::CreatingFeature(state) =
                        &mut app.mode
                {
                    // Pre-fill fields from preset.
                    state.mode = preset.mode;
                    state.agent = preset.agent.clone();
                    state.review = preset.review;
                    state.enable_chrome = preset.enable_chrome;
                    state.enable_notes = preset.enable_notes;
                    if let Some(ref prefix) = preset.branch_prefix
                        && !prefix.is_empty()
                    {
                        state.branch = format!("{}/", prefix);
                    }
                    state.step = CreateFeatureStep::Branch;
                }
            }
            _ => {}
        },
        CreateFeatureStep::Branch => match key {
            KeyCode::Esc => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    if state.worktrees.is_empty() {
                        app.cancel_create();
                    } else {
                        state.step = CreateFeatureStep::Source;
                    }
                } else {
                    app.cancel_create();
                }
            }
            KeyCode::Enter => {
                let empty = match &app.mode {
                    AppMode::CreatingFeature(s) => s.branch.is_empty(),
                    _ => return Ok(()),
                };
                if empty {
                    app.message = Some("Branch name cannot be empty".into());
                } else if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.step = CreateFeatureStep::Worktree;
                }
            }
            KeyCode::Backspace => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.branch.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.branch.push(c);
                }
            }
            _ => {}
        },
        CreateFeatureStep::Worktree => match key {
            KeyCode::Esc => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.step = CreateFeatureStep::Branch;
                }
            }
            KeyCode::Tab | KeyCode::Enter | KeyCode::Char('l') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.step = CreateFeatureStep::Mode;
                }
            }
            KeyCode::Down | KeyCode::Up | KeyCode::Char('j') | KeyCode::Char('k') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.use_worktree = !state.use_worktree;
                }
            }
            _ => {}
        },
        CreateFeatureStep::Mode => match key {
            KeyCode::Esc => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    if state.mode_focus > 0 {
                        state.mode_focus -= 1;
                    } else if state.source_index == 1 && !state.worktrees.is_empty() {
                        state.step = CreateFeatureStep::ExistingWorktree;
                    } else {
                        state.step = CreateFeatureStep::Worktree;
                    }
                }
            }
            KeyCode::Tab | KeyCode::Char('l') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    let max_focus = if state.agent == AgentKind::Claude {
                        4
                    } else {
                        3
                    };
                    if state.mode_focus < max_focus {
                        state.mode_focus += 1;
                    }
                }
            }
            KeyCode::BackTab | KeyCode::Char('h') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode
                    && state.mode_focus > 0
                {
                    state.mode_focus -= 1;
                }
            }
            KeyCode::Enter => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    let max_focus = if state.agent == AgentKind::Claude {
                        4
                    } else {
                        3
                    };
                    if state.mode_focus < max_focus {
                        state.mode_focus += 1;
                    } else {
                        let is_supervibe = matches!(state.mode, VibeMode::SuperVibe);
                        if is_supervibe {
                            state.step = CreateFeatureStep::ConfirmSuperVibe;
                        } else {
                            app.create_feature()?;
                        }
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    match state.mode_focus {
                        0 => {
                            state.agent_index = (state.agent_index + 1) % AgentKind::ALL.len();
                            state.agent = AgentKind::ALL[state.agent_index].clone();
                        }
                        1 => {
                            state.mode_index = (state.mode_index + 1) % VibeMode::ALL.len();
                            state.mode = VibeMode::ALL[state.mode_index].clone();
                        }
                        2 => {
                            state.review = !state.review;
                        }
                        3 => {
                            if state.agent == AgentKind::Claude {
                                state.enable_chrome = !state.enable_chrome;
                            } else {
                                state.enable_notes = !state.enable_notes;
                            }
                        }
                        4 => {
                            state.enable_notes = !state.enable_notes;
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    match state.mode_focus {
                        0 => {
                            state.agent_index = if state.agent_index == 0 {
                                AgentKind::ALL.len() - 1
                            } else {
                                state.agent_index - 1
                            };
                            state.agent = AgentKind::ALL[state.agent_index].clone();
                        }
                        1 => {
                            state.mode_index = if state.mode_index == 0 {
                                VibeMode::ALL.len() - 1
                            } else {
                                state.mode_index - 1
                            };
                            state.mode = VibeMode::ALL[state.mode_index].clone();
                        }
                        2 => {
                            state.review = !state.review;
                        }
                        3 => {
                            if state.agent == AgentKind::Claude {
                                state.enable_chrome = !state.enable_chrome;
                            } else {
                                state.enable_notes = !state.enable_notes;
                            }
                        }
                        4 => {
                            state.enable_notes = !state.enable_notes;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        },
        CreateFeatureStep::ConfirmSuperVibe => match key {
            KeyCode::Char('y') => {
                app.create_feature()?;
            }
            KeyCode::Char('n') | KeyCode::Esc => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.step = CreateFeatureStep::Mode;
                }
            }
            _ => {}
        },
    }
    Ok(())
}

pub fn handle_help_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
            let from_view = match std::mem::replace(
                &mut app.mode,
                AppMode::Normal,
            ) {
                AppMode::Help(v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            if let Some(view) = from_view {
                app.mode = AppMode::Viewing(view);
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_delete_project_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('y') => {
            app.delete_project()?;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_delete_feature_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('y') => {
            app.delete_feature()?;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_browse_path_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let creating_folder = match &app.mode {
        AppMode::BrowsingPath(state) => state.creating_folder,
        _ => false,
    };

    if creating_folder {
        match key.code {
            KeyCode::Esc => {
                if let AppMode::BrowsingPath(state) = &mut app.mode {
                    state.creating_folder = false;
                    state.new_folder_name.clear();
                }
            }
            KeyCode::Enter => {
                app.create_folder_in_browse()?;
            }
            KeyCode::Backspace => {
                if let AppMode::BrowsingPath(state) = &mut app.mode {
                    state.new_folder_name.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::BrowsingPath(state) = &mut app.mode {
                    state.new_folder_name.push(c);
                }
            }
            _ => {}
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => {
            app.cancel_browse_path();
        }
        KeyCode::Tab => {
            app.cancel_browse_path();
            if let AppMode::CreatingProject(state) = &mut app.mode {
                state.step = CreateProjectStep::Name;
            }
        }
        KeyCode::Char(' ') => {
            app.confirm_browse_path();
        }
        KeyCode::Char('c') => {
            if let AppMode::BrowsingPath(state) = &mut app.mode {
                state.creating_folder = true;
            }
        }
        KeyCode::Enter => {
            let is_dir = match &app.mode {
                AppMode::BrowsingPath(state) => state.explorer.current().is_dir(),
                _ => false,
            };
            if is_dir {
                if let AppMode::BrowsingPath(state) = &mut app.mode {
                    let _ = state.explorer.handle(&Event::Key(key));
                }
            } else {
                app.confirm_browse_path();
            }
        }
        _ => {
            if let AppMode::BrowsingPath(state) = &mut app.mode {
                let _ = state.explorer.handle(&Event::Key(key));
            }
        }
    }
    Ok(())
}

pub fn handle_rename_session_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.cancel_rename_session();
        }
        KeyCode::Enter => {
            app.apply_rename_session()?;
        }
        KeyCode::Backspace => {
            if let AppMode::RenamingSession(state) = &mut app.mode {
                state.input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::RenamingSession(state) = &mut app.mode {
                state.input.push(c);
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_running_hook_key(app: &mut App, key: KeyCode) -> Result<()> {
    let is_running = match &app.mode {
        AppMode::RunningHook(state) => state.child.is_some(),
        _ => return Ok(()),
    };

    if is_running {
        return Ok(());
    }

    match key {
        KeyCode::Enter | KeyCode::Esc => {
            app.complete_running_hook()?;
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_deleting_feature_key(app: &mut App, key: KeyCode) -> Result<()> {
    let (is_running, is_completed) = match &app.mode {
        AppMode::DeletingFeatureInProgress(state) => {
            (state.child.is_some(), state.stage == crate::app::DeleteStage::Completed)
        }
        _ => return Ok(()),
    };

    if is_running {
        return Ok(());
    }

    match key {
        KeyCode::Enter | KeyCode::Esc => {
            if is_completed {
                app.complete_deleting_feature()?;
            } else {
                app.cancel_deleting_feature();
            }
        }
        _ => {
            if is_completed {
                app.complete_deleting_feature()?;
            }
        }
    }
    Ok(())
}

pub fn handle_latest_prompt_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let view = match std::mem::replace(
                &mut app.mode,
                AppMode::Normal,
            ) {
                AppMode::LatestPrompt(_, v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            app.mode = AppMode::Viewing(view);
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_hook_prompt_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::HookPrompt(state) = &mut app.mode {
                let len = state.options.len();
                if len > 0 {
                    state.selected = (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::HookPrompt(state) = &mut app.mode {
                let len = state.options.len();
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
            app.confirm_hook_prompt()?;
        }
        _ => {}
    }
    Ok(())
}
