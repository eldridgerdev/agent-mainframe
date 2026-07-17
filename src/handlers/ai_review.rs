use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

/// Lines scrolled per detail-pane scroll keypress.
const DETAIL_SCROLL_STEP: usize = 5;

/// Key handling for the full-screen AI Review pane: AMF's own review of a
/// PR's diff, independent of PR Triage (see `crate::app::ai_review`'s module
/// doc). Navigate findings, skip/edit them, regenerate (`A`), post to GitHub
/// (`W`), and exit.
pub fn handle_ai_review_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // The harness picker, when open, captures all keys.
    if app.ai_review_harness_picking() {
        return handle_ai_harness_pick_key(app, key);
    }
    // The model picker, shown right after the harness, when open.
    if app.ai_review_model_picking() {
        return handle_ai_model_pick_key(app, key);
    }
    // The finding editor, when open, captures all keys.
    if app.ai_review_editing_finding() {
        match key.code {
            KeyCode::Esc => app.ai_review_stop_edit_finding(),
            _ => app.ai_review_finding_editor_key(key),
        }
        return Ok(());
    }
    // The post-to-GitHub confirm dialog, when open, captures all keys.
    if let Some(editing) = app.ai_review_post_confirm_view() {
        return handle_ai_review_post_key(app, key, editing);
    }

    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_ai_review(),
        KeyCode::Char('d') if ctrl => app.ai_review_scroll_detail_down(DETAIL_SCROLL_STEP),
        KeyCode::Char('u') if ctrl => app.ai_review_scroll_detail_up(DETAIL_SCROLL_STEP),
        KeyCode::PageDown => app.ai_review_scroll_detail_down(DETAIL_SCROLL_STEP * 2),
        KeyCode::PageUp => app.ai_review_scroll_detail_up(DETAIL_SCROLL_STEP * 2),
        KeyCode::Down | KeyCode::Char('j') => app.ai_review_select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.ai_review_select_prev(),
        KeyCode::Char('s') => app.ai_review_toggle_skip(),
        KeyCode::Char('e') => app.ai_review_edit_finding(),
        KeyCode::Char('A') => app.start_ai_pr_review(),
        KeyCode::Char('W') => app.ai_review_open_post_confirm(),
        _ => {}
    }
    Ok(())
}

fn handle_ai_harness_pick_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.ai_review_harness_pick_cancel(),
        KeyCode::Down | KeyCode::Char('j') => app.ai_review_harness_pick_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.ai_review_harness_pick_move(-1),
        KeyCode::Enter => app.ai_review_harness_pick_confirm(),
        _ => {}
    }
    Ok(())
}

fn handle_ai_model_pick_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.ai_review_model_pick_editing_custom() {
        match key.code {
            KeyCode::Esc => app.ai_review_model_pick_cancel(),
            KeyCode::Enter => app.ai_review_model_pick_confirm(),
            KeyCode::Backspace => app.ai_review_model_pick_backspace(),
            KeyCode::Char(c) => app.ai_review_model_pick_push_char(c),
            _ => {}
        }
        return Ok(());
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.ai_review_model_pick_cancel(),
        KeyCode::Down | KeyCode::Char('j') => app.ai_review_model_pick_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.ai_review_model_pick_move(-1),
        KeyCode::Enter => app.ai_review_model_pick_confirm(),
        _ => {}
    }
    Ok(())
}

/// Key handling while the post-to-GitHub confirm dialog is open.
///
/// Confirm view: `⏎` posts, `e` edits the summary, `esc`/`q` cancels. Edit
/// mode: keystrokes flow to the summary editor; `esc` returns to the confirm
/// view.
fn handle_ai_review_post_key(app: &mut App, key: KeyEvent, editing: bool) -> Result<()> {
    if editing {
        match key.code {
            KeyCode::Esc => app.ai_review_post_confirm_stop_edit(),
            _ => app.ai_review_post_confirm_editor_key(key),
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Enter => app.ai_review_post()?,
        KeyCode::Char('e') => app.ai_review_post_confirm_edit(),
        KeyCode::Esc | KeyCode::Char('q') => app.ai_review_cancel_post_confirm(),
        _ => {}
    }
    Ok(())
}

/// Key handling while the AI PR review's background pass is running:
/// `Esc`/`q` returns to the pane without aborting the background thread (it
/// keeps running — a real side effect, tokens spent — and
/// `App::poll_ai_pr_review_bg` still surfaces the result whenever it lands).
pub fn handle_ai_review_running_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
        app.cancel_ai_pr_review();
    }
    Ok(())
}
