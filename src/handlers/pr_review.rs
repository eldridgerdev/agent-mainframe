use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::App;

/// Key handling for the full-screen PR comment-review pane.
///
/// Read-only triage for now: navigate the comment list and exit. Action keys
/// (fix / reply / resolve) arrive with later epics.
pub fn handle_pr_review_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_pr_review(),
        KeyCode::Down | KeyCode::Char('j') => app.pr_review_select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.pr_review_select_prev(),
        _ => {}
    }
    Ok(())
}

/// Key handling while a PR's comments are being fetched: only allow cancel.
pub fn handle_pr_review_loading_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
        app.close_pr_review();
    }
    Ok(())
}
