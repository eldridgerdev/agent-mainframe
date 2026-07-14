//! Key handling for the native plan-mode discovery interview.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, PlanInterviewAdvanceError, PlanInterviewPhase};
use crate::plan_interview::PlanQuestionKind;

pub fn handle_plan_interview_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let confirming_abort =
        matches!(&app.mode, AppMode::PlanInterview(state) if state.abort_confirmation);
    if confirming_abort {
        match key.code {
            KeyCode::Char('y') => app.launch_plan_interview_without_plan()?,
            KeyCode::Char('n') => app.cancel_plan_interview_feature()?,
            KeyCode::Esc => {
                if let AppMode::PlanInterview(state) = &mut app.mode {
                    state.abort_confirmation = false;
                }
                app.message = None;
            }
            _ => {}
        }
        return Ok(());
    }

    let is_select = matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if matches!(state.current_question().map(|q| &q.kind), Some(PlanQuestionKind::Select(_)))
    );
    let control = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Esc => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.abort_confirmation = true;
            }
            app.message = None;
        }
        KeyCode::Enter if !key.modifiers.contains(KeyModifiers::ALT) => {
            let result = match &mut app.mode {
                AppMode::PlanInterview(state) => state.advance(),
                _ => return Ok(()),
            };
            let completed = result.is_ok()
                && matches!(&app.mode, AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Done);
            set_advance_message(app, result);
            if completed {
                app.complete_plan_interview()?;
            }
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) && !is_select => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state
                    .editor
                    .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            }
        }
        KeyCode::Char('b') if control => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                if !state.back() {
                    app.message = Some("Already at the feature brief".into());
                } else {
                    app.message = None;
                }
            }
        }
        KeyCode::Char('s') if control => {
            let result = match &mut app.mode {
                AppMode::PlanInterview(state) => state.skip(),
                _ => return Ok(()),
            };
            let completed = result.is_ok()
                && matches!(&app.mode, AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Done);
            set_advance_message(app, result);
            if completed {
                app.complete_plan_interview()?;
            }
        }
        KeyCode::Char('f') if control => {
            let result = match &mut app.mode {
                AppMode::PlanInterview(state) => state.finish_early(),
                _ => return Ok(()),
            };
            let completed = result.is_ok()
                && matches!(&app.mode, AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Done);
            set_advance_message(app, result);
            if completed {
                app.complete_plan_interview()?;
            }
        }
        KeyCode::Up | KeyCode::Char('k') if is_select => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.select_previous_option();
            }
        }
        KeyCode::Down | KeyCode::Char('j') if is_select => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.select_next_option();
            }
        }
        _ if !is_select => {
            if let AppMode::PlanInterview(state) = &mut app.mode
                && state.phase != PlanInterviewPhase::Done
            {
                state.editor.handle_key(key);
            }
        }
        _ => {}
    }
    Ok(())
}

fn set_advance_message(app: &mut App, result: Result<(), PlanInterviewAdvanceError>) {
    app.message = match result {
        Ok(()) => None,
        Err(PlanInterviewAdvanceError::BriefRequired) => {
            Some("Error: describe the feature before continuing".into())
        }
        Err(PlanInterviewAdvanceError::AnswerRequired) => {
            Some("Error: this question requires an answer".into())
        }
    };
}
