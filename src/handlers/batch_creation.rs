use anyhow::Result;
use crossterm::event::KeyCode;

use crate::app::{App, AppMode, CreateBatchFeaturesStep};
use crate::project::{AgentKind, VibeMode};

pub fn handle_create_batch_features_key(app: &mut App, key: KeyCode) -> Result<()> {
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
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.step = CreateBatchFeaturesStep::ProjectName;
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
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.step = CreateBatchFeaturesStep::ProjectName;
                }
            }
            KeyCode::Enter => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.step = CreateBatchFeaturesStep::FeatureBaseName;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    state.feature_count += 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    if state.feature_count > 1 {
                        state.feature_count -= 1;
                    }
                }
            }
            KeyCode::Backspace => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    let count_str = state.feature_count.to_string();
                    if count_str.len() > 1 {
                        state.feature_count = count_str[..count_str.len()-1].parse().unwrap_or(1);
                    } else {
                        state.feature_count = 1;
                    }
                }
            }
            KeyCode::Char(c) if c.is_ascii_digit() => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    let digit = c.to_digit(10).unwrap() as usize;
                    state.feature_count = state.feature_count * 10 + digit;
                    if state.feature_count > 50 {
                        state.feature_count = 50;
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
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode
                    && state.mode_focus > 0
                {
                    state.mode_focus -= 1;
                }
            }
            KeyCode::Enter => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
                    let max_focus = if state.agent == AgentKind::Claude {
                        4
                    } else {
                        3
                    };
                    if state.mode_focus < max_focus {
                        state.mode_focus += 1;
                    } else {
                        app.start_create_batch_features();
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
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
                if let AppMode::CreatingBatchFeatures(state) = &mut app.mode {
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
    }
    Ok(())
}
