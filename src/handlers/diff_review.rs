use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::fs;

use crate::app::{App, AppMode};
use crate::claude::ClaudeLauncher;

fn diff_review_uses_new_file_presentation(app: &App) -> bool {
    matches!(
        &app.mode,
        AppMode::DiffReviewPrompt(state)
            if state.diff_file.as_ref().is_some_and(|file| {
                matches!(
                    file.status,
                    crate::diff::DiffFileStatus::Added | crate::diff::DiffFileStatus::Untracked
                )
            })
    )
}

pub fn handle_diff_review_key(app: &mut App, key: KeyEvent) -> Result<()> {
    if matches!(
        &app.mode,
        AppMode::DiffReviewPrompt(state) if state.explanation_child.is_some()
    ) {
        return Ok(());
    }

    let editing_feedback = matches!(
        &app.mode,
        AppMode::DiffReviewPrompt(state) if state.editing_feedback
    );

    if editing_feedback {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('u') {
            if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
                state.reason.clear();
            }
            return Ok(());
        }

        match key.code {
            KeyCode::Esc => {
                if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
                    state.editing_feedback = false;
                    state.reason.clear();
                }
            }
            KeyCode::Enter => {
                submit_diff_review(app, true, false)?;
            }
            KeyCode::Backspace => {
                if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
                    state.reason.pop();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let AppMode::DiffReviewPrompt(state) = &mut app.mode
                    && state.reason.len() < 200
                {
                    state.reason.push(c);
                }
            }
            _ => {}
        }
        return Ok(());
    }

    match key.code {
        KeyCode::Esc => {
            submit_diff_review(app, false, true)?;
        }
        KeyCode::Enter => {
            submit_diff_review(app, false, false)?;
        }
        KeyCode::Char('e') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            generate_diff_review_explanation(app);
        }
        KeyCode::Char('v') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if diff_review_uses_new_file_presentation(app) {
                return Ok(());
            }
            let next_layout = match &app.mode {
                AppMode::DiffReviewPrompt(state) => Some(app.toggled_diff_viewer_layout(&state.layout)),
                _ => None,
            };
            if let Some(layout) = next_layout {
                app.persist_diff_viewer_layout(layout.clone());
                if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
                    state.layout = layout;
                }
            }
        }
        KeyCode::Char('q') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            submit_diff_review(app, false, true)?;
        }
        KeyCode::Char('r') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
                state.editing_feedback = true;
                state.reason.clear();
            }
        }
        _ => {}
    }
    Ok(())
}

fn submit_diff_review(app: &mut App, reject: bool, skip: bool) -> Result<()> {
    let (
        response_file,
        proceed_signal,
        reason,
        request_id,
        reply_socket,
        return_to_view,
    ) = match &app.mode {
        AppMode::DiffReviewPrompt(state) => (
            state.response_file.clone(),
            state.proceed_signal.clone(),
            state.reason.clone(),
            state.request_id.clone(),
            state.reply_socket.clone(),
            state.return_to_view.clone(),
        ),
        _ => return Ok(()),
    };

    let response = if skip {
        serde_json::json!({
            "type": "review-response",
            "decision": "cancel",
            "reason": null,
            "skip": true,
            "reject": false,
        })
    } else if reject {
        serde_json::json!({
            "type": "review-response",
            "decision": "reject",
            "reason": if reason.is_empty() { serde_json::Value::Null } else { serde_json::json!(reason) },
            "skip": false,
            "reject": true,
        })
    } else {
        serde_json::json!({
            "type": "review-response",
            "decision": "proceed",
            "reason": if reason.is_empty() { serde_json::Value::Null } else { serde_json::json!(reason) },
            "skip": false,
            "reject": false,
        })
    };

    let mut responded_over_ipc = false;
    if let (Some(req), Some(sock)) = (request_id, reply_socket) {
        if !req.is_empty() && !sock.is_empty() {
            let mut payload = response.clone();
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("request_id".to_string(), serde_json::json!(req));
            }
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

    app.mode = match return_to_view {
        Some(view) => AppMode::Viewing(view),
        None => AppMode::Normal,
    };
    Ok(())
}

fn generate_diff_review_explanation(app: &mut App) {
    let (workdir, relative_path, old_snippet, new_snippet) = match &app.mode {
        AppMode::DiffReviewPrompt(state) => (
            state.workdir.clone(),
            state.relative_path.clone(),
            state.old_snippet.clone(),
            state.new_snippet.clone(),
        ),
        _ => return,
    };

    if let Some(explanation) = find_review_note(&workdir, &relative_path) {
        if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
            state.explanation = Some(explanation.trim().to_string());
        }
        return;
    }

    let prompt = format!(
        "Explain these code changes concisely. What is being changed and why?\n\nFile: {relative_path}\n\nOld:\n```\n{old_snippet}\n```\n\nNew:\n```\n{new_snippet}\n```"
    );

    match ClaudeLauncher::spawn_headless(&workdir, &prompt) {
        Ok(child) => {
            if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
                state.explanation = None;
                state.explanation_child = Some(child);
            }
        }
        Err(err) => {
            if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
                state.explanation = Some(format!("Explanation unavailable: {err}"));
            }
        }
    }
}

fn find_review_note(workdir: &std::path::Path, relative_path: &str) -> Option<String> {
    let notes = fs::read_to_string(workdir.join(".claude").join("review-notes.md")).ok()?;
    let heading = format!("## {relative_path}");
    let mut in_block = false;
    let mut block = String::new();

    for line in notes.lines() {
        if line.starts_with("## ") {
            if in_block {
                break;
            }
            in_block = line == heading;
            continue;
        }
        if in_block && line == "---" {
            break;
        }
        if in_block {
            block.push_str(line);
            block.push('\n');
        }
    }

    let trimmed = block.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::DiffReviewState;
    use crate::diff::{DiffFile, DiffFileStatus};
    use crate::project::ProjectStore;
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn make_app_with_prompt(workdir: &std::path::Path) -> App {
        let mut app = App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.mode = AppMode::DiffReviewPrompt(DiffReviewState {
            session_id: "sess-1".to_string(),
            workdir: workdir.to_path_buf(),
            file_path: workdir.join("src/main.rs").display().to_string(),
            relative_path: "src/main.rs".to_string(),
            change_id: "chg-1".to_string(),
            tool: "edit".to_string(),
            old_snippet: "old".to_string(),
            new_snippet: "new".to_string(),
            diff_file: None,
            diff_error: None,
            reason: String::new(),
            editing_feedback: false,
            layout: crate::app::DiffViewerLayout::Unified,
            explanation: None,
            explanation_child: None,
            response_file: workdir.join("response.json"),
            proceed_signal: workdir.join("proceed"),
            request_id: None,
            reply_socket: None,
            return_to_view: None,
        });
        app
    }

    fn make_new_file_diff(path: &str) -> DiffFile {
        DiffFile {
            old_path: None,
            path: path.to_string(),
            status: DiffFileStatus::Added,
            additions: 2,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: Some("hello\nworld\n".to_string()),
            patch: String::new(),
            hunks: vec![],
        }
    }

    #[test]
    fn enter_writes_proceed_review_response() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app_with_prompt(tmp.path());

        handle_diff_review_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .unwrap();

        let response: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(tmp.path().join("response.json")).unwrap())
                .unwrap();
        assert_eq!(response["decision"], "proceed");
        assert_eq!(response["type"], "review-response");
    }

    #[test]
    fn reject_writes_review_response_with_feedback() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app_with_prompt(tmp.path());
        if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
            state.editing_feedback = true;
            state.reason = "try a smaller change".to_string();
        }

        handle_diff_review_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).unwrap();

        let response: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(tmp.path().join("response.json")).unwrap())
                .unwrap();
        assert_eq!(response["decision"], "reject");
        assert_eq!(response["reason"], "try a smaller change");
    }

    #[test]
    fn reject_shortcut_enters_feedback_mode() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app_with_prompt(tmp.path());

        handle_diff_review_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE),
        )
        .unwrap();

        match &app.mode {
            AppMode::DiffReviewPrompt(state) => {
                assert!(state.editing_feedback);
                assert!(state.reason.is_empty());
            }
            _ => panic!("expected diff review prompt"),
        }
        assert!(!tmp.path().join("response.json").exists());
    }

    #[test]
    fn feedback_mode_treats_e_as_text_not_explain() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app_with_prompt(tmp.path());
        if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
            state.editing_feedback = true;
        }

        handle_diff_review_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('e'), KeyModifiers::NONE),
        )
        .unwrap();

        match &app.mode {
            AppMode::DiffReviewPrompt(state) => {
                assert_eq!(state.reason, "e");
                assert!(state.explanation.is_none());
            }
            _ => panic!("expected diff review prompt"),
        }
    }

    #[test]
    fn feedback_mode_escape_closes_editor_without_submitting() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app_with_prompt(tmp.path());
        if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
            state.editing_feedback = true;
            state.reason = "draft".to_string();
        }

        handle_diff_review_key(&mut app, KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)).unwrap();

        match &app.mode {
            AppMode::DiffReviewPrompt(state) => {
                assert!(!state.editing_feedback);
                assert!(state.reason.is_empty());
            }
            _ => panic!("expected diff review prompt"),
        }
        assert!(!tmp.path().join("response.json").exists());
    }

    #[test]
    fn v_toggles_side_by_side_mode() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app_with_prompt(tmp.path());
        app.config.diff_viewer_layout = crate::app::DiffViewerLayout::Unified;

        handle_diff_review_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
        )
        .unwrap();

        match &app.mode {
            AppMode::DiffReviewPrompt(state) => {
                assert_eq!(state.layout, crate::app::DiffViewerLayout::SideBySide);
                assert_eq!(
                    app.config.diff_viewer_layout,
                    crate::app::DiffViewerLayout::SideBySide
                );
            }
            _ => panic!("expected diff review prompt"),
        }

        handle_diff_review_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
        )
        .unwrap();

        match &app.mode {
            AppMode::DiffReviewPrompt(state) => {
                assert_eq!(state.layout, crate::app::DiffViewerLayout::Unified);
                assert_eq!(
                    app.config.diff_viewer_layout,
                    crate::app::DiffViewerLayout::Unified
                );
            }
            _ => panic!("expected diff review prompt"),
        }
    }

    #[test]
    fn v_does_not_toggle_layout_for_new_file_review() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app_with_prompt(tmp.path());
        app.config.diff_viewer_layout = crate::app::DiffViewerLayout::SideBySide;
        if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
            state.layout = crate::app::DiffViewerLayout::SideBySide;
            state.diff_file = Some(make_new_file_diff("src/new.rs"));
        }

        handle_diff_review_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('v'), KeyModifiers::NONE),
        )
        .unwrap();

        match &app.mode {
            AppMode::DiffReviewPrompt(state) => {
                assert_eq!(state.layout, crate::app::DiffViewerLayout::SideBySide);
                assert_eq!(
                    app.config.diff_viewer_layout,
                    crate::app::DiffViewerLayout::SideBySide
                );
            }
            _ => panic!("expected diff review prompt"),
        }
    }

    #[test]
    fn submit_from_view_returns_to_view_mode() {
        let tmp = TempDir::new().unwrap();
        let mut app = make_app_with_prompt(tmp.path());
        if let AppMode::DiffReviewPrompt(state) = &mut app.mode {
            state.return_to_view = Some(crate::app::ViewState::new(
                "my-project".to_string(),
                "my-feature".to_string(),
                "amf-my-feature".to_string(),
                "claude".to_string(),
                "Claude".to_string(),
                crate::project::VibeMode::Vibeless,
                false,
            ));
        }

        handle_diff_review_key(
            &mut app,
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        )
        .unwrap();

        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }
}
