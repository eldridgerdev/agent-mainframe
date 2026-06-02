use anyhow::Result;
use crossterm::event::KeyCode;

use crate::app::{App, AppMode, CreateFeatureStep};
use crate::project::{AgentKind, VibeMode, normalized_feature_name};

fn branch_error(app: &App) -> Option<String> {
    let state = match &app.mode {
        AppMode::CreatingFeature(state) => state,
        _ => return None,
    };

    let branch = state.branch.trim();
    if branch.is_empty() {
        return Some("Feature name cannot be empty".to_string());
    }

    let project = app.store.find_project(&state.project_name)?;
    let normalized_branch = normalized_feature_name(branch);
    if project
        .features
        .iter()
        .any(|feature| normalized_feature_name(&feature.name) == normalized_branch)
    {
        return Some(format!(
            "Feature '{}' already exists in '{}'",
            state.branch, state.project_name
        ));
    }

    None
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
            let preset_count = match &app.mode {
                AppMode::CreatingFeature(state) => state.feature_presets.len(),
                _ => 0,
            };
            let source_options = if preset_count > 0 { 3 } else { 2 };
            match key {
                KeyCode::Esc => {
                    app.cancel_create();
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if let AppMode::CreatingFeature(state) = &mut app.mode {
                        state.source_index = (state.source_index + 1) % source_options;
                    }
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if let AppMode::CreatingFeature(state) = &mut app.mode {
                        state.source_index = if state.source_index == 0 {
                            source_options - 1
                        } else {
                            state.source_index - 1
                        };
                    }
                }
                KeyCode::Enter => {
                    if let AppMode::CreatingFeature(state) = &mut app.mode {
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
                        app.push_toast_warning("No available worktrees");
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
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.step = CreateFeatureStep::Source;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    let len = state.feature_presets.len();
                    if len > 0 {
                        state.preset_index = (state.preset_index + 1) % len;
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    let len = state.feature_presets.len();
                    if len > 0 {
                        state.preset_index = if state.preset_index == 0 {
                            len - 1
                        } else {
                            state.preset_index - 1
                        };
                    }
                }
            }
            KeyCode::Enter => {
                let preset_index = match &app.mode {
                    AppMode::CreatingFeature(s) => s.preset_index,
                    _ => return Ok(()),
                };
                let preset = match &app.mode {
                    AppMode::CreatingFeature(state) => {
                        state.feature_presets.get(preset_index).cloned()
                    }
                    _ => None,
                };
                if let Some(preset) = preset
                    && let AppMode::CreatingFeature(state) = &mut app.mode
                {
                    // Pre-fill fields from preset.
                    state.mode = preset.mode;
                    state.agent = preset.agent.clone();
                    state.review = preset.review;
                    state.plan_mode = preset.plan_mode;
                    state.enable_chrome = preset.enable_chrome;
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
                if let Some(error) = branch_error(app) {
                    if let AppMode::CreatingFeature(state) = &mut app.mode {
                        state.branch_error = Some(error.clone());
                    }
                    app.push_toast_warning(error);
                } else if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.branch_error = None;
                    state.step = CreateFeatureStep::Worktree;
                }
            }
            KeyCode::Backspace => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.branch.pop();
                    state.branch_error = None;
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.branch.push(c);
                    state.branch_error = None;
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
                        5
                    } else {
                        4
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
                        5
                    } else {
                        4
                    };
                    if state.mode_focus < max_focus {
                        state.mode_focus += 1;
                    } else {
                        let is_supervibe = matches!(state.mode, VibeMode::SuperVibe);
                        if is_supervibe {
                            state.step = CreateFeatureStep::ConfirmSuperVibe;
                        } else {
                            state.session_name = App::default_session_name_for_agent(&state.agent);
                            state.step = CreateFeatureStep::SessionName;
                        }
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    match state.mode_focus {
                        0 => {
                            state.agent_index =
                                (state.agent_index + 1) % state.allowed_agents.len();
                            state.agent = state.allowed_agents[state.agent_index].clone();
                        }
                        1 => {
                            state.mode_index = (state.mode_index + 1) % VibeMode::ALL.len();
                            state.mode = VibeMode::ALL[state.mode_index].clone();
                        }
                        2 => {
                            state.review = !state.review;
                        }
                        3 => {
                            state.plan_mode = !state.plan_mode;
                        }
                        4 => {
                            if state.agent == AgentKind::Claude {
                                state.enable_chrome = !state.enable_chrome;
                            } else {
                                state.steering_enabled = !state.steering_enabled;
                            }
                        }
                        5 => {
                            state.steering_enabled = !state.steering_enabled;
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
                                state.allowed_agents.len() - 1
                            } else {
                                state.agent_index - 1
                            };
                            state.agent = state.allowed_agents[state.agent_index].clone();
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
                            state.plan_mode = !state.plan_mode;
                        }
                        4 => {
                            if state.agent == AgentKind::Claude {
                                state.enable_chrome = !state.enable_chrome;
                            } else {
                                state.steering_enabled = !state.steering_enabled;
                            }
                        }
                        5 => {
                            state.steering_enabled = !state.steering_enabled;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        },
        CreateFeatureStep::SessionName => match key {
            KeyCode::Esc => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.step = CreateFeatureStep::Mode;
                }
            }
            KeyCode::Enter => {
                let previous_message = app.message.clone();
                app.create_feature()?;
                if matches!(
                    app.mode,
                    AppMode::CreatingFeature(ref state)
                        if state.step == CreateFeatureStep::SessionName
                ) && app.message != previous_message
                    && let Some(message) = app.message.clone()
                {
                    app.push_toast_warning(message);
                }
            }
            KeyCode::Backspace => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.session_name.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.session_name.push(c);
                }
            }
            _ => {}
        },
        CreateFeatureStep::TaskPrompt => match key {
            KeyCode::Esc => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.step = CreateFeatureStep::Mode;
                }
            }
            KeyCode::Tab => {
                app.create_feature()?;
            }
            KeyCode::Enter => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.task_prompt.push('\n');
                    state.refresh_prompt_analysis();
                }
            }
            KeyCode::Backspace => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.task_prompt.pop();
                    state.refresh_prompt_analysis();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.task_prompt.push(c);
                    state.refresh_prompt_analysis();
                }
            }
            _ => {}
        },
        CreateFeatureStep::ConfirmSuperVibe => match key {
            KeyCode::Enter | KeyCode::Char('y') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.session_name = App::default_session_name_for_agent(&state.agent);
                    state.step = CreateFeatureStep::SessionName;
                }
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
