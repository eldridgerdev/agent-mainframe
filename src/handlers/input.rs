use anyhow::Result;

use crate::app::{App, AppMode, CreateFeatureStep, CreateProjectStep, PromptEditorFocus};
use crate::tmux::TmuxManager;

pub fn handle_paste(app: &mut App, text: &str) -> Result<()> {
    match &app.mode {
        AppMode::Viewing(view) => {
            if app.compose_intercept_active(view) {
                app.open_compose_from_view(None)?;
                if let AppMode::Compose(state) = &mut app.mode {
                    let outcome = state.editor.insert_str(text);
                    if outcome.text_changed {
                        state.refresh_suggestions();
                        state.request_cursor_scroll();
                    }
                }
            } else {
                let session = view.session.clone();
                let window = view.window.clone();
                TmuxManager::paste_text(&session, &window, text)?;
            }
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
                    CreateProjectStep::Agent => {}
                }
            }
            app.refresh_create_project_agent_selection();
        }
        AppMode::CreatingFeature(_) => {
            if let AppMode::CreatingFeature(state) = &mut app.mode {
                match state.step {
                    CreateFeatureStep::Branch => {
                        state.branch.push_str(text);
                        state.branch_error = None;
                    }
                    CreateFeatureStep::TaskPrompt => {
                        state.task_prompt.push_str(text);
                        state.refresh_prompt_analysis();
                    }
                    CreateFeatureStep::SessionName => {
                        state.session_name.push_str(text);
                    }
                    _ => {}
                }
            }
        }
        AppMode::PlanInterview(_) => {
            if let AppMode::PlanInterview(state) = &mut app.mode
                && !state.abort_confirmation
                && !matches!(
                    state.current_question().map(|question| &question.kind),
                    Some(crate::plan_interview::PlanQuestionKind::Select(_))
                )
                && state.phase != crate::app::PlanInterviewPhase::Done
            {
                state.editor.insert_str(text);
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
                let outcome = state.editor.insert_str(text);
                if outcome.text_changed {
                    state.refresh_prompt_analysis();
                }
            }
        }
        AppMode::Compose(_) => {
            if let AppMode::Compose(state) = &mut app.mode {
                let outcome = state.editor.insert_str(text);
                if outcome.text_changed {
                    state.refresh_suggestions();
                    state.request_cursor_scroll();
                }
            }
        }
        AppMode::PromptEditor(_) => {
            if let AppMode::PromptEditor(state) = &mut app.mode {
                match state.focus {
                    // Name and Tags are single-line; collapse pasted newlines.
                    PromptEditorFocus::Name | PromptEditorFocus::Tags => {
                        let field = match state.focus {
                            PromptEditorFocus::Tags => &mut state.tags,
                            _ => &mut state.name,
                        };
                        for chunk in text.split(['\n', '\r']) {
                            field.push_str(chunk);
                        }
                    }
                    PromptEditorFocus::Body => {
                        state.editor.insert_str(text);
                    }
                }
            }
        }
        AppMode::PlaceholderFill(_) => {
            if let AppMode::PlaceholderFill(state) = &mut app.mode {
                // Select slots have no text field to paste into.
                if !state.is_select() {
                    state.input.insert_str(text);
                }
            }
        }
        _ => {}
    }
    Ok(())
}
