use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

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
