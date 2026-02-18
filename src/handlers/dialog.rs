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
        CreateFeatureStep::Source => match key {
            KeyCode::Esc => {
                app.cancel_create();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.source_index = (state.source_index + 1) % 2;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.source_index = if state.source_index == 0 { 1 } else { 0 };
                }
            }
            KeyCode::Enter => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    if state.source_index == 0 {
                        state.step = CreateFeatureStep::Branch;
                    } else {
                        state.step = CreateFeatureStep::ExistingWorktree;
                    }
                }
            }
            _ => {}
        },
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
                    if let Some(wt) = state.worktrees.get(state.worktree_index) {
                        state.branch = wt.branch.clone().unwrap_or_else(|| {
                            wt.path
                                .file_name()
                                .map(|n: &std::ffi::OsStr| n.to_string_lossy().into_owned())
                                .unwrap_or_default()
                        });
                    }
                    state.step = CreateFeatureStep::Mode;
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
                    if state.source_index == 1 && !state.worktrees.is_empty() {
                        state.step = CreateFeatureStep::ExistingWorktree;
                    } else {
                        state.step = CreateFeatureStep::Branch;
                    }
                }
            }
            KeyCode::Enter => {
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
                    if state.source_index == 1 && !state.worktrees.is_empty() {
                        state.step = CreateFeatureStep::ExistingWorktree;
                    } else {
                        state.step = CreateFeatureStep::Worktree;
                    }
                }
            }
            KeyCode::Enter => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    if state.mode_focus < 2 {
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
            app.mode = AppMode::Normal;
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
