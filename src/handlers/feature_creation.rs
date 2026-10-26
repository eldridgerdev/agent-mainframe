use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, CreateFeatureState, CreateFeatureStep, Selection};
use crate::project::{ProjectStatus, VibeMode, normalized_feature_name};

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

/// Key entry point with modifiers: `Ctrl+S` creates from any form row,
/// `Ctrl+U` clears the focused text field, and other `Ctrl` chords are ignored
/// instead of typing their letter. Everything else goes to
/// [`handle_create_feature_key`].
pub fn handle_create_feature_event(app: &mut App, key: KeyEvent) -> Result<()> {
    let step = match &app.mode {
        AppMode::CreatingFeature(state) => state.step.clone(),
        _ => return Ok(()),
    };
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('s') | KeyCode::Char('S'))
    {
        if CreateFeatureState::is_form_step(&step) {
            submit_create_feature(app)?;
        }
        return Ok(());
    }
    if super::text_field::is_clear_key(&key) {
        if let AppMode::CreatingFeature(state) = &mut app.mode {
            match step {
                CreateFeatureStep::Branch => {
                    state.branch.clear();
                    state.branch_error = None;
                }
                CreateFeatureStep::SessionName => state.session_name.clear(),
                CreateFeatureStep::ExistingWorktree if state.worktree_search_active => {
                    state.worktree_query.clear();
                    state.clamp_worktree_selection();
                }
                _ => {}
            }
        }
        return Ok(());
    }
    if super::text_field::is_unbound_ctrl_chord(&key) {
        return Ok(());
    }
    handle_create_feature_key(app, key.code)
}

/// Create from the form: check the name, ask about SuperVibe if it is
/// chosen, then create. Shared by `Enter` on the last row and `Ctrl+S`.
fn submit_create_feature(app: &mut App) -> Result<()> {
    let (name_editable, supervibe) = match &app.mode {
        AppMode::CreatingFeature(state) => (
            !state.uses_existing_worktree(),
            matches!(state.mode, VibeMode::SuperVibe),
        ),
        _ => return Ok(()),
    };
    if name_editable && let Some(error) = branch_error(app) {
        if let AppMode::CreatingFeature(state) = &mut app.mode {
            state.branch_error = Some(error.clone());
            state.step = CreateFeatureStep::Branch;
        }
        app.push_toast_warning(error);
        return Ok(());
    }
    if supervibe {
        if let AppMode::CreatingFeature(state) = &mut app.mode {
            state.step = CreateFeatureStep::ConfirmSuperVibe;
        }
        return Ok(());
    }
    create_and_open(app)
}

/// Create the feature, then open its session so the user lands in the agent
/// they just asked for rather than on the dashboard with a toast.
fn create_and_open(app: &mut App) -> Result<()> {
    let (project_name, feature_name) = match &app.mode {
        AppMode::CreatingFeature(state) => (
            state.project_name.clone(),
            state
                .feature_name
                .clone()
                .unwrap_or_else(|| state.branch.clone()),
        ),
        _ => return Ok(()),
    };
    let previous_message = app.message.clone();
    app.create_feature()?;

    if let AppMode::CreatingFeature(state) = &mut app.mode {
        // Not created: the form (not the SuperVibe prompt) shows the problem.
        if state.step == CreateFeatureStep::ConfirmSuperVibe {
            state.step = CreateFeatureStep::SessionName;
        }
        if app.message != previous_message
            && let Some(message) = app.message.clone()
        {
            app.push_toast_warning(message);
        }
        return Ok(());
    }

    // Only a plain, synchronous creation that left the feature running opens
    // it. A plan interview, startup prompt or hook prompt already owns the
    // screen (mode is not Normal); a feature still waiting on its worktree
    // hook, or one the resource gate left stopped, has nothing to show yet
    // and its message says what to do.
    if !matches!(app.mode, AppMode::Normal) {
        return Ok(());
    }
    let Selection::Feature(pi, fi) = app.selection else {
        return Ok(());
    };
    let ready = app
        .store
        .projects
        .get(pi)
        .filter(|project| project.name == project_name)
        .and_then(|project| project.features.get(fi))
        .is_some_and(|feature| {
            feature.name == feature_name
                && !feature.pending_worktree_script
                && feature.status != ProjectStatus::Stopped
        });
    if ready {
        app.enter_view()?;
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
                    if state.worktree_search_active {
                        state.worktree_search_active = false;
                    } else {
                        state.step = CreateFeatureStep::Source;
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.clamp_worktree_selection();
                    let visible = state.visible_worktree_indices();
                    if !visible.is_empty() {
                        let pos = visible
                            .iter()
                            .position(|&idx| idx == state.worktree_index)
                            .unwrap_or(0);
                        state.worktree_index = visible[(pos + 1) % visible.len()];
                    }
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.clamp_worktree_selection();
                    let visible = state.visible_worktree_indices();
                    if !visible.is_empty() {
                        let pos = visible
                            .iter()
                            .position(|&idx| idx == state.worktree_index)
                            .unwrap_or(0);
                        state.worktree_index = if pos == 0 {
                            *visible.last().unwrap()
                        } else {
                            visible[pos - 1]
                        };
                    }
                }
            }
            KeyCode::Char('/') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    if state.worktree_search_active {
                        state.worktree_query.push('/');
                    } else {
                        state.worktree_search_active = true;
                        state.worktree_query.clear();
                    }
                    state.clamp_worktree_selection();
                }
            }
            KeyCode::Backspace => {
                if let AppMode::CreatingFeature(state) = &mut app.mode
                    && state.worktree_search_active
                {
                    state.worktree_query.pop();
                    state.clamp_worktree_selection();
                }
            }
            KeyCode::Enter => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.clamp_worktree_selection();
                    let visible = state.visible_worktree_indices();
                    if state.worktrees.is_empty() {
                        app.push_toast_warning("No available worktrees");
                    } else if visible.is_empty() {
                        app.push_toast_warning("No worktrees match the current filter");
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
            KeyCode::Char(c) => {
                if let AppMode::CreatingFeature(state) = &mut app.mode
                    && state.worktree_search_active
                {
                    state.worktree_query.push(c);
                    state.clamp_worktree_selection();
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
                    state.mode_index = VibeMode::ALL
                        .iter()
                        .position(|mode| *mode == preset.mode)
                        .unwrap_or(0);
                    state.mode = preset.mode;
                    match state.allowed_agents.iter().position(|a| *a == preset.agent) {
                        Some(index) => state.select_agent(index),
                        None => state.set_agent(preset.agent.clone()),
                    }
                    state.review = preset.review;
                    state.plan_mode = preset.plan_mode;
                    state.quick_plan = preset.quick_plan;
                    state.enable_chrome = preset.enable_chrome;
                    state.remote_control = preset.remote_control;
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
            KeyCode::Tab | KeyCode::BackTab => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.move_form_focus(key == KeyCode::Tab);
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
                    state.move_form_focus(true);
                }
            }
            KeyCode::BackTab | KeyCode::Char('h') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.move_form_focus(false);
                }
            }
            KeyCode::Down
            | KeyCode::Up
            | KeyCode::Char('j')
            | KeyCode::Char('k')
            | KeyCode::Char(' ') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    // Without git there is no worktree to create; keep "No".
                    state.use_worktree = state.git_available && !state.use_worktree;
                }
            }
            _ => {}
        },
        CreateFeatureStep::Mode => match key {
            KeyCode::Esc => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    if state.mode_focus > 0 {
                        state.mode_focus -= 1;
                    } else if state.uses_existing_worktree() {
                        state.step = CreateFeatureStep::ExistingWorktree;
                    } else {
                        state.step = CreateFeatureStep::Worktree;
                    }
                }
            }
            KeyCode::Tab | KeyCode::Char('l') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.move_form_focus(true);
                }
            }
            KeyCode::BackTab | KeyCode::Char('h') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.move_form_focus(false);
                }
            }
            KeyCode::Enter => {
                // Enter moves to the next field; the last option row leads to
                // the session name, whose Enter creates.
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    if state.mode_focus < state.mode_max_focus() {
                        state.mode_focus += 1;
                    } else {
                        state.step = CreateFeatureStep::SessionName;
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char(' ') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    cycle_mode_field(state, true);
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    cycle_mode_field(state, false);
                }
            }
            _ => {}
        },
        CreateFeatureStep::SessionName => match key {
            KeyCode::Esc => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.step = CreateFeatureStep::Mode;
                    state.mode_focus = state.mode_max_focus();
                }
            }
            KeyCode::Enter => {
                submit_create_feature(app)?;
            }
            KeyCode::Tab | KeyCode::BackTab => {
                if let AppMode::CreatingFeature(state) = &mut app.mode {
                    state.move_form_focus(key == KeyCode::Tab);
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
                create_and_open(app)?;
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

/// Change the value of the focused options row: the next/previous harness or
/// mode, or flip a checkbox (the Plan row cycles None → Quick → Full).
fn cycle_mode_field(state: &mut CreateFeatureState, forward: bool) {
    let claude = state.agent == crate::project::AgentKind::Claude;
    match state.mode_focus {
        0 => {
            let len = state.allowed_agents.len();
            if len > 0 {
                let index = if forward {
                    (state.agent_index + 1) % len
                } else {
                    (state.agent_index + len - 1) % len
                };
                state.select_agent(index);
            }
        }
        1 => {
            let len = VibeMode::ALL.len();
            state.mode_index = if forward {
                (state.mode_index + 1) % len
            } else {
                (state.mode_index + len - 1) % len
            };
            state.mode = VibeMode::ALL[state.mode_index].clone();
        }
        2 => state.review = !state.review,
        3 => state.cycle_plan_choice(forward),
        4 if claude => state.enable_chrome = !state.enable_chrome,
        // Remote Control requires claude.ai OAuth; it is unavailable for
        // z.ai / third-party provider sessions, where the toggle is inert.
        5 if claude => {
            if state.remote_control_available {
                state.remote_control = !state.remote_control;
            }
        }
        4 | 6 => state.steering_enabled = !state.steering_enabled,
        _ => {}
    }
}
