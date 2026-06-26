use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

/// Lines scrolled per detail-pane scroll keypress.
const DETAIL_SCROLL_STEP: usize = 5;

/// Key handling for the full-screen PR comment-review pane.
///
/// Navigate the comment list, scroll the detail, hide/show resolved comments,
/// refresh from GitHub, and exit. Action keys: `f` fix, `R`/`n` reply, `x`
/// resolve/reopen the thread, `m` mark done, `s` skip, `i` install syntax
/// highlighting for the selected comment's file.
pub fn handle_pr_review_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // The harness picker, when open, captures all keys.
    if app.pr_review_harness_picking() {
        return handle_harness_pick_key(app, key);
    }
    // The fix confirm/edit dialog, when open, captures all keys.
    if let Some(editing) = app.pr_review_fix_editing() {
        return handle_fix_confirm_key(app, key, editing);
    }
    // The reply dialog, when open, captures all keys.
    if let Some(editing) = app.pr_review_reply_view() {
        return handle_reply_key(app, key, editing);
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
        KeyCode::Char('R') => app.pr_review_open_reply_done(),
        KeyCode::Char('n') => app.pr_review_open_reply_not_needed(),
        KeyCode::Char('t') => app.pr_review_toggle_fix_target(),
        KeyCode::Char('m') => app.pr_review_mark_done(),
        KeyCode::Char('x') => app.pr_review_toggle_resolve(),
        KeyCode::Char('s') => app.pr_review_skip(),
        KeyCode::Char('r') => app.refresh_pr_review(),
        KeyCode::Char('i') => app.open_syntax_language_picker_for_selected_diff_file(),
        KeyCode::Char('g') => app.open_pr_picker_from_pane(),
        _ => {}
    }
    Ok(())
}

/// Key handling for the PR picker: navigate the list, `⏎` open the highlighted
/// PR, `a` toggle closed/merged PRs, `#` switch to typing a number, `esc` close.
pub fn handle_pr_picker_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_pr_review(),
        KeyCode::Down | KeyCode::Char('j') => app.pr_picker_select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.pr_picker_select_prev(),
        KeyCode::Enter => app.pr_picker_choose(),
        KeyCode::Char('a') => app.pr_picker_toggle_closed(),
        KeyCode::Char('#') | KeyCode::Char('g') => app.pr_picker_to_number_prompt(),
        _ => {}
    }
    Ok(())
}

/// Key handling while the reply dialog is open.
///
/// Confirm view: `⏎` posts, `e` edits, `esc`/`q` cancels. Edit mode: keystrokes
/// flow to the editor; `esc` returns to the confirm view (the "not needed" reply
/// opens straight in edit mode so the user can type the reason).
fn handle_reply_key(app: &mut App, key: KeyEvent, editing: bool) -> Result<()> {
    if editing {
        match key.code {
            KeyCode::Esc => app.pr_review_reply_stop_edit(),
            _ => app.pr_review_reply_editor_key(key),
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Enter => app.pr_review_post_reply()?,
        KeyCode::Char('e') => app.pr_review_reply_edit(),
        KeyCode::Esc | KeyCode::Char('q') => app.pr_review_cancel_reply(),
        _ => {}
    }
    Ok(())
}

/// Key handling while the dedicated-review harness picker is open.
///
/// `j/k` (or arrows) move the highlight, `⏎` picks the harness and continues to
/// the fix confirm dialog, `esc`/`q` cancels (aborts this fix).
fn handle_harness_pick_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.pr_review_harness_pick_cancel(),
        KeyCode::Down | KeyCode::Char('j') => app.pr_review_harness_pick_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.pr_review_harness_pick_move(-1),
        KeyCode::Enter => app.pr_review_harness_pick_confirm(),
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
