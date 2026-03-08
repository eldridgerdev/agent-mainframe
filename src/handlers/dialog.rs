use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, CreateProjectStep};

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

pub fn handle_help_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
            let from_view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
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

pub fn handle_latest_prompt_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
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

pub fn handle_steering_prompt_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.cancel_steering_prompt();
        }
        KeyCode::Tab => {
            app.submit_steering_prompt()?;
        }
        KeyCode::Enter => {
            if let AppMode::SteeringPrompt(state) = &mut app.mode {
                state.prompt.push('\n');
                state.prompt_analysis = crate::app::analyze_prompt(&state.prompt);
            }
        }
        KeyCode::Backspace => {
            if let AppMode::SteeringPrompt(state) = &mut app.mode {
                state.prompt.pop();
                state.prompt_analysis = crate::app::analyze_prompt(&state.prompt);
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::SteeringPrompt(state) = &mut app.mode {
                state.prompt.push(c);
                state.prompt_analysis = crate::app::analyze_prompt(&state.prompt);
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

pub fn handle_theme_picker_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::ThemePicker(state) = &mut app.mode
                && state.selected + 1 < state.themes.len()
            {
                state.selected += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::ThemePicker(state) = &mut app.mode
                && state.selected > 0
            {
                state.selected -= 1;
            }
        }
        KeyCode::Enter => {
            let theme_name = match &app.mode {
                AppMode::ThemePicker(state) => state.themes.get(state.selected).copied(),
                _ => None,
            };
            if let Some(name) = theme_name {
                app.apply_theme(name);
            }
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            app.mode = AppMode::Normal;
        }
        _ => {}
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

pub fn handle_rename_feature_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.cancel_rename_feature();
        }
        KeyCode::Enter => {
            app.apply_rename_feature()?;
        }
        KeyCode::Backspace => {
            if let AppMode::RenamingFeature(state) = &mut app.mode {
                state.input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::RenamingFeature(state) = &mut app.mode {
                state.input.push(c);
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_session_config_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::SessionConfig(state) = &mut app.mode
                && state.selected_agent + 1 < state.allowed_agents.len()
            {
                state.selected_agent += 1;
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::SessionConfig(state) = &mut app.mode
                && state.selected_agent > 0
            {
                state.selected_agent -= 1;
            }
        }
        KeyCode::Enter => {
            app.apply_session_config()?;
        }
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_session_config();
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_debug_log_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            let from_view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::DebugLog(state) => state.from_view,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            if let Some(view) = from_view {
                app.mode = AppMode::Viewing(view);
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::DebugLog(state) = &mut app.mode {
                state.scroll_offset = state.scroll_offset.saturating_add(1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::DebugLog(state) = &mut app.mode {
                state.scroll_offset = state.scroll_offset.saturating_sub(1);
            }
        }
        KeyCode::Char('c') => {
            app.debug_log.clear();
            if let AppMode::DebugLog(state) = &mut app.mode {
                state.scroll_offset = 0;
            }
        }
        _ => {}
    }
    Ok(())
}
