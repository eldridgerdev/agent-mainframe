use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, PromptEditorFocus, PromptExportTarget};
use crate::editor::VimMode;

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
            KeyCode::Char('w') => app.export_selected_template(PromptExportTarget::Worktree)?,
            KeyCode::Esc => {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.pending_export = false;
                }
                app.message = Some("Export cancelled".into());
            }
            // Navigation dismisses the export prompt and moves the cursor.
            KeyCode::Char('j') | KeyCode::Down => {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.pending_export = false;
                }
                app.message = None;
                app.prompt_library_select_next();
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.pending_export = false;
                }
                app.message = None;
                app.prompt_library_select_prev();
            }
            _ => {
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.pending_export = false;
                }
                app.message = None;
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
                let from_view = match &app.mode {
                    AppMode::PromptLibrary(state) => state.from_view.clone(),
                    _ => None,
                };
                if let AppMode::PromptLibrary(state) = &mut app.mode {
                    state.pending_export = true;
                    state.confirm_delete = false;
                }
                app.message = Some(app.build_export_menu_message(from_view.as_ref()));
            }
        }
        KeyCode::Char('d') => {
            let selected = matches!(&app.mode, AppMode::PromptLibrary(state) if state.selected_entry().is_some());
            let deletable = matches!(&app.mode, AppMode::PromptLibrary(state)
                if state.selected_entry().is_some_and(|e| e.source.is_deletable()));
            if confirm_delete {
                app.delete_selected_template()?;
            } else if selected && !deletable {
                app.push_toast_warning("Config templates can't be deleted here — edit or duplicate (y) instead");
            } else if selected {
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

/// The raw string backing whichever single-line field (Name or Tags) is
/// focused, so char/backspace edits route to the right one. Falls back to
/// the name field when Body is focused (the caller never invokes it then).
fn active_single_line_field(state: &mut crate::app::PromptEditorState) -> &mut String {
    match state.focus {
        PromptEditorFocus::Tags => &mut state.tags,
        _ => &mut state.name,
    }
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
    // Tab cycles Name → Tags → Body; Shift+Tab reverses.
    if key.code == KeyCode::Tab {
        if let AppMode::PromptEditor(state) = &mut app.mode {
            state.focus = state.focus.next();
        }
        return Ok(());
    }
    if key.code == KeyCode::BackTab {
        if let AppMode::PromptEditor(state) = &mut app.mode {
            state.focus = state.focus.prev();
        }
        return Ok(());
    }

    // Name and Tags are single-line text fields edited the same way.
    let single_line = matches!(&app.mode, AppMode::PromptEditor(state)
        if matches!(state.focus, PromptEditorFocus::Name | PromptEditorFocus::Tags));
    if single_line {
        match key.code {
            // Single-line fields; Enter saves the whole template.
            KeyCode::Enter => app.submit_prompt_editor()?,
            KeyCode::Esc => app.cancel_prompt_editor(),
            KeyCode::Backspace => {
                if let AppMode::PromptEditor(state) = &mut app.mode {
                    active_single_line_field(state).pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::PromptEditor(state) = &mut app.mode {
                    active_single_line_field(state).push(c);
                }
            }
            _ => {}
        }
        return Ok(());
    }

    // Body editor: Esc cancels unless vim is in Insert mode (where it just
    // transitions to Normal). A second Esc from Normal mode closes the dialog.
    if key.code == KeyCode::Esc
        && !matches!(&app.mode, AppMode::PromptEditor(state)
            if matches!(state.editor.vim_mode(), Some(VimMode::Insert)))
    {
        app.cancel_prompt_editor();
        return Ok(());
    }
    if let AppMode::PromptEditor(state) = &mut app.mode {
        state.editor.handle_key(key);
    }
    Ok(())
}

/// Fill-in flow: collect a value for each `{{slot}}`, one field at a time.
pub fn handle_placeholder_fill_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // Ctrl+S submits from anywhere (collecting the current field first).
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
        app.submit_placeholder_fill()?;
        return Ok(());
    }

    let multiline = matches!(&app.mode, AppMode::PlaceholderFill(state) if state.current_is_multiline());
    let is_select = matches!(&app.mode, AppMode::PlaceholderFill(state) if state.is_select());

    match key.code {
        KeyCode::Esc => app.cancel_placeholder_fill(),
        KeyCode::BackTab => app.placeholder_fill_prev(),
        KeyCode::Tab => app.placeholder_fill_next()?,
        // On a single-line field Enter advances; on a multi-line field it
        // inserts a newline and is forwarded to the editor below.
        KeyCode::Enter if !multiline => app.placeholder_fill_next()?,
        // Select slots choose from a fixed option list.
        KeyCode::Up | KeyCode::Char('k') if is_select => {
            if let AppMode::PlaceholderFill(state) = &mut app.mode {
                state.select_prev();
            }
        }
        KeyCode::Down | KeyCode::Char('j') if is_select => {
            if let AppMode::PlaceholderFill(state) = &mut app.mode {
                state.select_next();
            }
        }
        // Text / multi-line slots forward to the editor; Select slots ignore
        // any other key (there's nothing to type).
        _ => {
            if !is_select
                && let AppMode::PlaceholderFill(state) = &mut app.mode
            {
                state.input.handle_key(key);
            }
        }
    }
    Ok(())
}
