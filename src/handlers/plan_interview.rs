//! Key handling for the native plan-mode discovery interview.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, PlanInterviewAdvanceError, PlanInterviewPhase};
use crate::plan_interview::PlanQuestionKind;

pub fn handle_plan_interview_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let confirming_abort =
        matches!(&app.mode, AppMode::PlanInterview(state) if state.abort_confirmation);
    // Only a feature-creation interview has a launch to cancel; for an
    // on-demand one `n` is not offered, so it must not fall through to a
    // handler that would exit the interview anyway.
    let has_pending_launch =
        matches!(&app.mode, AppMode::PlanInterview(state) if state.pending_launch.is_some());
    if confirming_abort {
        match key.code {
            KeyCode::Char('y') => app.launch_plan_interview_without_plan()?,
            KeyCode::Char('n') if has_pending_launch => app.cancel_plan_interview_feature()?,
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
    if phase == PlanInterviewPhase::KickoffHandoff {
        return handle_plan_kickoff_handoff_key(app, key);
    }
    if phase == PlanInterviewPhase::ResumePrompt {
        return handle_plan_resume_key(app, key);
    }
    if phase == PlanInterviewPhase::Editing {
        return handle_plan_edit_key(app, key);
    }
    if matches!(
        phase,
        PlanInterviewPhase::DirectedFeedback | PlanInterviewPhase::DirectedFeedbackLoading
    ) {
        return handle_plan_directed_feedback_key(app, key);
    }
    if matches!(
        phase,
        PlanInterviewPhase::Investigation | PlanInterviewPhase::InvestigationLoading
    ) {
        return handle_plan_investigation_key(app, key);
    }
    if phase == PlanInterviewPhase::Review {
        return handle_plan_review_key(app, key);
    }
    if matches!(
        phase,
        PlanInterviewPhase::Critique | PlanInterviewPhase::CritiqueLoading
    ) {
        return handle_plan_critique_key(app, key);
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
            let recorded = result.is_ok();
            let completed = recorded
                && matches!(&app.mode, AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Done);
            set_advance_message(app, result);
            if recorded {
                app.persist_plan_interview_draft();
            }
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
            let moved = match &mut app.mode {
                AppMode::PlanInterview(state) => state.back(),
                _ => false,
            };
            app.message = if moved {
                // Stepping back keeps the answer that was on screen, so the
                // draft has to record it before the editor is reloaded.
                app.persist_plan_interview_draft();
                None
            } else {
                Some("Already at the feature brief".into())
            };
        }
        KeyCode::Char('s') if control => {
            let result = match &mut app.mode {
                AppMode::PlanInterview(state) => state.skip(),
                _ => return Ok(()),
            };
            let recorded = result.is_ok();
            let completed = recorded
                && matches!(&app.mode, AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Done);
            set_advance_message(app, result);
            if recorded {
                app.persist_plan_interview_draft();
            }
            if completed {
                app.continue_plan_interview_after_done()?;
            }
        }
        // Re-run only: put back the answer the previous interview accepted for
        // this question. The pre-fill makes keeping an answer the default, so
        // this is what makes changing one's mind about a change cheap.
        KeyCode::Char('r') if control => {
            let restored = match &mut app.mode {
                AppMode::PlanInterview(state) => state.restore_prior_answer(),
                _ => false,
            };
            app.message = if restored {
                None
            } else {
                Some("No previous answer to restore for this question".into())
            };
        }
        KeyCode::Char('f') if control => {
            let result = match &mut app.mode {
                AppMode::PlanInterview(state) => state.finish_early(),
                _ => return Ok(()),
            };
            let recorded = result.is_ok();
            let completed = recorded
                && matches!(&app.mode, AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Done);
            set_advance_message(app, result);
            if recorded {
                app.persist_plan_interview_draft();
            }
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

/// The resume-or-discard choice shown when the interview finds a saved draft.
///
/// Deliberately the first thing the user sees and deliberately explicit: resume
/// silently would overwrite a blank interview with stale answers, and discard
/// silently would throw away work they never chose to abandon. `Esc` keeps the
/// draft and falls through to the interview's normal abort choice.
fn handle_plan_resume_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Char('r') | KeyCode::Enter => app.resume_plan_interview_draft()?,
        KeyCode::Char('d') => app.discard_plan_interview_draft(),
        KeyCode::Esc => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.abort_confirmation = true;
            }
            app.message = None;
        }
        _ => {}
    }
    Ok(())
}

/// The offer to hand an accepted on-demand plan to the feature's already
/// running agent session.
///
/// Every key here is safe: the plan is written and the instruction block points
/// at it before this prompt appears, so declining only means the running session
/// is not interrupted. `Esc` therefore declines rather than opening the abort
/// confirmation — there is no longer anything to abort.
fn handle_plan_kickoff_handoff_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Char('y') | KeyCode::Enter => app.send_plan_kickoff_to_live_session()?,
        KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => {
            app.dismiss_plan_kickoff_handoff()
        }
        _ => {}
    }
    Ok(())
}

const REVIEW_FAST_SCROLL_STEP: usize = 8;

fn handle_plan_review_key(app: &mut App, key: KeyEvent) -> Result<()> {
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
        KeyCode::Char('a') if key.modifiers.is_empty() => {
            app.start_plan_interview_critique()?;
        }
        KeyCode::Char('f') if key.modifiers.is_empty() => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.begin_directed_feedback();
            }
            app.message = None;
        }
        KeyCode::Char('i') if key.modifiers.is_empty() => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.begin_investigation();
            }
            app.message = None;
        }
        _ => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                apply_scroll_key(key, &mut state.review_scroll_offset);
            }
        }
    }
    Ok(())
}

/// Multi-line user direction for a repository-aware plan revision. `Ctrl+S`
/// submits; ordinary Enter remains a newline so the instruction can be as
/// detailed as necessary. Esc always returns to the unchanged plan, including
/// while a paid call is in flight.
fn handle_plan_directed_feedback_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let loading = matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::DirectedFeedbackLoading
    );
    match key.code {
        KeyCode::Esc => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.cancel_directed_feedback();
            }
            app.message = if loading {
                Some("Directed revision dismissed; any late result will be ignored".into())
            } else {
                None
            };
        }
        KeyCode::Char('s') if !loading && key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.start_plan_interview_directed_feedback()?;
        }
        _ if !loading => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                let outcome = state.editor.handle_key(key);
                if outcome.text_changed {
                    state.edit_sync_to_cursor = true;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// Research-focus editor for context-isolated repository investigation. Blank
/// lines delimit separate investigator contexts; `Ctrl+S` starts the paid
/// read-only passes and their separate no-tools merge.
fn handle_plan_investigation_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let loading = matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::InvestigationLoading
    );
    match key.code {
        KeyCode::Esc => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.cancel_investigation();
            }
            app.message = if loading {
                Some("Investigation dismissed; any late result will be ignored".into())
            } else {
                None
            };
        }
        KeyCode::Char('s') if !loading && key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.start_plan_interview_investigation()?;
        }
        _ if !loading => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                let outcome = state.editor.handle_key(key);
                if outcome.text_changed {
                    state.edit_sync_to_cursor = true;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

/// The advisory agent review of the draft plan. Every action here either
/// scrolls, returns to the untouched plan, or asks for an explicit revision —
/// the review never rewrites the plan on its own.
fn handle_plan_critique_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let loading = matches!(
        &app.mode,
        AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::CritiqueLoading
    );
    match key.code {
        // Esc backs out to the plan rather than aborting the interview: the
        // plan is already generated, so losing it to a stray Esc would be a
        // far worse trade than dropping an in-flight review.
        KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                state.close_critique();
            }
            app.message = if loading {
                Some("Plan review dismissed; press a to see it if it lands".into())
            } else {
                None
            };
        }
        KeyCode::Char('r') if !loading && key.modifiers.is_empty() => {
            let revising = match &mut app.mode {
                AppMode::PlanInterview(state) => state.revise_from_critique(),
                _ => false,
            };
            if revising {
                app.message = None;
                app.start_plan_interview_synthesis()?;
            }
        }
        _ if !loading => {
            if let AppMode::PlanInterview(state) = &mut app.mode {
                apply_scroll_key(key, &mut state.critique_scroll_offset);
            }
        }
        _ => {}
    }
    Ok(())
}

/// Shared markdown scrolling for the review gate and the advisory review.
fn apply_scroll_key(key: KeyEvent, offset: &mut usize) {
    let control = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('j') | KeyCode::Down if control => {
            *offset = offset.saturating_add(REVIEW_FAST_SCROLL_STEP);
        }
        KeyCode::Char('k') | KeyCode::Up if control => {
            *offset = offset.saturating_sub(REVIEW_FAST_SCROLL_STEP);
        }
        KeyCode::Char('j') | KeyCode::Down => *offset = offset.saturating_add(1),
        KeyCode::Char('k') | KeyCode::Up => *offset = offset.saturating_sub(1),
        KeyCode::PageDown => *offset = offset.saturating_add(10),
        KeyCode::PageUp => *offset = offset.saturating_sub(10),
        KeyCode::Home | KeyCode::Char('g') => *offset = 0,
        KeyCode::End | KeyCode::Char('G') => *offset = usize::MAX,
        _ => {}
    }
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
                app.persist_plan_interview_draft();
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
