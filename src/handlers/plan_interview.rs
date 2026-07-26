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

    let phase = match &app.mode {
        AppMode::PlanInterview(state) => state.phase,
        _ => return Ok(()),
    };
    if phase == PlanInterviewPhase::Editing {
        return handle_plan_edit_key(app, key);
    }
    if phase == PlanInterviewPhase::Review {
        return handle_plan_review_key(app, key);
    }

    let is_select = matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if matches!(state.current_question().map(|q| &q.kind), Some(PlanQuestionKind::Select(_)))
    );
    let accepts_text = matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Brief
                || matches!(
                    state.current_question().map(|q| &q.kind),
                    Some(PlanQuestionKind::FreeText)
                )
    );
    let is_ai_consent = matches!(
        &app.mode,
        AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::AiConsent
    );
    let control = key.modifiers.contains(KeyModifiers::CONTROL);

    match key.code {
        KeyCode::Esc => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.abort_confirmation = true;
            }
            app.message = None;
        }
        KeyCode::Char('a') if is_ai_consent && key.modifiers.is_empty() => {
            let opted_in = match &mut app.mode {
                AppMode::PlanInterview(state) => state.opt_in_ai_followups(),
                _ => false,
            };
            if opted_in {
                app.message = None;
                app.continue_plan_interview_after_done()?;
            }
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
                app.continue_plan_interview_after_done()?;
            }
        }
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::ALT) && accepts_text => {
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
                app.continue_plan_interview_after_done()?;
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
                app.continue_plan_interview_after_done()?;
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
        _ if accepts_text && !is_select => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.editor.handle_key(key);
            }
        }
        _ => {}
    }
    Ok(())
}

const REVIEW_FAST_SCROLL_STEP: usize = 8;

fn handle_plan_review_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.abort_confirmation = true;
            }
            app.message = None;
        }
        KeyCode::Enter => {
            if let Err(error) = app.complete_plan_interview() {
                app.report_logged_error(
                    "plan_interview",
                    format!("Failed to accept plan interview: {error}"),
                );
            }
        }
        KeyCode::Char('e') if key.modifiers.is_empty() => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.begin_plan_edit();
            }
            app.message = None;
        }
        KeyCode::Char('r') if key.modifiers.is_empty() => {
            app.message = None;
            app.start_plan_interview_synthesis()?;
        }
        KeyCode::Char('j') | KeyCode::Down if control => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.review_scroll_offset = state
                    .review_scroll_offset
                    .saturating_add(REVIEW_FAST_SCROLL_STEP);
            }
        }
        KeyCode::Char('k') | KeyCode::Up if control => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.review_scroll_offset = state
                    .review_scroll_offset
                    .saturating_sub(REVIEW_FAST_SCROLL_STEP);
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.review_scroll_offset = state.review_scroll_offset.saturating_add(1);
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.review_scroll_offset = state.review_scroll_offset.saturating_sub(1);
            }
        }
        KeyCode::PageDown => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.review_scroll_offset = state.review_scroll_offset.saturating_add(10);
            }
        }
        KeyCode::PageUp => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.review_scroll_offset = state.review_scroll_offset.saturating_sub(10);
            }
        }
        KeyCode::Home | KeyCode::Char('g') => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.review_scroll_offset = 0;
            }
        }
        KeyCode::End | KeyCode::Char('G') => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.review_scroll_offset = usize::MAX;
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_plan_edit_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.cancel_plan_edit();
            }
            app.message = None;
        }
        KeyCode::Char('s') if control => {
            let saved = match &mut app.mode {
                AppMode::PlanInterview(state) => state.save_plan_edit(),
                _ => false,
            };
            app.message = if saved {
                None
            } else {
                Some("Error: plan markdown cannot be empty".into())
            };
        }
        _ => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                let outcome = state.editor.handle_key(key);
                if outcome.text_changed || outcome.cursor_moved {
                    state.edit_sync_to_cursor = true;
                }
            }
            app.message = None;
        }
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
