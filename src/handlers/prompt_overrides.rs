use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, PromptOverrideStep};
use crate::editor::VimMode;

/// The prompt-override manager overlay. Layered: help → editor (typing → scope
/// picker → harness picker) → list.
pub fn handle_prompt_overrides_key(app: &mut App, key: KeyEvent, visible_rows: u16) -> Result<()> {
    let AppMode::PromptOverrides(state) = &app.mode else {
        return Ok(());
    };
    // The list occupies the modal body minus its header + footer chrome.
    let list_height = (visible_rows as usize).saturating_sub(6).max(1);

    if state.help_open {
        app.prompt_overrides_toggle_help();
        return Ok(());
    }

    // ── Editor open ──────────────────────────────────────────────────────
    if let Some(edit) = &state.edit {
        let step = edit.step;
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if step == PromptOverrideStep::Editing {
            if ctrl && key.code == KeyCode::Char('t') {
                app.prompt_overrides_editor_toggle_vim();
                return Ok(());
            }
            if ctrl && key.code == KeyCode::Char('s') {
                app.prompt_overrides_editor_advance();
                return Ok(());
            }
            if ctrl && key.code == KeyCode::Char('q') {
                app.prompt_overrides_cancel_edit();
                return Ok(());
            }
            let vim_insert = matches!(
                &app.mode,
                AppMode::PromptOverrides(s)
                    if s.edit.as_ref().is_some_and(|e| e.editor.vim_mode() == Some(VimMode::Insert))
            );
            if key.code == KeyCode::Esc && !vim_insert {
                app.prompt_overrides_cancel_edit();
                return Ok(());
            }
            if let AppMode::PromptOverrides(s) = &mut app.mode
                && let Some(e) = &mut s.edit
            {
                e.editor.handle_key(key);
            }
            return Ok(());
        }

        // Scope / harness pickers.
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => app.prompt_overrides_picker_move(1),
            KeyCode::Char('k') | KeyCode::Up => app.prompt_overrides_picker_move(-1),
            KeyCode::Enter => {
                if step == PromptOverrideStep::ScopePicker {
                    app.prompt_overrides_editor_advance();
                } else {
                    app.prompt_overrides_confirm_save()?;
                }
            }
            KeyCode::Esc => app.prompt_overrides_picker_back(),
            _ if ctrl && key.code == KeyCode::Char('q') => app.prompt_overrides_picker_back(),
            _ => {}
        }
        return Ok(());
    }

    // ── List view ───────────────────────────────────────────────────────
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            app.prompt_overrides_select(1);
            app.prompt_overrides_clamp_scroll(list_height);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.prompt_overrides_select(-1);
            app.prompt_overrides_clamp_scroll(list_height);
        }
        KeyCode::Enter | KeyCode::Char('e') => app.prompt_overrides_start_edit(),
        KeyCode::Char('d') => app.prompt_overrides_clear_selected()?,
        KeyCode::Char('?') | KeyCode::Char('h') => app.prompt_overrides_toggle_help(),
        KeyCode::Esc | KeyCode::Char('q') => app.prompt_overrides_close(),
        _ => {
            if let AppMode::PromptOverrides(s) = &mut app.mode {
                s.confirm_clear = false;
            }
        }
    }
    Ok(())
}
