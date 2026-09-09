//! Key handling for `AppMode::PlanInterviewAttachDoc` — the file browser that
//! attaches a reference document to a plan interview. Modeled on
//! `handlers/browse.rs`: `Enter` descends into a directory or selects a file,
//! `Esc` backs out, everything else drives the explorer widget.

use anyhow::Result;
use crossterm::event::{Event, KeyCode, KeyEvent};

use crate::app::{App, AppMode};

pub fn handle_plan_interview_attach_doc_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.cancel_plan_interview_attach_doc(),
        KeyCode::Enter => {
            let is_dir = matches!(
                &app.mode,
                AppMode::PlanInterviewAttachDoc(state) if state.explorer.current().is_dir()
            );
            if is_dir {
                if let AppMode::PlanInterviewAttachDoc(state) = &mut app.mode {
                    state.error = None;
                    let _ = state.explorer.handle(&Event::Key(key));
                }
            } else {
                app.confirm_plan_interview_attach_doc();
            }
        }
        _ => {
            if let AppMode::PlanInterviewAttachDoc(state) = &mut app.mode {
                state.error = None;
                let _ = state.explorer.handle(&Event::Key(key));
            }
        }
    }
    Ok(())
}
