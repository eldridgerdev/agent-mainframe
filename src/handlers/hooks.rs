use anyhow::Result;
use crossterm::event::KeyCode;

use crate::app::{App, AppMode};

pub fn handle_running_hook_key(app: &mut App, key: KeyCode) -> Result<()> {
    let is_running = match &app.mode {
        AppMode::RunningHook(state) => state.child.is_some(),
        _ => return Ok(()),
    };

    if is_running {
        if let KeyCode::Char('h') = key {
            app.hide_running_hook();
        }
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
        AppMode::DeletingFeatureInProgress(state) => (
            state.child.is_some(),
            state.stage == crate::app::DeleteStage::Completed,
        ),
        _ => return Ok(()),
    };

    if is_running {
        if let KeyCode::Char('h') = key {
            app.hide_deleting_feature();
        }
        return Ok(());
    }

    match key {
        KeyCode::Char('h') => {
            app.hide_deleting_feature();
        }
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

pub fn handle_hook_prompt_key(app: &mut App, key: KeyCode) -> Result<()> {
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
