use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode};

const COMPOSE_PAGE_SCROLL: usize = 10;

pub fn handle_compose_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(' ') {
        // Hand off to the leader menu: close the composer (draft is
        // kept) and activate the leader in the underlying view.
        app.cancel_compose();
        app.activate_leader();
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        app.cancel_compose();
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('e') {
        app.compose_switch_to_direct();
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('v') {
        match crate::app::util::read_clipboard() {
            Ok(crate::app::util::ClipboardContent::Text(text)) if !text.is_empty() => {
                if let AppMode::Compose(state) = &mut app.mode {
                    let outcome = state.editor.insert_str(&text);
                    if outcome.text_changed {
                        state.refresh_suggestions();
                        state.request_cursor_scroll();
                    }
                }
            }
            Ok(crate::app::util::ClipboardContent::Image { data, mime }) => {
                let placeholder = if let AppMode::Compose(state) = &mut app.mode {
                    let placeholder = state.add_image(data, mime);
                    state.editor.insert_str(&placeholder);
                    state.refresh_suggestions();
                    state.request_cursor_scroll();
                    Some(placeholder)
                } else {
                    None
                };
                if let Some(placeholder) = placeholder {
                    app.push_toast_success(format!("Attached {placeholder} from clipboard"));
                }
            }
            Ok(_) => {}
            Err(e) => {
                app.push_toast_warning(format!("Clipboard error: {e}"));
            }
        }
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t') {
        if let AppMode::Compose(state) = &mut app.mode {
            state.editor.toggle_vim();
            app.message = Some(if state.editor.vim_mode().is_some() {
                "Vim mode enabled".into()
            } else {
                "Vim mode disabled".into()
            });
        }
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('l') {
        if let AppMode::Compose(state) = &mut app.mode {
            state.clear_prompt();
            app.push_toast_success("Compose input cleared");
        }
        return Ok(());
    }

    if let AppMode::Compose(state) = &mut app.mode {
        match key.code {
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.scroll_down(1);
                return Ok(());
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.scroll_up(1);
                return Ok(());
            }
            KeyCode::PageDown => {
                state.scroll_down(COMPOSE_PAGE_SCROLL);
                return Ok(());
            }
            KeyCode::PageUp => {
                state.scroll_up(COMPOSE_PAGE_SCROLL);
                return Ok(());
            }
            _ => {}
        }

        // Suggestion navigation while the /command popup is visible.
        if !state.suggestions.is_empty() {
            match key.code {
                KeyCode::Down => {
                    state.select_next_suggestion();
                    return Ok(());
                }
                KeyCode::Up => {
                    state.select_prev_suggestion();
                    return Ok(());
                }
                KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.select_next_suggestion();
                    return Ok(());
                }
                KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    state.select_prev_suggestion();
                    return Ok(());
                }
                KeyCode::Tab => {
                    state.complete_selected_suggestion();
                    return Ok(());
                }
                _ => {}
            }
        }
    }

    match key.code {
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) => {
            if let AppMode::Compose(state) = &mut app.mode {
                let outcome = state.editor.insert_str("\n");
                if outcome.text_changed {
                    state.refresh_suggestions();
                    state.request_cursor_scroll();
                }
            }
        }
        KeyCode::Enter => {
            // Mirror CC's menu: Enter on an incomplete /prefix runs the
            // highlighted suggestion rather than the partial text.
            if let AppMode::Compose(state) = &mut app.mode
                && state.pending_command_prefix().is_some()
                && state.exact_command_match().is_none()
                && state.selected_suggestion().is_some()
            {
                state.complete_selected_suggestion();
            }
            app.submit_compose()?;
        }
        KeyCode::Esc if matches!(&app.mode, AppMode::Compose(state) if state.editor.vim_mode().is_none()) =>
        {
            app.cancel_compose();
        }
        _ => {
            if let AppMode::Compose(state) = &mut app.mode {
                let outcome = state.editor.handle_key(key);
                if outcome.text_changed {
                    state.refresh_suggestions();
                }
                if outcome.text_changed || outcome.cursor_moved {
                    state.request_cursor_scroll();
                }
            }
        }
    }
    Ok(())
}
