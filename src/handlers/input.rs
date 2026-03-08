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
                match state.step {
                    CreateFeatureStep::Branch => {
                        state.branch.push_str(text);
                    }
                    CreateFeatureStep::TaskPrompt => {
                        state.task_prompt.push_str(text);
                        state.refresh_prompt_analysis();
                    }
                    _ => {}
                }
            }
        }
        AppMode::RenamingSession(_) => {
            if let AppMode::RenamingSession(state) = &mut app.mode {
                state.input.push_str(text);
            }
        }
        AppMode::RenamingFeature(_) => {
            if let AppMode::RenamingFeature(state) = &mut app.mode {
                state.input.push_str(text);
            }
        }
        AppMode::Searching(_) => {
            if let AppMode::Searching(state) = &mut app.mode {
                state.query.push_str(text);
                app.perform_search();
            }
        }
        AppMode::SteeringPrompt(_) => {
            if let AppMode::SteeringPrompt(state) = &mut app.mode {
                state.prompt.push_str(text);
                state.prompt_analysis = crate::app::analyze_prompt(&state.prompt);
            }
        }
        _ => {}
    }
    Ok(())
}
