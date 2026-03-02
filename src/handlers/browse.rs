use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent};

use crate::app::{App, AppMode, CreateProjectStep};

pub fn handle_browse_path_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let creating_folder = match &app.mode {
        AppMode::BrowsingPath(state) => state.creating_folder,
        _ => false,
    };

    if creating_folder {
        match key.code {
            KeyCode::Esc => {
                if let AppMode::BrowsingPath(state) = &mut app.mode {
                    state.creating_folder = false;
                    state.new_folder_name.clear();
                }
            }
            KeyCode::Enter => {
                app.create_folder_in_browse()?;
            }
            KeyCode::Backspace => {
                if let AppMode::BrowsingPath(state) = &mut app.mode {
                    state.new_folder_name.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::BrowsingPath(state) = &mut app.mode {
                    state.new_folder_name.push(c);
                }
            }
            _ => {}
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => {
            app.cancel_browse_path();
        }
        KeyCode::Tab => {
            app.cancel_browse_path();
            if let AppMode::CreatingProject(state) = &mut app.mode {
                state.step = CreateProjectStep::Name;
            }
        }
        KeyCode::Char(' ') => {
            app.confirm_browse_path();
        }
        KeyCode::Char('c') => {
            if let AppMode::BrowsingPath(state) = &mut app.mode {
                state.creating_folder = true;
            }
        }
        KeyCode::Enter => {
            let is_dir = match &app.mode {
                AppMode::BrowsingPath(state) => state.explorer.current().is_dir(),
                _ => false,
            };
            if is_dir {
                if let AppMode::BrowsingPath(state) = &mut app.mode {
                    let _ = state.explorer.handle(&Event::Key(key));
                }
            } else {
                app.confirm_browse_path();
            }
        }
        _ => {
            if let AppMode::BrowsingPath(state) = &mut app.mode {
                let _ = state.explorer.handle(&Event::Key(key));
            }
        }
    }
    Ok(())
}
