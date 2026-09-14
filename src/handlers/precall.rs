use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{App, AppMode};

/// The blocking pre-call notice: `v` view/hide the prompt, `e` edit it in the
/// override manager, `Enter` continue, `Esc` cancel.
pub fn handle_prompt_precall_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if !matches!(&app.mode, AppMode::PromptPrecall(_)) {
        return Ok(());
    }
    match key.code {
        KeyCode::Char('v') | KeyCode::Char('V') => app.precall_toggle_view(),
        KeyCode::Char('e') | KeyCode::Char('E') => app.precall_edit(),
        KeyCode::Char('j') | KeyCode::Down => app.precall_scroll(1),
        KeyCode::Char('k') | KeyCode::Up => app.precall_scroll(-1),
        KeyCode::Enter | KeyCode::Char('c') => app.precall_confirm()?,
        KeyCode::Esc | KeyCode::Char('q') => app.precall_cancel(),
        _ => {}
    }
    Ok(())
}
