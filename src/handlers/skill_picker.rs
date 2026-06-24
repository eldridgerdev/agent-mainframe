use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode};

/// Skill picker: a search-as-you-type list of the workspace's agent skills.
/// Typing filters; Enter inserts the highlighted skill's `/skill-name`
/// invocation back into the editor it was opened from.
pub fn handle_skill_picker_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // Ctrl+n / Ctrl+p navigate without leaving the query line.
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Char('n') => app.skill_picker_select_next(),
            KeyCode::Char('p') => app.skill_picker_select_prev(),
            _ => {}
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => app.cancel_skill_picker(),
        KeyCode::Enter | KeyCode::Tab => app.insert_selected_skill(),
        KeyCode::Down => app.skill_picker_select_next(),
        KeyCode::Up => app.skill_picker_select_prev(),
        KeyCode::Backspace => {
            if let AppMode::SkillPicker(state) = &mut app.mode {
                state.query.pop();
            }
            app.skill_picker_filter();
        }
        KeyCode::Char(c) => {
            if let AppMode::SkillPicker(state) = &mut app.mode {
                state.query.push(c);
            }
            app.skill_picker_filter();
        }
        _ => {}
    }
    Ok(())
}
