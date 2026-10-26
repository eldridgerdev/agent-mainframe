use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, AppMode, CreateBatchFeaturesState, CreateBatchFeaturesStep};
use crate::project::{AgentKind, VibeMode};

/// Key entry point with modifiers: `Ctrl+U` clears the focused text field and
/// other `Ctrl` chords are ignored instead of typing their letter.
pub fn handle_create_batch_features_event(app: &mut App, key: KeyEvent) -> Result<()> {
    if super::text_field::is_clear_key(&key) {
        if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
            match state.step {
                CreateBatchFeaturesStep::WorkspacePath => state.workspace_path.clear(),
                CreateBatchFeaturesStep::ProjectName => state.project_name.clear(),
                CreateBatchFeaturesStep::FeatureCount => state.feature_count = 1,
                CreateBatchFeaturesStep::FeatureBaseName => state.feature_prefix.clear(),
                CreateBatchFeaturesStep::FeatureSettings => {}
            }
        }
        return Ok(());
    }
    if super::text_field::is_unbound_ctrl_chord(&key) {
        return Ok(());
    }
    handle_create_batch_features_key(app, key.code)
}

fn handle_create_batch_features_key(app: &mut App, key: KeyCode) -> Result<()> {
    let step = match &app.mode {
        AppMode::CreatingBatchFeatures(state) => state.step.clone(),
        _ => return Ok(()),
    };

    match step {
        CreateBatchFeaturesStep::WorkspacePath => match key {
            KeyCode::Esc => {
                app.cancel_create();
            }
            KeyCode::Enter => {
                let mut missing = false;
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    if state.workspace_path.trim().is_empty() {
                        missing = true;
                    } else {
                        if state.project_name.trim().is_empty() {
                            state.project_name =
                                CreateBatchFeaturesState::name_from_path(&state.workspace_path);
                        }
                        state.step = CreateBatchFeaturesStep::ProjectName;
                    }
                }
                if missing {
                    app.push_toast_warning("Type the git repository's folder path first");
                }
            }
            KeyCode::Backspace => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.workspace_path.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.workspace_path.push(c);
                }
            }
            _ => {}
        },
        CreateBatchFeaturesStep::ProjectName => match key {
            KeyCode::Esc => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.step = CreateBatchFeaturesStep::WorkspacePath;
                }
            }
            KeyCode::Enter => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.step = CreateBatchFeaturesStep::FeatureCount;
                }
            }
            KeyCode::Backspace => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.project_name.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.project_name.push(c);
                }
            }
            _ => {}
        },
        CreateBatchFeaturesStep::FeatureCount => match key {
            KeyCode::Esc => {
                // A batch into the selected project starts here, so there is
                // nothing earlier to go back to.
                let existing = matches!(
                    &app.mode,
                    AppMode::CreatingBatchFeatures(state) if state.existing_project
                );
                if existing {
                    app.cancel_create();
                } else if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.step = CreateBatchFeaturesStep::ProjectName;
                }
            }
            KeyCode::Enter => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.step = CreateBatchFeaturesStep::FeatureBaseName;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode
                    && state.feature_count < CreateBatchFeaturesState::MAX_FEATURES
                {
                    state.feature_count += 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode
                    && state.feature_count > 1
                {
                    state.feature_count -= 1;
                }
            }
            KeyCode::Backspace => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    let count_str = state.feature_count.to_string();
                    if count_str.len() > 1 {
                        state.feature_count = count_str[..count_str.len() - 1].parse().unwrap_or(1);
                    } else {
                        state.feature_count = 1;
                    }
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    let digit = c.to_digit(10).unwrap() as usize;
                    state.feature_count = state.feature_count * 10 + digit;
                    if state.feature_count > CreateBatchFeaturesState::MAX_FEATURES {
                        state.feature_count = CreateBatchFeaturesState::MAX_FEATURES;
                    }
                }
            }
            _ => {}
        },
        CreateBatchFeaturesStep::FeatureBaseName => match key {
            KeyCode::Esc => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.step = CreateBatchFeaturesStep::FeatureCount;
                }
            }
            KeyCode::Enter => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.step = CreateBatchFeaturesStep::FeatureSettings;
                }
            }
            KeyCode::Backspace => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.feature_prefix.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.feature_prefix.push(c);
                }
            }
            _ => {}
        },
        CreateBatchFeaturesStep::FeatureSettings => match key {
            KeyCode::Esc => {
                app.message = None;
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    if state.mode_focus > 0 {
                        state.mode_focus -= 1;
                    } else {
                        state.step = CreateBatchFeaturesStep::FeatureBaseName;
                    }
                }
            }
            KeyCode::Tab | KeyCode::Char('l') => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    let max_focus = if state.agent == AgentKind::Claude {
                        3
                    } else {
                        2
                    };
                    if state.mode_focus < max_focus {
                        state.mode_focus += 1;
                    }
                }
            }
            KeyCode::BackTab | KeyCode::Char('h') => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode
                    && state.mode_focus > 0
                {
                    state.mode_focus -= 1;
                }
            }
            KeyCode::Enter => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    let max_focus = if state.agent == AgentKind::Claude {
                        3
                    } else {
                        2
                    };
                    if state.mode_focus < max_focus {
                        state.mode_focus += 1;
                    } else {
                        let _ = app.create_batch_features();
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char(' ') => {
                let allowed_agents = app.active_extension.allowed_agents();
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    match state.mode_focus {
                        0 => {
                            state.agent_index = (state.agent_index + 1) % allowed_agents.len();
                            state.agent = allowed_agents[state.agent_index].clone();
                        }
                        1 => {
                            state.mode_index = (state.mode_index + 1) % VibeMode::ALL.len();
                            state.mode = VibeMode::ALL[state.mode_index].clone();
                        }
                        2 => {
                            state.review = !state.review;
                        }
                        3 => {
                            state.enable_chrome = !state.enable_chrome;
                        }
                        _ => {}
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let allowed_agents = app.active_extension.allowed_agents();
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    match state.mode_focus {
                        0 => {
                            state.agent_index = if state.agent_index == 0 {
                                allowed_agents.len() - 1
                            } else {
                                state.agent_index - 1
                            };
                            state.agent = allowed_agents[state.agent_index].clone();
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
                            state.enable_chrome = !state.enable_chrome;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        },
    }
    Ok(())
}
