//! Key dispatch for the native TODOs overlay (`AppMode::Todos`).
//!
//! Five input layers, checked in order: a pending delete confirmation, the
//! launch chooser / destination step, the move/copy scope chooser, an active
//! inline edit (add / title / notes / scratchpad), and the normal navigation +
//! action keys — which now also move focus between panes (`Tab`), reveal them
//! independently (`p` / `g`), and re-file the selected item across scopes
//! (`M` / `C`).

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode};

pub fn handle_todos_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // Layer 1: delete confirmation.
    if matches!(&app.mode, AppMode::Todos(state) if state.pending_delete) {
        match key.code {
            KeyCode::Char('y') => app.todos_confirm_delete()?,
            KeyCode::Char('n') | KeyCode::Esc => app.todos_cancel_delete(),
            _ => {}
        }
        return Ok(());
    }

    // Layer 2: the launch chooser / destination step.
    if matches!(&app.mode, AppMode::Todos(state) if state.launch.is_some()) {
        return handle_launch_step_key(app, key.code);
    }

    // Layer 3: the move/copy scope chooser.
    if matches!(&app.mode, AppMode::Todos(state) if state.scope_move.is_some()) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => app.todo_scope_move_cursor(1),
            KeyCode::Char('k') | KeyCode::Up => app.todo_scope_move_cursor(-1),
            KeyCode::Enter => app.confirm_todo_scope_move()?,
            KeyCode::Esc | KeyCode::Char('q') => app.cancel_todo_scope_move(),
            _ => {}
        }
        return Ok(());
    }

    // Layer 4: active inline edit.
    if matches!(&app.mode, AppMode::Todos(state) if state.editor.is_some()) {
        return handle_edit_key(app, key);
    }

    // Layer 5: navigation + actions.
    // Ctrl+Q exits, matching the embedded-view exit chord.
    if key.code == KeyCode::Char('q') && key.modifiers.contains(KeyModifiers::CONTROL) {
        app.close_todos_view();
        return Ok(());
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_todos_view(),
        KeyCode::Down | KeyCode::Char('j') => app.todos_select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.todos_select_prev(),
        KeyCode::Char('a') | KeyCode::Char('n') => app.todos_begin_add(),
        KeyCode::Char('e') => app.todos_begin_edit_title(),
        KeyCode::Char('o') => app.todos_begin_edit_notes(),
        KeyCode::Char('b') => app.todos_begin_edit_scratchpad(),
        KeyCode::Char(' ') | KeyCode::Char('x') => app.todos_toggle_done()?,
        KeyCode::Char('i') => app.todos_toggle_in_progress()?,
        KeyCode::Char('P') => app.todos_cycle_priority()?,
        KeyCode::Char('J') => app.todos_reorder(1)?,
        KeyCode::Char('K') => app.todos_reorder(-1)?,
        KeyCode::Char('d') => app.todos_request_delete(),
        KeyCode::Enter => app.todos_launch_selected()?,
        // Pane focus and independent project/global visibility. `BackTab` is
        // what a terminal reports for Shift+Tab.
        KeyCode::Tab => app.todos_cycle_focus(1),
        KeyCode::BackTab => app.todos_cycle_focus(-1),
        KeyCode::Char('p') => app.todos_toggle_project_visibility(),
        KeyCode::Char('g') => app.todos_toggle_global_visibility(),
        // Re-file the selected item into another scope: `M` moves it (links
        // and all), `C` leaves a copy behind as fresh, unstarted work.
        KeyCode::Char('M') => app.todos_begin_scope_move(false),
        KeyCode::Char('C') => app.todos_begin_scope_move(true),
        // Distinct from `Enter`: that acts on the cursor, while this picks the
        // next TODO in priority order wherever it is in the view.
        KeyCode::Char('I') => app.implement_next_todo_in_overlay()?,
        _ => {}
    }
    Ok(())
}

/// Key dispatch for the one-line TODO quick-capture (`AppMode::TodoQuickCapture`).
pub fn handle_todo_quick_capture_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.cancel_todo_quick_capture(),
        KeyCode::Enter => app.commit_todo_quick_capture()?,
        KeyCode::Backspace => {
            if let AppMode::TodoQuickCapture(state) = &mut app.mode {
                state.input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::TodoQuickCapture(state) = &mut app.mode {
                state.input.push(c);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Key dispatch for the "re-home or delete the TODO list" prompt shown when a
/// list's host feature is deleted (`AppMode::TodosHostReassign`).
pub fn handle_todos_host_reassign_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('j') | KeyCode::Down => app.todos_host_reassign_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.todos_host_reassign_move(-1),
        KeyCode::Enter => app.confirm_todos_host_reassign()?,
        // Esc keeps the list by re-homing onto the first surviving feature.
        KeyCode::Esc => app.cancel_todos_host_reassign()?,
        _ => {}
    }
    Ok(())
}

/// Key dispatch for the "work has already started on this TODO" prompt raised
/// by `I` (`AppMode::TodoImplementChoice`).
///
/// `Esc` is *Cancel*: it restores the mode the key was pressed in, so the
/// prompt never costs the user their place in the list.
pub fn handle_todo_implement_choice_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('j') | KeyCode::Down => app.todo_implement_choice_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.todo_implement_choice_move(-1),
        KeyCode::Enter => app.confirm_todo_implement_choice()?,
        KeyCode::Esc | KeyCode::Char('q') => app.cancel_todo_implement_choice(),
        _ => {}
    }
    Ok(())
}

/// Key dispatch for the launch step layered over the list: the chooser shown
/// by `g`/`Enter` on an unlinked TODO, and the destination step after it.
///
/// `Esc` unwinds one step at a time — destination back to chooser, chooser back
/// to the list — so reaching the interview by mistake is always one keypress
/// from where the user was.
fn handle_launch_step_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('j') | KeyCode::Down => app.todo_launch_step_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.todo_launch_step_move(-1),
        KeyCode::Enter => app.confirm_todo_launch_step()?,
        KeyCode::Esc | KeyCode::Char('q') => app.cancel_todo_launch_step(),
        _ => {}
    }
    Ok(())
}

fn handle_edit_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        // Alt+Enter inserts a newline (notes are multi-line); plain Enter
        // commits. Mirrors the compose editor.
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
            if let AppMode::Todos(state) = &mut app.mode
                && let Some(ed) = &mut state.editor
            {
                ed.editor.insert_str("\n");
            }
        }
        KeyCode::Enter => app.todos_commit_edit()?,
        KeyCode::Esc => app.todos_cancel_edit(),
        _ => {
            if let AppMode::Todos(state) = &mut app.mode
                && let Some(ed) = &mut state.editor
            {
                ed.editor.handle_key(key);
            }
        }
    }
    Ok(())
}

/// Key dispatch for the "which feature should work this TODO?" picker raised
/// by a spawn from the project or global pane (`AppMode::TodoSpawnTarget`).
///
/// `Esc` restores the mode the key was pressed in, so declining to choose
/// never costs the user their place in the list.
pub fn handle_todo_spawn_target_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('j') | KeyCode::Down => app.todo_spawn_target_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.todo_spawn_target_move(-1),
        KeyCode::Enter => app.confirm_todo_spawn_target()?,
        KeyCode::Esc | KeyCode::Char('q') => app.cancel_todo_spawn_target(),
        _ => {}
    }
    Ok(())
}

/// Key dispatch for the "what happens to this worktree's TODOs?" prompt raised
/// before a feature with unfinished worktree TODOs is deleted
/// (`AppMode::TodoDeleteDisposition`).
///
/// `Esc` is *Cancel*: the feature and its worktree stay. There is no `q` here —
/// this prompt stands between the user and an irreversible delete, and a
/// stray `q` should not be the key that dismisses it.
pub fn handle_todo_delete_disposition_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('j') | KeyCode::Down => app.todo_delete_disposition_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.todo_delete_disposition_move(-1),
        KeyCode::Enter => app.confirm_todo_delete_disposition()?,
        KeyCode::Esc => app.cancel_todo_delete_disposition(),
        _ => {}
    }
    Ok(())
}
