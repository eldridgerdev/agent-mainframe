use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;

pub fn handle_context_settings_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.cancel_context_settings(),
        KeyCode::Enter => {
            app.context_settings_confirm();
        }
        KeyCode::Tab | KeyCode::Down => app.context_settings_focus_next(),
        KeyCode::BackTab | KeyCode::Up => app.context_settings_focus_prev(),
        KeyCode::Backspace => app.context_settings_backspace(),
        KeyCode::Char(c) => app.context_settings_push_char(c),
        _ => {}
    }
    Ok(())
}
