use anyhow::Result;
use crossterm::event::KeyCode;

use crate::app::App;

pub fn handle_search_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.cancel_search();
        }
        KeyCode::Enter => {
            app.jump_to_search_match();
        }
        KeyCode::Down | KeyCode::Tab => {
            app.select_next_search_match();
        }
        KeyCode::Up | KeyCode::BackTab => {
            app.select_prev_search_match();
        }
        KeyCode::Backspace => {
            if let crate::app::AppMode::Searching(state) = &mut app.mode {
                state.query.pop();
                app.perform_search();
            }
        }
        KeyCode::Char(c) => {
            if let crate::app::AppMode::Searching(state) = &mut app.mode {
                state.query.push(c);
                app.perform_search();
            }
        }
        _ => {}
    }
    Ok(())
}
