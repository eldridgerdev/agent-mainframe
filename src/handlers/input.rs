use anyhow::Result;

use crate::app::{App, AppMode, CreateFeatureStep, CreateProjectStep};
use crate::tmux::TmuxManager;

pub fn handle_paste(app: &mut App, text: &str) -> Result<()> {
    match &app.mode {
        AppMode::Viewing(view) => {
            let session = view.session.clone();
            let window = view.window.clone();
            TmuxManager::paste_text(&session, &window, text)?;
        }
        AppMode::CreatingProject(_) => {
            if let AppMode::CreatingProject(state) = &mut app.mode {
                match state.step {
                    CreateProjectStep::Name => {
                        state.name.push_str(text);
                    }
                    CreateProjectStep::Path => {
                        state.path.push_str(text);
                    }
                }
            }
        }
        AppMode::CreatingFeature(_) => {
            if let AppMode::CreatingFeature(state) = &mut app.mode {
                if matches!(state.step, CreateFeatureStep::Branch) {
                    state.branch.push_str(text);
                }
            }
        }
        AppMode::RenamingSession(_) => {
            if let AppMode::RenamingSession(state) = &mut app.mode {
                state.input.push_str(text);
            }
        }
        _ => {}
    }
    Ok(())
}
