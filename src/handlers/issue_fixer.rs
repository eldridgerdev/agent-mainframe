use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;

/// Key handling for the issue browser. Pagination is explicit: left/right
/// changes page, while j/k only changes the selected issue on the page.
pub fn handle_issue_browser_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_issue_browser(),
        KeyCode::Enter => app.issue_browser_open_setup(),
        KeyCode::Down | KeyCode::Char('j') => app.issue_browser_select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.issue_browser_select_prev(),
        KeyCode::Left | KeyCode::Char('h') => app.issue_browser_previous_page(),
        KeyCode::Right | KeyCode::Char('l') => app.issue_browser_next_page(),
        KeyCode::Char('r') => app.refresh_issue_browser(),
        _ => {}
    }
    Ok(())
}

/// Key handling for the issue setup dialog. Name and branch are inline text
/// fields; the prompt uses an explicit edit mode so Enter remains available
/// for confirmation while the prompt itself can contain newlines.
pub fn handle_issue_setup_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if let crate::app::AppMode::IssueSetup(state) = &app.mode
        && state.prompt_editing
    {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q')
                if key.modifiers.contains(KeyModifiers::CONTROL) || key.code == KeyCode::Esc =>
            {
                app.issue_setup_toggle_prompt_editing();
            }
            KeyCode::Tab => app.issue_setup_toggle_prompt_editing(),
            _ => app.issue_setup_prompt_key(key),
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.issue_setup_cancel(),
        KeyCode::Enter => app.issue_setup_confirm()?,
        KeyCode::Down | KeyCode::Char('j') => app.issue_setup_move(1),
        KeyCode::Up | KeyCode::Char('k') => app.issue_setup_move(-1),
        KeyCode::Right => app.issue_setup_adjust(1),
        KeyCode::Left => app.issue_setup_adjust(-1),
        KeyCode::Backspace => app.issue_setup_text_backspace(),
        KeyCode::Char('e')
            if {
                matches!(
                    &app.mode,
                    crate::app::AppMode::IssueSetup(state)
                        if state.focused_is_prompt()
                )
            } =>
        {
            app.issue_setup_toggle_prompt_editing()
        }
        KeyCode::Char(c) => {
            if let crate::app::AppMode::IssueSetup(state) = &app.mode {
                if state.focused_is_text() {
                    app.issue_setup_text_push(c);
                } else {
                    match c {
                        'l' | ' ' => app.issue_setup_adjust(1),
                        'h' => app.issue_setup_adjust(-1),
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn handle_issue_duplicate_warning_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Enter | KeyCode::Char('y') => app.issue_duplicate_override()?,
        KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('q') => app.issue_duplicate_cancel(),
        _ => {}
    }
    Ok(())
}
