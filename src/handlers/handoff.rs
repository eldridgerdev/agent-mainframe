//! Key dispatch for the fresh-context instruction prompt (`AppMode::FreshContextPrompt`).

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, AppMode};

/// Key dispatch for the one-line fresh-context instruction prompt, shown
/// before starting a fresh agent session over `Ctrl+Space` then `Shift+F`.
pub fn handle_fresh_context_prompt_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.cancel_fresh_context_prompt(),
        KeyCode::Enter => app.commit_fresh_context_prompt()?,
        KeyCode::Backspace => {
            if let AppMode::FreshContextPrompt(state) = &mut app.mode {
                state.input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::FreshContextPrompt(state) = &mut app.mode {
                state.input.push(c);
            }
        }
        _ => {}
    }
    Ok(())
}
