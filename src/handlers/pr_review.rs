use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

/// Lines scrolled per detail-pane scroll keypress.
const DETAIL_SCROLL_STEP: usize = 5;

/// Key handling for the full-screen PR comment-review pane.
///
/// Read-only triage for now: navigate the comment list, scroll the detail,
/// hide/show resolved comments, refresh from GitHub, and exit. Action keys
/// (fix / reply / resolve) arrive with later epics.
pub fn handle_pr_review_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // The fix confirm/edit dialog, when open, captures all keys.
    if let Some(editing) = app.pr_review_fix_editing() {
        return handle_fix_confirm_key(app, key, editing);
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_pr_review(),
        KeyCode::Char('d') if ctrl => app.pr_review_scroll_detail_down(DETAIL_SCROLL_STEP),
        KeyCode::Char('u') if ctrl => app.pr_review_scroll_detail_up(DETAIL_SCROLL_STEP),
        KeyCode::PageDown => app.pr_review_scroll_detail_down(DETAIL_SCROLL_STEP * 2),
        KeyCode::PageUp => app.pr_review_scroll_detail_up(DETAIL_SCROLL_STEP * 2),
        KeyCode::Down | KeyCode::Char('j') => app.pr_review_select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.pr_review_select_prev(),
        KeyCode::Char('h') => app.pr_review_toggle_resolved(),
        KeyCode::Char('f') => app.pr_review_open_fix_confirm(),
        KeyCode::Char('t') => app.pr_review_toggle_fix_target(),
        KeyCode::Char('m') => app.pr_review_mark_done(),
        KeyCode::Char('s') => app.pr_review_skip(),
        KeyCode::Char('r') => app.refresh_pr_review(),
        KeyCode::Char('g') => app.open_pr_number_prompt(),
        _ => {}
    }
    Ok(())
}

/// Key handling while the fix confirm/edit dialog is open.
///
/// Confirm view (`editing == false`): `⏎` injects, `e` edits, `esc`/`q` cancel.
/// Edit mode (`editing == true`): keystrokes flow to the prompt editor; `esc`
/// returns to the confirm view so the prompt can be reviewed before sending.
fn handle_fix_confirm_key(app: &mut App, key: KeyEvent, editing: bool) -> Result<()> {
    if editing {
        match key.code {
            KeyCode::Esc => app.pr_review_fix_stop_edit(),
            _ => {
                app.pr_review_fix_editor_key(key);
            }
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Enter => app.pr_review_inject_fix()?,
        KeyCode::Char('e') => app.pr_review_fix_edit(),
        KeyCode::Esc | KeyCode::Char('q') => app.pr_review_cancel_fix(),
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

/// Key handling for the manual PR-number override prompt.
pub fn handle_pr_number_prompt_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.close_pr_review(),
        KeyCode::Enter => app.submit_pr_number(),
        KeyCode::Backspace => app.pr_number_prompt_backspace(),
        KeyCode::Char(c) => app.pr_number_prompt_push(c),
        _ => {}
    }
    Ok(())
}
