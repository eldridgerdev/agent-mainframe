use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode};

pub fn handle_change_reason_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('u') {
        if let AppMode::ChangeReasonPrompt(state) = &mut app.mode {
            state.reason.clear();
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => {
            submit_change_reason(app, false, true)?;
        }
        KeyCode::Enter => {
            submit_change_reason(app, false, false)?;
        }
        KeyCode::Char('r') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            submit_change_reason(app, true, false)?;
        }
        KeyCode::Backspace => {
            if let AppMode::ChangeReasonPrompt(state) = &mut app.mode {
                state.reason.pop();
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::ChangeReasonPrompt(state) = &mut app.mode {
                if state.reason.len() < 200 {
                    state.reason.push(c);
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn submit_change_reason(app: &mut App, reject: bool, skip: bool) -> Result<()> {
    let (
        response_file,
        proceed_signal,
        reason,
        request_id,
        reply_socket,
    ) = match &app.mode {
        AppMode::ChangeReasonPrompt(state) => (
            state.response_file.clone(),
            state.proceed_signal.clone(),
            state.reason.clone(),
            state.request_id.clone(),
            state.reply_socket.clone(),
        ),
        _ => return Ok(()),
    };

    let response = if skip {
        serde_json::json!({
            "reason": null,
            "skip": true,
            "reject": false,
        })
    } else if reject {
        serde_json::json!({
            "reason": null,
            "skip": false,
            "reject": true,
        })
    } else {
        serde_json::json!({
            "reason": reason,
            "skip": false,
            "reject": false,
        })
    };

    let mut responded_over_ipc = false;
    if let (Some(req), Some(sock)) = (request_id, reply_socket) {
        if !req.is_empty() && !sock.is_empty() {
            let payload = serde_json::json!({
                "type": "change-reason-response",
                "request_id": req,
                "response": response,
            });
            if crate::ipc::send(
                std::path::Path::new(&sock),
                &serde_json::to_string(&payload).unwrap_or_default(),
            )
            .is_ok()
            {
                responded_over_ipc = true;
            } else {
                app.log_warn(
                    "ipc",
                    "Failed IPC response for change-reason; falling back to files".to_string(),
                );
            }
        }
    }

    if !responded_over_ipc {
        if let Some(parent) = response_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(
            &response_file,
            serde_json::to_string(&response).unwrap_or_default(),
        );

        if let Some(parent) = proceed_signal.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&proceed_signal, "");
    }

    app.mode = AppMode::Normal;
    Ok(())
}
