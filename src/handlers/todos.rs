//! Key dispatch for the native TODOs overlay (`AppMode::Todos`).
//!
//! Three input layers, checked in order: a pending delete confirmation, an
//! active inline edit (add / title / notes / carry-over), and the normal
//! navigation + action keys.

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

    // Layer 2: active inline edit.
    if matches!(&app.mode, AppMode::Todos(state) if state.editor.is_some()) {
        return handle_edit_key(app, key);
    }

    // Layer 3: navigation + actions.
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
        KeyCode::Char('p') => app.todos_cycle_priority()?,
        KeyCode::Char('J') => app.todos_reorder(1)?,
        KeyCode::Char('K') => app.todos_reorder(-1)?,
        KeyCode::Char('d') => app.todos_request_delete(),
        KeyCode::Char('g') | KeyCode::Enter => app.todos_spawn_agent()?,
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
