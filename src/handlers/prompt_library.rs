use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, PromptExportTarget};

/// Prompt-library picker: navigate, filter, inject, and manage templates.
pub fn handle_prompt_library_key(app: &mut App, key: KeyCode) -> Result<()> {
    // While the search line is active, typing edits the query.
    let search_active = matches!(&app.mode, AppMode::PromptLibrary(state) if state.search_active);
    if search_active {
        match key {
            KeyCode::Esc => {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.search_active = false;
                    state.query.clear();
                }
                app.prompt_library_filter();
            }
            KeyCode::Enter => {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.search_active = false;
                }
            }
            KeyCode::Backspace => {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.query.pop();
                }
                app.prompt_library_filter();
            }
            KeyCode::Down => app.prompt_library_select_next(),
            KeyCode::Up => app.prompt_library_select_prev(),
            KeyCode::Char(c) => {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.query.push(c);
                }
                app.prompt_library_filter();
            }
            _ => {}
        }
        return Ok(());
    }

    // A pending export choice (after `x`) intercepts its target keys.
    let pending_export = matches!(&app.mode, AppMode::PromptLibrary(state) if state.pending_export);
    if pending_export {
        match key {
            KeyCode::Char('g') => app.export_selected_template(PromptExportTarget::Global)?,
            KeyCode::Char('p') => app.export_selected_template(PromptExportTarget::Project)?,
            KeyCode::Esc => {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.pending_export = false;
                }
                app.message = Some("Export cancelled".into());
            }
            _ => {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.pending_export = false;
                }
            }
        }
        return Ok(());
    }

    // A pending delete confirm intercepts Esc/d before anything else.
    let confirm_delete = matches!(&app.mode, AppMode::PromptLibrary(state) if state.confirm_delete);

    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            if confirm_delete {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.confirm_delete = false;
                }
                app.message = Some("Delete cancelled".into());
                return Ok(());
            }
            let from_view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::PromptLibrary(state) => state.from_view,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            match from_view {
                Some(view) => app.mode = AppMode::Viewing(view),
                None => app.mode = AppMode::Normal,
            }
        }
        KeyCode::Char('j') | KeyCode::Down => app.prompt_library_select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.prompt_library_select_prev(),
        KeyCode::Char('/') => {
            if let AppMode::PromptLibrary(state) = &mut app.mode {
                state.search_active = true;
                state.confirm_delete = false;
            }
        }
        KeyCode::Enter | KeyCode::Tab => app.inject_selected_template()?,
        KeyCode::Char('n') => app.start_new_prompt_template(),
        KeyCode::Char('e') => app.start_edit_selected_template(),
        KeyCode::Char('y') => app.duplicate_selected_template_to_user()?,
        KeyCode::Char('x') => {
            let has_selection =
                matches!(&app.mode, AppMode::PromptLibrary(state) if state.selected_entry().is_some());
            if has_selection {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.pending_export = true;
                    state.confirm_delete = false;
                }
                app.message =
                    Some("Export to: (g) global  (p) project  ·  Esc cancel".into());
            }
        }
        KeyCode::Char('d') => {
            let has_selection =
                matches!(&app.mode, AppMode::PromptLibrary(state) if state.selected_entry().is_some());
            if confirm_delete {
                app.delete_selected_template()?;
            } else if has_selection {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.confirm_delete = true;
                }
                app.message = Some("Press d again to delete, Esc to cancel".into());
            }
        }
        _ => {
            if let AppMode::PromptLibrary(state) = &mut app.mode {
                state.confirm_delete = false;
            }
        }
    }
    Ok(())
}

/// Prompt-template editor: name field + multi-line body editor.
pub fn handle_prompt_editor_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
        app.submit_prompt_editor()?;
        return Ok(());
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        app.cancel_prompt_editor();
        return Ok(());
    }
    if key.code == KeyCode::Tab {
        if let AppMode::PromptEditor(state) = &mut app.mode {
            state.name_field_active = !state.name_field_active;
        }
        return Ok(());
    }

    let name_active = matches!(&app.mode, AppMode::PromptEditor(state) if state.name_field_active);
    if name_active {
        match key.code {
            // Name is a single line; Enter saves the whole template.
            KeyCode::Enter => app.submit_prompt_editor()?,
            KeyCode::Esc => app.cancel_prompt_editor(),
            KeyCode::Backspace => {
                if let AppMode::PromptEditor(state) = &mut app.mode {
                    state.name.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::PromptEditor(state) = &mut app.mode {
                    state.name.push(c);
                }
            }
            _ => {}
        }
        return Ok(());
    }

    // Body editor: Esc (outside vim) cancels; everything else, including
    // Enter (newline), goes to the TextEditor.
    if key.code == KeyCode::Esc
        && matches!(&app.mode, AppMode::PromptEditor(state) if state.editor.vim_mode().is_none())
    {
        app.cancel_prompt_editor();
        return Ok(());
    }
    if let AppMode::PromptEditor(state) = &mut app.mode {
        state.editor.handle_key(key);
    }
    Ok(())
}
