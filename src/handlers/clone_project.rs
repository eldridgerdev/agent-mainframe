use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode};

/// Keys on the clone pre-flight / progress screens. Every decision lives in
/// `App::clone_view_*`; this only maps keys.
pub fn handle_clone_project_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(());
    }
    match key.code {
        KeyCode::Esc => app.clone_view_escape(),
        KeyCode::Enter | KeyCode::Char('s') => app.clone_view_start(),
        KeyCode::Char('r') => app.clone_view_recheck(),
        KeyCode::Char('c') => app.clone_view_copy_fix(1),
        KeyCode::Char(d @ '1'..='9') => app.clone_view_copy_fix(d as usize - '0' as usize),
        _ => {}
    }
    Ok(())
}

/// Keys on the missing-projects-folder prompt. The two sub-states (choices,
/// typing a folder) share no letters: while typing, every character is text.
pub fn handle_clone_root_prompt_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let editing = matches!(&app.mode, AppMode::CloneRootPrompt(p) if p.editing.is_some());
    if editing {
        if super::text_field::is_clear_key(&key) {
            if let AppMode::CloneRootPrompt(p) = &mut app.mode {
                p.editing = Some(String::new());
                p.error = None;
            }
            return Ok(());
        }
        if super::text_field::is_unbound_ctrl_chord(&key) {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => app.clone_root_prompt_stop_editing(),
            KeyCode::Enter => app.clone_root_prompt_confirm_choice(),
            KeyCode::Backspace => {
                if let AppMode::CloneRootPrompt(p) = &mut app.mode {
                    if let Some(buf) = &mut p.editing {
                        buf.pop();
                    }
                    p.error = None;
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::CloneRootPrompt(p) = &mut app.mode {
                    if let Some(buf) = &mut p.editing {
                        buf.push(c);
                    }
                    p.error = None;
                }
            }
            _ => {}
        }
        return Ok(());
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return Ok(());
    }
    match key.code {
        KeyCode::Esc => app.clone_root_prompt_cancel(),
        KeyCode::Enter | KeyCode::Char('c') => app.clone_root_prompt_create(),
        KeyCode::Char('d') => app.clone_root_prompt_choose(),
        _ => {}
    }
    Ok(())
}
