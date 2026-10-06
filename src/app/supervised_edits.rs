//! Supervised edits: Vibeless mode's per-edit approval. A harness hook
//! (Claude's `custom-diff-review.sh`, OpenCode's `change-tracker.js`) holds
//! each file write until AMF answers `proceed`, `reject` or `cancel`. The
//! TUI answers from `AppMode::DiffReviewPrompt`; the desktop GUI answers the
//! same file-fallback requests through `gui_supervised_edits`. Both use the
//! helpers here, so the diff a reviewer sees and the reply a hook reads are
//! built once.

use std::path::Path;

use anyhow::{Context, Result};

use super::{App, PendingInput};

/// A reviewer's answer to one pending edit, in the hook protocol's terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditReviewDecision {
    /// `proceed`: let the agent write the change.
    Approve,
    /// `reject`: block the change, optionally with feedback for the agent.
    Reject,
    /// `cancel` (the TUI's Esc): sent with `skip: true`. Claude's hook blocks
    /// the change as cancelled; OpenCode's tracker lets it through without
    /// recording a reason.
    Cancel,
}

/// Where a reply went. The hook reads it from whichever path it is waiting
/// on, so this is reported for diagnostics only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditReviewDelivery {
    Ipc,
    Files,
}

/// The flat `{type, decision, reason, skip, reject}` reply every supervised
/// edit hook understands. `reason` is omitted for a cancel and sent as `null`
/// when empty, matching what the TUI has always written.
pub(crate) fn edit_review_response(
    decision: EditReviewDecision,
    reason: &str,
) -> serde_json::Value {
    let reason_value = if reason.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::json!(reason)
    };
    match decision {
        EditReviewDecision::Cancel => serde_json::json!({
            "type": "review-response",
            "decision": "cancel",
            "reason": null,
            "skip": true,
            "reject": false,
        }),
        EditReviewDecision::Reject => serde_json::json!({
            "type": "review-response",
            "decision": "reject",
            "reason": reason_value,
            "skip": false,
            "reject": true,
        }),
        EditReviewDecision::Approve => serde_json::json!({
            "type": "review-response",
            "decision": "proceed",
            "reason": reason_value,
            "skip": false,
            "reject": false,
        }),
    }
}

/// Reply locations for one pending edit, copied out of its notification.
pub(crate) struct EditReviewReply<'a> {
    pub request_id: Option<&'a str>,
    pub reply_socket: Option<&'a str>,
    pub response_file: &'a Path,
    pub proceed_signal: &'a Path,
}

/// Load the whole-file diff a reviewer judges, from the hook's captured
/// original and proposed copies. A new file is presented as added. Without
/// both copies there is no diff and no error: the snippets are all there is.
pub(crate) fn load_edit_review_diff(
    input: &PendingInput,
) -> (Option<crate::diff::DiffFile>, Option<String>) {
    let diff_path = input
        .relative_path
        .clone()
        .filter(|path| !path.is_empty())
        .or_else(|| input.target_file_path.clone())
        .unwrap_or_default();
    let (mut diff_file, diff_error) = match (
        input.original_file.as_deref(),
        input.proposed_file.as_deref(),
    ) {
        (Some(original), Some(proposed)) => {
            match crate::diff::load_review_file(
                Path::new(original),
                Path::new(proposed),
                &diff_path,
            ) {
                Ok(file) => (Some(file), None),
                Err(err) => (None, Some(err.to_string())),
            }
        }
        _ => (None, None),
    };
    if input.is_new_file == Some(true)
        && let Some(file) = &mut diff_file
    {
        file.status = crate::diff::DiffFileStatus::Added;
        file.old_path = None;
        file.deletions = 0;
    }
    (diff_file, diff_error)
}

impl App {
    /// Deliver `response` the way the hook is waiting for it: over the reply
    /// socket when the request arrived over IPC, otherwise by writing the
    /// response file and then touching the proceed signal (the hook polls the
    /// signal and only then reads the response, so the order matters).
    pub(crate) fn deliver_edit_review_response(
        &mut self,
        reply: &EditReviewReply<'_>,
        response: &serde_json::Value,
    ) -> Result<EditReviewDelivery> {
        if let (Some(req), Some(sock)) = (reply.request_id, reply.reply_socket)
            && !req.is_empty()
            && !sock.is_empty()
        {
            let mut payload = response.clone();
            if let Some(obj) = payload.as_object_mut() {
                obj.insert("request_id".to_string(), serde_json::json!(req));
            }
            if crate::ipc::send(
                Path::new(sock),
                &serde_json::to_string(&payload).unwrap_or_default(),
            )
            .is_ok()
            {
                return Ok(EditReviewDelivery::Ipc);
            }
            self.log_warn(
                "ipc",
                "Failed IPC response for change-reason; falling back to files".to_string(),
            );
        }

        if let Some(parent) = reply.response_file.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Could not create {}", parent.display()))?;
        }
        std::fs::write(
            reply.response_file,
            serde_json::to_string(response).unwrap_or_default(),
        )
        .with_context(|| format!("Could not write {}", reply.response_file.display()))?;

        if let Some(parent) = reply.proceed_signal.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("Could not create {}", parent.display()))?;
        }
        std::fs::write(reply.proceed_signal, "")
            .with_context(|| format!("Could not write {}", reply.proceed_signal.display()))?;
        Ok(EditReviewDelivery::Files)
    }

    /// Whether a notification is a per-edit approval request AMF answers
    /// with its own diff viewer (rather than a plain attention ping).
    pub(crate) fn is_structured_edit_review(&self, notification_type: &str) -> bool {
        notification_type == "change-reason"
            || (notification_type == "diff-review" && self.use_custom_diff_review_viewer())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn responses_match_the_hook_protocol() {
        let approve = edit_review_response(EditReviewDecision::Approve, "");
        assert_eq!(approve["decision"], "proceed");
        assert_eq!(approve["reason"], serde_json::Value::Null);
        assert_eq!(approve["reject"], false);
        assert_eq!(approve["skip"], false);

        let reject = edit_review_response(EditReviewDecision::Reject, "Keep the old name");
        assert_eq!(reject["decision"], "reject");
        assert_eq!(reject["reason"], "Keep the old name");
        assert_eq!(reject["reject"], true);

        let cancel = edit_review_response(EditReviewDecision::Cancel, "ignored");
        assert_eq!(cancel["decision"], "cancel");
        assert_eq!(cancel["reason"], serde_json::Value::Null);
        assert_eq!(cancel["skip"], true);
        assert_eq!(cancel["reject"], false);
    }

    #[test]
    fn file_delivery_writes_the_response_before_the_signal() {
        let dir = tempfile::tempdir().unwrap();
        let response_file = dir.path().join("hook/response.json");
        let proceed_signal = dir.path().join("hook/proceed");
        let reply = EditReviewReply {
            request_id: None,
            reply_socket: None,
            response_file: &response_file,
            proceed_signal: &proceed_signal,
        };
        let response = edit_review_response(EditReviewDecision::Reject, "no");
        let mut app = App::new_for_test(
            crate::project::ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: std::collections::HashMap::new(),
            },
            Box::new(crate::traits::MockTmuxOps::new()),
            Box::new(crate::traits::MockWorktreeOps::new()),
        );
        assert_eq!(
            app.deliver_edit_review_response(&reply, &response).unwrap(),
            EditReviewDelivery::Files
        );
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&response_file).unwrap()).unwrap();
        assert_eq!(written, response);
        assert!(proceed_signal.exists());
    }

    #[test]
    fn a_failed_response_write_leaves_the_hook_waiting() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the response's directory should be makes the write fail.
        std::fs::write(dir.path().join("blocked"), "").unwrap();
        let response_file = dir.path().join("blocked/response.json");
        let proceed_signal = dir.path().join("proceed");
        let mut app = App::new_for_test(
            crate::project::ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: std::collections::HashMap::new(),
            },
            Box::new(crate::traits::MockTmuxOps::new()),
            Box::new(crate::traits::MockWorktreeOps::new()),
        );
        let result = app.deliver_edit_review_response(
            &EditReviewReply {
                request_id: None,
                reply_socket: None,
                response_file: &response_file,
                proceed_signal: &proceed_signal,
            },
            &edit_review_response(EditReviewDecision::Reject, "no"),
        );
        assert!(result.is_err());
        // Claude's hook reads a missing reply as approval, so the signal must
        // not appear without one.
        assert!(!proceed_signal.exists());
    }
}
