use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};

use crate::app::expert_assist::{ExpertAssistField, ExpertAssistPhase};
use crate::app::{App, AppMode};

pub fn handle_expert_assist_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let AppMode::ExpertAssist(state) = &mut app.mode else {
        return Ok(());
    };
    if state.phase != ExpertAssistPhase::Draft {
        if state.editing_handoff {
            let mut save = None;
            match key.code {
                KeyCode::Esc => state.editing_handoff = false,
                KeyCode::Backspace => {
                    state.handoff_buffer.pop();
                }
                KeyCode::Enter => {
                    let id = state.consultation_id.clone();
                    let body = state.handoff_buffer.clone();
                    if let Some(id) = id {
                        save = Some((id, body));
                    }
                }
                KeyCode::Char(c) => state.handoff_buffer.push(c),
                _ => {}
            }
            if let Some((id, body)) = save {
                let saved = app.save_expert_handoff_edit(&id, &body);
                if !saved && let AppMode::ExpertAssist(state) = &mut app.mode {
                    state.status = "Could not save the edited handoff.".into();
                }
            }
            return Ok(());
        }
        let mut send = false;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => app.cancel_expert_assist(),
            KeyCode::Char('v') if state.phase == ExpertAssistPhase::Ready => {
                state.status = state
                    .handoff_body
                    .clone()
                    .unwrap_or_else(|| "No staged handoff is available.".into());
            }
            KeyCode::Char('d') if state.phase == ExpertAssistPhase::Ready => {
                let id = state.consultation_id.clone();
                if let (Some(db), Some(id)) = (app.db.as_ref(), id)
                    && let Ok(Some(row)) = db.expert_consultation(&id)
                    && db
                        .dismiss_expert_handoff(&id, row.revision)
                        .unwrap_or(false)
                {
                    app.message = Some("Expert handoff dismissed.".into());
                    app.cancel_expert_assist();
                }
            }
            KeyCode::Char('e') if state.phase == ExpertAssistPhase::Ready => {
                state.handoff_buffer = state.handoff_body.clone().unwrap_or_default();
                state.editing_handoff = true;
                state.status = "Editing staged handoff; Enter saves, Esc cancels.".into();
            }
            KeyCode::Char('s') if state.phase == ExpertAssistPhase::Ready => {
                send = true;
            }
            _ => {}
        }
        if send
            && let Err(error) = app.send_expert_handoff()
            && let AppMode::ExpertAssist(state) = &mut app.mode
        {
            state.status = format!("Send blocked: {error:#}");
        }
        return Ok(());
    }
    let mut submit = false;
    match key.code {
        KeyCode::Esc => app.cancel_expert_assist(),
        KeyCode::Tab | KeyCode::Down => state.next_field(),
        KeyCode::BackTab | KeyCode::Up => {
            state.field = match state.field {
                ExpertAssistField::Question => ExpertAssistField::AttemptedFixes,
                ExpertAssistField::Criteria => ExpertAssistField::Question,
                ExpertAssistField::AttemptedFixes => ExpertAssistField::Criteria,
            };
        }
        KeyCode::Backspace => {
            state.active_text_mut().pop();
        }
        KeyCode::Enter => {
            if state.question.trim().is_empty() || state.acceptance_criteria.trim().is_empty() {
                state.status = "Question and acceptance criteria are required.".into();
            } else {
                state.phase = ExpertAssistPhase::Running;
                state.status = "Consultation is ready for pre-call confirmation.".into();
                submit = true;
            }
        }
        KeyCode::Char(c) => state.active_text_mut().push(c),
        _ => {}
    }
    if submit {
        app.submit_expert_assist_form();
    }
    Ok(())
}
