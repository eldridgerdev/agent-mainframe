//! Supervised edits: Vibeless mode's per-edit approval. A harness hook
//! (Claude's `custom-diff-review.sh`, OpenCode's `change-tracker.js`) holds
//! each file write until AMF answers `proceed`, `reject` or `cancel`. The
//! TUI answers from `AppMode::DiffReviewPrompt`; the desktop GUI answers the
//! same file-fallback requests through `gui_supervised_edits`. Both use the
//! helpers here, so the diff a reviewer sees and the reply a hook reads are
//! built once.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use super::{App, PendingInput};

/// OpenCode keeps a request-specific lease while its pre-write hook polls.
/// Legacy session-wide paths cannot prove which write is waiting.
pub(crate) fn opencode_edit_is_waiting(input: &PendingInput) -> bool {
    #[derive(serde::Deserialize)]
    struct Waiter {
        pid: i64,
        session_id: String,
        change_id: String,
    }
    let Some(response) = input.response_file.as_deref().map(Path::new) else {
        return false;
    };
    let Some(dir) = response.parent() else {
        return false;
    };
    if response != dir.join("response.json")
        || input.proceed_signal.as_deref().map(Path::new) != Some(dir.join("proceed").as_path())
    {
        return false;
    }
    let lease = dir.join("waiter.json");
    let Ok(bytes) = std::fs::read(&lease) else {
        return false;
    };
    let Ok(waiter) = serde_json::from_slice::<Waiter>(&bytes) else {
        return false;
    };
    waiter.session_id == input.session_id
        && input.change_id.as_deref() == Some(waiter.change_id.as_str())
        && crate::resources::procs::pid_alive(waiter.pid)
        && std::fs::metadata(lease)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age < std::time::Duration::from_secs(5))
}

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

/// Held across validation and delivery. Keep the lock file in place: unlinking
/// it would let another process lock a different inode for the same request.
pub(crate) struct EditReviewClaim {
    _lock: std::fs::File,
    response_file: std::path::PathBuf,
}

impl EditReviewClaim {
    pub(crate) fn acquire(response_file: &Path) -> Result<Self> {
        let lock_path = response_file.with_extension("answer-lock");
        let lock = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .with_context(|| format!("Could not open {}", lock_path.display()))?;
        lock.try_lock().map_err(|_| {
            anyhow::anyhow!("Another reviewer is answering this edit; refresh and retry")
        })?;
        Ok(Self {
            _lock: lock,
            response_file: response_file.to_path_buf(),
        })
    }
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
        let claim = EditReviewClaim::acquire(reply.response_file)?;
        self.deliver_claimed_edit_review_response(&claim, reply, response)
    }

    pub(crate) fn deliver_claimed_edit_review_response(
        &mut self,
        claim: &EditReviewClaim,
        reply: &EditReviewReply<'_>,
        response: &serde_json::Value,
    ) -> Result<EditReviewDelivery> {
        anyhow::ensure!(
            claim.response_file == reply.response_file,
            "The edit reply path changed while acquiring ownership"
        );
        anyhow::ensure!(
            !reply.proceed_signal.exists(),
            "This edit was already answered"
        );
        // If signalling failed (or the responder crashed), only the original
        // decision can be retried. Never replace another reviewer's answer.
        let existing = match std::fs::read(reply.response_file) {
            Ok(bytes) => {
                let previous: serde_json::Value = serde_json::from_slice(&bytes)?;
                anyhow::ensure!(
                    previous == *response,
                    "This edit already has a different answer"
                );
                true
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(err).context("Could not read the existing edit answer"),
        };
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

        if !existing {
            let parent = reply.response_file.parent().unwrap_or(Path::new("."));
            let mut staged = tempfile::NamedTempFile::new_in(parent)?;
            staged.write_all(&serde_json::to_vec(response)?)?;
            staged
                .persist_noclobber(reply.response_file)
                .with_context(|| format!("Could not publish {}", reply.response_file.display()))?;
        }
        // Never recreate a hook's deleted directories after it stops waiting.
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

    fn empty_app() -> App {
        App::new_for_test(
            crate::project::ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: vec![],
                extra: std::collections::HashMap::new(),
            },
            Box::new(crate::traits::MockTmuxOps::new()),
            Box::new(crate::traits::MockWorktreeOps::new()),
        )
    }

    #[test]
    fn competing_process_cannot_replace_a_rejection() {
        const CHILD: &str = "AMF_EDIT_DELIVERY_CHILD";
        if let Ok(root) = std::env::var(CHILD) {
            let root = Path::new(&root);
            let result = empty_app().deliver_edit_review_response(
                &EditReviewReply {
                    request_id: None,
                    reply_socket: None,
                    response_file: &root.join("response.json"),
                    proceed_signal: &root.join("proceed"),
                },
                &edit_review_response(EditReviewDecision::Approve, ""),
            );
            assert!(result.is_err());
            assert!(!root.join("proceed").exists());
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let response_file = dir.path().join("response.json");
        let claim = EditReviewClaim::acquire(&response_file).unwrap();
        let response = edit_review_response(EditReviewDecision::Reject, "keep it");
        // A different process races while the rejecting reviewer owns delivery.
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "app::supervised_edits::tests::competing_process_cannot_replace_a_rejection",
            ])
            .env(CHILD, dir.path())
            .status()
            .unwrap();
        assert!(status.success());
        let proceed_signal = dir.path().join("proceed");
        let reply = EditReviewReply {
            request_id: None,
            reply_socket: None,
            response_file: &response_file,
            proceed_signal: &proceed_signal,
        };
        empty_app()
            .deliver_claimed_edit_review_response(&claim, &reply, &response)
            .unwrap();
        drop(claim);
        assert!(
            empty_app()
                .deliver_edit_review_response(
                    &reply,
                    &edit_review_response(EditReviewDecision::Approve, "")
                )
                .is_err()
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(response_file).unwrap())
                .unwrap(),
            response
        );
    }

    #[test]
    fn failed_signal_can_retry_only_the_published_decision() {
        let dir = tempfile::tempdir().unwrap();
        let response_file = dir.path().join("response.json");
        let proceed_signal = dir.path().join("missing/proceed");
        let reply = EditReviewReply {
            request_id: None,
            reply_socket: None,
            response_file: &response_file,
            proceed_signal: &proceed_signal,
        };
        let response = edit_review_response(EditReviewDecision::Reject, "no");
        let mut app = empty_app();
        assert!(app.deliver_edit_review_response(&reply, &response).is_err());
        assert!(response_file.exists());
        std::fs::create_dir(dir.path().join("missing")).unwrap();
        assert!(
            app.deliver_edit_review_response(
                &reply,
                &edit_review_response(EditReviewDecision::Approve, "")
            )
            .is_err()
        );
        assert!(!proceed_signal.exists());
        app.deliver_edit_review_response(&reply, &response).unwrap();
        assert!(proceed_signal.exists());
    }

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
        std::fs::create_dir(dir.path().join("hook")).unwrap();
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
