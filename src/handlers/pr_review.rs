use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

/// Lines scrolled per detail-pane scroll keypress.
const DETAIL_SCROLL_STEP: usize = 5;

/// Visual rows scrolled per page-key press in the fix-prompt editor.
const FIX_PAGE_STEP: isize = 10;

/// Key handling for the full-screen PR Triage pane.
///
/// Navigate the comment list, scroll the detail, hide/show resolved comments,
/// refresh from GitHub, and exit. Action keys: `f` fix, `space` mark / `B`
/// inject one combined prompt for all marked comments, `R` opens the
/// reply-kind picker (Done / not-needed), `M` add to memory, `m` opens the
/// "Mark" picker (Done (local) / Skip (local) / Resolve on GitHub), `i`
/// install syntax highlighting for the selected comment's file, `A` run an AI
/// review of the PR diff (Epic E — draft findings merge into this same
/// list), `W` post the AI-review draft findings to GitHub as a real review.
pub fn handle_pr_review_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // The AI-review harness picker is independent from the fix-session picker.
    if app.pr_review_ai_harness_picking() {
        return handle_ai_harness_pick_key(app, key);
    }
    // The AI-review model picker, shown right after the harness, when open.
    if app.pr_review_ai_model_picking() {
        return handle_ai_model_pick_key(app, key);
    }
    // The fix-target picker, when open, captures all keys.
    if app.pr_review_harness_picking() {
        return handle_harness_pick_key(app, key);
    }
    // The reply-kind picker (`R`), when open, captures all keys.
    if app.pr_review_reply_pick_picking() {
        return handle_reply_pick_key(app, key);
    }
    // The "Mark" picker (`m`), when open, captures all keys.
    if app.pr_review_mark_pick_picking() {
        return handle_mark_pick_key(app, key);
    }
    // The fix confirm/edit dialog, when open, captures all keys.
    if let Some(editing) = app.pr_review_fix_editing() {
        return handle_fix_confirm_key(app, key, editing);
    }
    // The reply dialog, when open, captures all keys.
    if let Some(editing) = app.pr_review_reply_view() {
        return handle_reply_key(app, key, editing);
    }
    // The "add to memory" dialog, when open, captures all keys.
    if let Some(editing) = app.pr_review_memory_add_view() {
        return handle_memory_add_key(app, key, editing);
    }
    // The AI-review post-to-GitHub confirm dialog, when open, captures all keys.
    if let Some(editing) = app.pr_review_ai_review_post_view() {
        return handle_ai_review_post_key(app, key, editing);
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
        KeyCode::Char('o') => app.pr_review_cycle_sort(),
        KeyCode::Char('f') => app.pr_review_open_fix_confirm(),
        KeyCode::Char('P') => app.pr_review_toggle_to_session()?,
        KeyCode::Char(' ') => app.pr_review_toggle_mark(),
        KeyCode::Char('B') => app.pr_review_open_batch_confirm(),
        KeyCode::Char('R') => app.pr_review_open_reply_pick(),
        KeyCode::Char('M') => app.pr_review_open_memory_add(),
        KeyCode::Char('m') => app.pr_review_open_mark_pick(),
        KeyCode::Char('r') => app.refresh_pr_review(),
        KeyCode::Char('i') => app.open_syntax_language_picker_for_selected_diff_file(),
        KeyCode::Char('g') => app.open_pr_picker_from_pane(),
        KeyCode::Char('A') => app.start_ai_pr_review(),
        KeyCode::Char('W') => app.pr_review_open_ai_review_post_confirm(),
        _ => {}
    }
    Ok(())
}

/// Key handling while the reply-kind picker (`R`) is open: `j/k` move,
/// `⏎` confirm (opens the corresponding reply dialog), `esc`/`q` cancel.
fn handle_reply_pick_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.pr_review_reply_pick_cancel(),
        KeyCode::Down | KeyCode::Char('j') => app.pr_review_reply_pick_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.pr_review_reply_pick_move(-1),
        KeyCode::Enter => app.pr_review_reply_pick_confirm(),
        _ => {}
    }
    Ok(())
}

/// Key handling while the "Mark" picker (`m`) is open: `j/k` move, `⏎`
/// confirm (applies the chosen action immediately), `esc`/`q` cancel.
fn handle_mark_pick_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.pr_review_mark_pick_cancel(),
        KeyCode::Down | KeyCode::Char('j') => app.pr_review_mark_pick_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.pr_review_mark_pick_move(-1),
        KeyCode::Enter => app.pr_review_mark_pick_confirm(),
        _ => {}
    }
    Ok(())
}

/// Pick the harness for the paid `A` review pass. Confirm validates the CLI
/// before any diff is fetched or agent tokens are spent; failures stay inline.
fn handle_ai_harness_pick_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.pr_review_ai_harness_pick_cancel(),
        KeyCode::Down | KeyCode::Char('j') => app.pr_review_ai_harness_pick_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.pr_review_ai_harness_pick_move(-1),
        KeyCode::Enter => app.pr_review_ai_harness_pick_confirm(),
        _ => {}
    }
    Ok(())
}

/// Pick the model for the paid `A` review pass, shown once per pane right
/// after the harness. While the `Custom` row is open for text entry, keys
/// route to the buffer instead of list navigation.
fn handle_ai_model_pick_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if app.pr_review_ai_model_pick_editing_custom() {
        match key.code {
            KeyCode::Esc => app.pr_review_ai_model_pick_cancel(),
            KeyCode::Enter => app.pr_review_ai_model_pick_confirm(),
            KeyCode::Backspace => app.pr_review_ai_model_pick_backspace(),
            KeyCode::Char(c) => app.pr_review_ai_model_pick_push_char(c),
            _ => {}
        }
        return Ok(());
    }
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.pr_review_ai_model_pick_cancel(),
        KeyCode::Down | KeyCode::Char('j') => app.pr_review_ai_model_pick_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.pr_review_ai_model_pick_move(-1),
        KeyCode::Enter => app.pr_review_ai_model_pick_confirm(),
        _ => {}
    }
    Ok(())
}

/// Key handling for the PR picker: navigate the list, `⏎` open the highlighted
/// PR, `a` toggle closed/merged PRs, `#` switch to typing a number, `b` open
/// the review-memory lookback bootstrap, `esc` close.
pub fn handle_pr_picker_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // The lookback-bootstrap depth picker, when open, captures all keys.
    if app.review_memory_bootstrap_picking() {
        return handle_bootstrap_pick_key(app, key);
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_pr_review(),
        KeyCode::Down | KeyCode::Char('j') => app.pr_picker_select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.pr_picker_select_prev(),
        KeyCode::Enter => app.pr_picker_choose(),
        KeyCode::Char('a') => app.pr_picker_toggle_closed(),
        KeyCode::Char('#') | KeyCode::Char('g') => app.pr_picker_to_number_prompt(),
        KeyCode::Char('b') => app.open_review_memory_bootstrap_pick(),
        _ => {}
    }
    Ok(())
}

/// Key handling for the lookback-bootstrap depth picker: `j/k` move, `⏎` run,
/// `esc`/`q` cancel back to the PR picker.
fn handle_bootstrap_pick_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.review_memory_bootstrap_pick_cancel(),
        KeyCode::Down | KeyCode::Char('j') => app.review_memory_bootstrap_pick_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.review_memory_bootstrap_pick_move(-1),
        KeyCode::Enter => app.review_memory_bootstrap_pick_confirm(),
        _ => {}
    }
    Ok(())
}

/// Key handling for the full-screen lookback-bootstrap running view: `esc`/`q`
/// return to the PR picker (the background thread keeps running to
/// completion; its result still lands via `poll_review_memory_bootstrap_bg`).
pub fn handle_review_memory_bootstrap_running_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
        app.cancel_review_memory_bootstrap();
    }
    Ok(())
}

/// Key handling while the AI PR review's background pass is running:
/// `Esc`/`q` returns to the review pane without aborting the background
/// thread (it keeps running — a real side effect, tokens spent — and
/// [`App::poll_ai_pr_review_bg`] still surfaces the result whenever it lands).
pub fn handle_ai_pr_review_running_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
        app.cancel_ai_pr_review();
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

/// Key handling while the AI-review post-to-GitHub confirm dialog is open.
///
/// Confirm view: `⏎` posts, `e` edits the summary, `esc`/`q` cancels. Edit
/// mode: keystrokes flow to the summary editor; `esc` returns to the confirm
/// view. Only the summary is editable — inline finding bodies are vetted by
/// skipping (`s`), not hand-edited here.
fn handle_ai_review_post_key(app: &mut App, key: KeyEvent, editing: bool) -> Result<()> {
    if editing {
        match key.code {
            KeyCode::Esc => app.pr_review_ai_review_post_stop_edit(),
            _ => app.pr_review_ai_review_post_editor_key(key),
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Enter => app.pr_review_post_ai_review()?,
        KeyCode::Char('e') => app.pr_review_ai_review_post_edit(),
        KeyCode::Esc | KeyCode::Char('q') => app.pr_review_cancel_ai_review_post(),
        _ => {}
    }
    Ok(())
}

/// Key handling while the "add to memory" dialog is open.
///
/// Confirm view: `⏎` appends, `e` edits, `Tab` cycles the category, `esc`/`q`
/// cancels. Edit mode: keystrokes flow to the finding editor; `esc` returns to
/// the confirm view.
fn handle_memory_add_key(app: &mut App, key: KeyEvent, editing: bool) -> Result<()> {
    if editing {
        match key.code {
            KeyCode::Esc => app.pr_review_memory_add_stop_edit(),
            _ => app.pr_review_memory_add_editor_key(key),
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Enter => app.pr_review_append_memory()?,
        KeyCode::Char('e') => app.pr_review_memory_add_edit(),
        KeyCode::Tab => app.pr_review_cycle_memory_category(),
        KeyCode::Esc | KeyCode::Char('q') => app.pr_review_cancel_memory_add(),
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
/// Edit mode (`editing == true`): keystrokes flow to the prompt editor, which
/// now supports vim (toggle with `Ctrl+T`), scrolling (`Ctrl+J/K`,
/// `PgUp/PgDn`), and a `Tab` submit gesture that coexists with multi-line
/// editing. `Esc` leaves edit mode in plain keymap; under vim it goes to the
/// editor (Insert→Normal), so `Ctrl+Q` is the keymap-independent way back to
/// the confirm view.
fn handle_fix_confirm_key(app: &mut App, key: KeyEvent, editing: bool) -> Result<()> {
    if editing {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Tab submits (injects) even from multi-line edit mode, where Enter is
        // a newline.
        if key.code == KeyCode::Tab {
            return app.pr_review_inject_fix();
        }
        if ctrl && key.code == KeyCode::Char('t') {
            app.pr_review_fix_toggle_vim();
            return Ok(());
        }
        if ctrl && key.code == KeyCode::Char('q') {
            app.pr_review_fix_stop_edit();
            return Ok(());
        }
        match key.code {
            KeyCode::Char('j') if ctrl => {
                app.pr_review_fix_scroll(1);
                return Ok(());
            }
            KeyCode::Char('k') if ctrl => {
                app.pr_review_fix_scroll(-1);
                return Ok(());
            }
            KeyCode::PageDown => {
                app.pr_review_fix_scroll(FIX_PAGE_STEP);
                return Ok(());
            }
            KeyCode::PageUp => {
                app.pr_review_fix_scroll(-FIX_PAGE_STEP);
                return Ok(());
            }
            // Esc leaves edit mode only in plain keymap; vim consumes it for
            // Insert→Normal.
            KeyCode::Esc if app.pr_review_fix_vim_mode().is_none() => {
                app.pr_review_fix_stop_edit();
                return Ok(());
            }
            _ => {}
        }
        app.pr_review_fix_editor_key(key);
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
