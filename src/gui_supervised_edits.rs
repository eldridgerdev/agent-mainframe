//! Supervised edits in the desktop GUI: Vibeless mode's per-edit approval.
//!
//! A Vibeless agent's hook holds each file write until AMF answers it (see
//! `app::supervised_edits`). The TUI receives those requests over its IPC
//! socket, or reads the hook's file fallback. The GUI owns no IPC socket --
//! binding it would take requests away from a running TUI -- so it answers
//! the file-fallback requests only: the ones a hook writes when no TUI is
//! listening, which is exactly when nothing else can answer them.
//!
//! Reading uses the TUI's own notification reader and diff loader; answering
//! uses its response builder and delivery. Every answer names the edit by a
//! stable id and the revision the reviewer saw, and is refused when the edit
//! has changed, been answered, or the agent has stopped waiting.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::supervised_edits::{
    EditReviewDecision, EditReviewReply, edit_review_response, load_edit_review_diff,
};
use crate::app::{App, PendingInput};
use crate::gui_contract::{FeatureTarget, GuiError, GuiHandle, GuiResult};
use crate::gui_diff::{DiffContext, DiffFileView};

/// The TUI's feedback field accepts 200 characters; the GUI keeps the same
/// limit so a reply reads the same whichever interface sent it.
pub const MAX_FEEDBACK_CHARS: usize = 200;

/// Snippets are shown only when the hook captured no whole-file copies.
const MAX_SNIPPET_CHARS: usize = 20_000;

#[derive(Debug, Clone, Serialize)]
pub struct DecisionEffects {
    pub approve: String,
    pub reject: String,
    pub cancel: String,
    /// Whether rejection feedback reaches the agent. OpenCode's tracker
    /// blocks the write but does not forward the text.
    pub feedback_reaches_agent: bool,
}

#[derive(Debug, Serialize)]
pub struct SupervisedEditView {
    /// Stable for one request: the notification file, the hook's change id
    /// and the agent session.
    pub id: String,
    /// Changes whenever the request or its captured file copies change.
    pub revision: String,
    /// The hook protocol: `diff-review` (Claude) or `change-reason`
    /// (OpenCode).
    pub kind: String,
    pub path: String,
    pub tool: String,
    pub is_new_file: bool,
    /// The agent's own explanation, when its hook supplies one.
    pub agent_reason: Option<String>,
    pub diff: Option<DiffFileView>,
    pub diff_error: Option<String>,
    pub old_snippet: Option<String>,
    pub new_snippet: Option<String>,
    /// Seconds since the Unix epoch when the request was written.
    pub requested_at: Option<u64>,
    /// An answer has been delivered and the hook has not consumed it yet.
    pub answered: bool,
    /// Why this edit can't be answered from here, if it can't.
    pub unavailable: Option<String>,
    pub effects: DecisionEffects,
}

#[derive(Debug, Serialize)]
pub struct SupervisedEditsView {
    pub target: FeatureTarget,
    pub feature_name: String,
    pub edits: Vec<SupervisedEditView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PendingEditCount {
    pub project_id: String,
    pub feature_id: String,
    pub feature_name: String,
    /// Edits still waiting for an answer.
    pub count: usize,
    /// The oldest waiting edit's id and path, for a notification.
    pub first_id: String,
    pub first_path: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SupervisedEditDecision {
    Approve,
    Reject {
        #[serde(default)]
        feedback: String,
    },
    Cancel,
}

#[derive(Debug, Serialize)]
pub struct SupervisedEditOutcome {
    pub message: String,
    pub view: SupervisedEditsView,
}

struct PendingEdit {
    input: PendingInput,
    view: SupervisedEditView,
}

fn hex(digest: impl AsRef<[u8]>) -> String {
    digest
        .as_ref()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn edit_id(input: &PendingInput) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.file_path.to_string_lossy().as_bytes());
    hasher.update([0]);
    hasher.update(input.change_id.as_deref().unwrap_or_default().as_bytes());
    hasher.update([0]);
    hasher.update(input.session_id.as_bytes());
    hex(hasher.finalize())
}

/// Hashes the request and both captured copies, so an answer can be refused
/// when what the reviewer saw is no longer what the agent will write.
fn edit_revision(input: &PendingInput) -> String {
    let mut hasher = Sha256::new();
    for path in [
        Some(input.file_path.to_string_lossy().into_owned()),
        input.original_file.clone(),
        input.proposed_file.clone(),
    ] {
        match path.and_then(|path| std::fs::read(path).ok()) {
            Some(bytes) => {
                hasher.update((bytes.len() as u64).to_le_bytes());
                hasher.update(&bytes);
            }
            None => hasher.update(u64::MAX.to_le_bytes()),
        }
    }
    hex(hasher.finalize())
}

fn effects(kind: &str) -> DecisionEffects {
    if kind == "change-reason" {
        DecisionEffects {
            approve: "The agent writes this change and records its stated reason.".into(),
            reject: "The agent does not write this change. OpenCode is told the change was \
                     rejected; your feedback is not forwarded."
                .into(),
            cancel: "OpenCode treats cancel as a skip: the agent writes this change without \
                     recording a reason."
                .into(),
            feedback_reaches_agent: false,
        }
    } else {
        DecisionEffects {
            approve: "The agent writes this change.".into(),
            reject: "The agent does not write this change and receives your feedback.".into(),
            cancel: "The agent does not write this change and is told you cancelled it.".into(),
            feedback_reaches_agent: true,
        }
    }
}

fn non_empty(path: Option<&str>) -> Option<&Path> {
    path.filter(|path| !path.is_empty()).map(Path::new)
}

fn truncated(text: Option<String>) -> Option<String> {
    text.filter(|text| !text.is_empty()).map(|text| {
        if text.chars().count() > MAX_SNIPPET_CHARS {
            let mut short: String = text.chars().take(MAX_SNIPPET_CHARS).collect();
            short.push_str("\n…");
            short
        } else {
            text
        }
    })
}

fn context_lines(context: &DiffContext) -> usize {
    match context {
        DiffContext::Standard => 3,
        DiffContext::Expanded => 10,
        DiffContext::Full => usize::MAX,
    }
}

fn project_view(input: &PendingInput, context: usize) -> SupervisedEditView {
    let (diff, diff_error) = load_edit_review_diff(input);
    let proceed_signal = non_empty(input.proceed_signal.as_deref());
    let answered = proceed_signal.is_some_and(Path::exists);
    let unavailable = match (proceed_signal, non_empty(input.response_file.as_deref())) {
        (None, _) | (_, None) => {
            Some("This request has no reply path; answer it from the AMF TUI.".to_string())
        }
        (Some(signal), Some(_))
            if !answered && signal.parent().is_some_and(|dir| !dir.exists()) =>
        {
            Some("The agent is no longer waiting for this edit.".to_string())
        }
        _ => None,
    };
    let requested_at = std::fs::metadata(&input.file_path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs());
    let path = input
        .relative_path
        .clone()
        .filter(|path| !path.is_empty())
        .or_else(|| input.target_file_path.clone())
        .unwrap_or_else(|| "(unknown file)".to_string());
    SupervisedEditView {
        id: edit_id(input),
        revision: edit_revision(input),
        kind: input.notification_type.clone(),
        path,
        tool: input.tool.clone().unwrap_or_default(),
        is_new_file: input.is_new_file == Some(true),
        agent_reason: input.reason.clone().filter(|reason| !reason.is_empty()),
        old_snippet: diff
            .is_none()
            .then(|| truncated(input.old_snippet.clone()))
            .flatten(),
        new_snippet: diff
            .is_none()
            .then(|| truncated(input.new_snippet.clone()))
            .flatten(),
        diff: diff.map(|file| crate::gui_diff::file_view(file, context)),
        diff_error,
        requested_at,
        answered,
        unavailable,
        effects: effects(&input.notification_type),
    }
}

/// The feature's file-backed supervised-edit requests, oldest first.
fn pending_for_feature(
    app: &App,
    project_name: &str,
    feature_name: &str,
    context: usize,
) -> Vec<PendingEdit> {
    let mut edits: Vec<PendingEdit> = app
        .read_notification_files()
        .into_iter()
        .filter(|input| {
            app.is_structured_edit_review(&input.notification_type)
                && input.project_name.as_deref() == Some(project_name)
                && input.feature_name.as_deref() == Some(feature_name)
        })
        .map(|input| PendingEdit {
            view: project_view(&input, context),
            input,
        })
        .collect();
    edits.sort_by(|a, b| {
        a.view
            .requested_at
            .cmp(&b.view.requested_at)
            .then_with(|| a.view.path.cmp(&b.view.path))
    });
    edits
}

fn resolve(gui: &mut GuiHandle, target: &FeatureTarget) -> GuiResult<(String, String)> {
    gui.refresh_snapshot()?;
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("Feature was deleted; refresh and retry"))?;
    let project = &app.store.projects[pi];
    Ok((project.name.clone(), project.features[fi].name.clone()))
}

pub fn load(
    gui: &mut GuiHandle,
    target: FeatureTarget,
    context: DiffContext,
) -> GuiResult<SupervisedEditsView> {
    let (project_name, feature_name) = resolve(gui, &target)?;
    let edits = pending_for_feature(
        gui.app_for_workflow(),
        &project_name,
        &feature_name,
        context_lines(&context),
    )
    .into_iter()
    .map(|edit| edit.view)
    .collect();
    Ok(SupervisedEditsView {
        target,
        feature_name,
        edits,
    })
}

/// Waiting edits per feature, for navigation badges and arrival notices.
/// Reads notification files only; diffs are not loaded.
pub fn pending_counts(gui: &mut GuiHandle) -> GuiResult<Vec<PendingEditCount>> {
    gui.refresh_snapshot()?;
    let app = gui.app_for_workflow();
    let mut counts: Vec<PendingEditCount> = Vec::new();
    let mut waiting: Vec<(Option<u64>, PendingInput)> = app
        .read_notification_files()
        .into_iter()
        .filter(|input| app.is_structured_edit_review(&input.notification_type))
        .filter(|input| {
            non_empty(input.proceed_signal.as_deref()).is_some_and(|signal| !signal.exists())
        })
        .map(|input| {
            let at = std::fs::metadata(&input.file_path)
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs());
            (at, input)
        })
        .collect();
    waiting.sort_by_key(|(at, _)| *at);
    for (_, input) in waiting {
        let Some(project) = app
            .store
            .projects
            .iter()
            .find(|project| Some(project.name.as_str()) == input.project_name.as_deref())
        else {
            continue;
        };
        let Some(feature) = project
            .features
            .iter()
            .find(|feature| Some(feature.name.as_str()) == input.feature_name.as_deref())
        else {
            continue;
        };
        if let Some(existing) = counts.iter_mut().find(|c| c.feature_id == feature.id) {
            existing.count += 1;
            continue;
        }
        counts.push(PendingEditCount {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
            feature_name: feature.name.clone(),
            count: 1,
            first_id: edit_id(&input),
            first_path: input
                .relative_path
                .clone()
                .filter(|path| !path.is_empty())
                .or_else(|| input.target_file_path.clone())
                .unwrap_or_default(),
        });
    }
    Ok(counts)
}

/// Answer one pending edit. The caller must have shown the reviewer the
/// diff at `revision` and had the answer explicitly confirmed.
pub fn respond(
    gui: &mut GuiHandle,
    target: FeatureTarget,
    edit_id: &str,
    revision: &str,
    decision: SupervisedEditDecision,
) -> GuiResult<SupervisedEditOutcome> {
    let feedback = match &decision {
        SupervisedEditDecision::Reject { feedback } => feedback.trim().to_string(),
        _ => String::new(),
    };
    if feedback.chars().count() > MAX_FEEDBACK_CHARS {
        return Err(GuiError::from(anyhow::anyhow!(
            "Feedback is limited to {MAX_FEEDBACK_CHARS} characters"
        )));
    }
    let (project_name, feature_name) = resolve(gui, &target)?;
    let app = gui.app_for_workflow();
    let edit = pending_for_feature(app, &project_name, &feature_name, 3)
        .into_iter()
        .find(|edit| edit.view.id == edit_id)
        .ok_or_else(|| {
            GuiError::conflict(
                "This edit is no longer waiting for review: the agent continued, it was \
                 answered elsewhere, or the agent stopped",
            )
        })?;
    if edit.view.answered {
        return Err(GuiError::conflict(
            "This edit was already answered; the agent has not picked the answer up yet",
        ));
    }
    if edit.view.revision != revision {
        return Err(GuiError::conflict(
            "This edit changed after you reviewed it; review the refreshed diff before answering",
        ));
    }
    if let Some(reason) = &edit.view.unavailable {
        return Err(GuiError::conflict(reason.clone()));
    }

    let (decision, reason, verb) = match decision {
        // Approving passes the agent's own reason back, as the TUI does, so a
        // tracker that records it keeps it.
        SupervisedEditDecision::Approve => (
            EditReviewDecision::Approve,
            edit.input.reason.clone().unwrap_or_default(),
            "Approved",
        ),
        SupervisedEditDecision::Reject { .. } => (EditReviewDecision::Reject, feedback, "Rejected"),
        SupervisedEditDecision::Cancel => (EditReviewDecision::Cancel, String::new(), "Cancelled"),
    };
    let response_file = PathBuf::from(edit.input.response_file.as_deref().unwrap_or_default());
    let proceed_signal = PathBuf::from(edit.input.proceed_signal.as_deref().unwrap_or_default());
    app.deliver_edit_review_response(
        &EditReviewReply {
            request_id: edit.input.request_id.as_deref(),
            reply_socket: edit.input.reply_socket.as_deref(),
            response_file: &response_file,
            proceed_signal: &proceed_signal,
        },
        &edit_review_response(decision, &reason),
    )
    .map_err(GuiError::from)?;
    app.log_info(
        "diff-review",
        format!("GUI {} edit to {}", verb.to_lowercase(), edit.view.path),
    );
    let view = load(gui, target, DiffContext::Standard)?;
    Ok(SupervisedEditOutcome {
        message: format!("{verb} the edit to {}", edit.view.path),
        view,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui_contract::GuiErrorKind;
    use crate::gui_diff::tests::fixture;

    struct Hook {
        dir: PathBuf,
        notification: PathBuf,
        response: PathBuf,
        proceed: PathBuf,
    }

    /// Writes what `custom-diff-review.sh` writes when no AMF socket answers:
    /// captured copies in its temp dir and a notification in the feature's
    /// `.claude/notifications/`.
    fn claude_hook(repo: &Path, scratch: &Path, change: &str, proposed: &str) -> Hook {
        let dir = scratch.join(format!("hook-{change}"));
        std::fs::create_dir_all(&dir).unwrap();
        let original = std::fs::read_to_string(repo.join("code.txt")).unwrap();
        std::fs::write(dir.join("original.txt"), &original).unwrap();
        std::fs::write(dir.join("proposed.txt"), proposed).unwrap();
        let notify_dir = repo.join(".claude/notifications");
        std::fs::create_dir_all(&notify_dir).unwrap();
        let notification = notify_dir.join(format!("sess-diff-{change}.json"));
        let hook = Hook {
            notification: notification.clone(),
            response: dir.join("response.json"),
            proceed: dir.join("proceed"),
            dir,
        };
        std::fs::write(
            &notification,
            serde_json::json!({
                "type": "diff-review",
                "session_id": "sess",
                "cwd": repo,
                "file_path": repo.join("code.txt"),
                "relative_path": "code.txt",
                "tool": "edit",
                "change_id": change,
                "original_file": hook.dir.join("original.txt"),
                "proposed_file": hook.dir.join("proposed.txt"),
                "response_file": hook.response,
                "proceed_signal": hook.proceed,
                "old_snippet": "line 3",
                "new_snippet": "line three",
                "is_new_file": false,
            })
            .to_string(),
        )
        .unwrap();
        hook
    }

    fn proposed(repo: &Path) -> String {
        std::fs::read_to_string(repo.join("code.txt"))
            .unwrap()
            .replace("line 3\n", "line three\n")
    }

    #[test]
    fn lists_file_backed_edits_with_the_shared_diff_and_answers_once() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let hook = claude_hook(&repo, dir.path(), "101", &proposed(&repo));
        let source_before = std::fs::read_to_string(repo.join("code.txt")).unwrap();

        let view = load(&mut gui, target.clone(), DiffContext::Standard).unwrap();
        assert_eq!(view.edits.len(), 1);
        let edit = &view.edits[0];
        assert_eq!(edit.path, "code.txt");
        assert_eq!(edit.kind, "diff-review");
        assert!(!edit.answered);
        assert!(edit.unavailable.is_none());
        assert!(edit.effects.feedback_reaches_agent);
        let lines = &edit.diff.as_ref().unwrap().hunks[0].lines;
        assert!(
            lines
                .iter()
                .any(|l| l.kind == "removed" && l.text == "-line 3")
        );
        assert!(
            lines
                .iter()
                .any(|l| l.kind == "added" && l.text == "+line three")
        );
        let counts = pending_counts(&mut gui).unwrap();
        assert_eq!(counts.len(), 1);
        assert_eq!(counts[0].count, 1);
        assert_eq!(counts[0].first_id, edit.id);

        let outcome = respond(
            &mut gui,
            target.clone(),
            &edit.id,
            &edit.revision,
            SupervisedEditDecision::Reject {
                feedback: "  Keep the numeral  ".into(),
            },
        )
        .unwrap();
        assert_eq!(outcome.message, "Rejected the edit to code.txt");
        let reply: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hook.response).unwrap()).unwrap();
        assert_eq!(reply["decision"], "reject");
        assert_eq!(reply["reason"], "Keep the numeral");
        assert!(hook.proceed.exists());
        assert!(outcome.view.edits[0].answered);
        assert!(pending_counts(&mut gui).unwrap().is_empty());
        // Answering never touches source; the agent's hook decides that.
        assert_eq!(
            std::fs::read_to_string(repo.join("code.txt")).unwrap(),
            source_before
        );

        // A second answer before the hook consumes the first is refused.
        let err = respond(
            &mut gui,
            target.clone(),
            &edit.id,
            &edit.revision,
            SupervisedEditDecision::Approve,
        )
        .unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::Conflict);
        assert!(err.message.contains("already answered"), "{}", err.message);
        let reply: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hook.response).unwrap()).unwrap();
        assert_eq!(reply["decision"], "reject");

        // The hook consumes the answer and removes its files.
        std::fs::remove_file(&hook.notification).unwrap();
        std::fs::remove_dir_all(&hook.dir).unwrap();
        let err = respond(
            &mut gui,
            target.clone(),
            &edit.id,
            &edit.revision,
            SupervisedEditDecision::Approve,
        )
        .unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::Conflict);
        assert!(err.message.contains("no longer waiting"), "{}", err.message);
        assert!(
            !hook.dir.exists(),
            "a stale answer must not recreate hook files"
        );
    }

    #[test]
    fn refuses_changed_edits_departed_agents_and_overlong_feedback() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let hook = claude_hook(&repo, dir.path(), "202", &proposed(&repo));
        let edit = load(&mut gui, target.clone(), DiffContext::Standard)
            .unwrap()
            .edits
            .remove(0);

        std::fs::write(hook.dir.join("proposed.txt"), "something else\n").unwrap();
        let err = respond(
            &mut gui,
            target.clone(),
            &edit.id,
            &edit.revision,
            SupervisedEditDecision::Approve,
        )
        .unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::Conflict);
        assert!(
            err.message.contains("changed after you reviewed"),
            "{}",
            err.message
        );
        assert!(!hook.proceed.exists());

        let fresh = load(&mut gui, target.clone(), DiffContext::Standard)
            .unwrap()
            .edits
            .remove(0);
        assert_eq!(fresh.id, edit.id);
        assert_ne!(fresh.revision, edit.revision);
        let err = respond(
            &mut gui,
            target.clone(),
            &fresh.id,
            &fresh.revision,
            SupervisedEditDecision::Reject {
                feedback: "x".repeat(MAX_FEEDBACK_CHARS + 1),
            },
        )
        .unwrap_err();
        assert!(err.message.contains("limited to 200"), "{}", err.message);
        assert!(!hook.proceed.exists());

        // The hook was killed: its temp dir is gone but the notification
        // stayed behind. Answering would only recreate an unread file.
        std::fs::remove_dir_all(&hook.dir).unwrap();
        let gone = load(&mut gui, target.clone(), DiffContext::Standard)
            .unwrap()
            .edits
            .remove(0);
        assert_eq!(
            gone.unavailable.as_deref(),
            Some("The agent is no longer waiting for this edit.")
        );
        let err = respond(
            &mut gui,
            target,
            &gone.id,
            &gone.revision,
            SupervisedEditDecision::Cancel,
        )
        .unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::Conflict);
        assert!(!hook.dir.exists());
    }

    #[test]
    fn approve_returns_the_agent_reason_and_deleted_features_are_not_found() {
        let (dir, mut gui, target) = fixture();
        let repo = dir.path().join("repo");
        let hook = claude_hook(&repo, dir.path(), "303", &proposed(&repo));
        // OpenCode's tracker uses `change-reason` and states its reason.
        let mut notification: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hook.notification).unwrap()).unwrap();
        notification["type"] = "change-reason".into();
        notification["reason"] = "Spell out the number".into();
        std::fs::write(&hook.notification, notification.to_string()).unwrap();

        let edit = load(&mut gui, target.clone(), DiffContext::Standard)
            .unwrap()
            .edits
            .remove(0);
        assert_eq!(edit.agent_reason.as_deref(), Some("Spell out the number"));
        assert!(!edit.effects.feedback_reaches_agent);
        respond(
            &mut gui,
            target.clone(),
            &edit.id,
            &edit.revision,
            SupervisedEditDecision::Approve,
        )
        .unwrap();
        let reply: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&hook.response).unwrap()).unwrap();
        assert_eq!(reply["decision"], "proceed");
        assert_eq!(reply["reason"], "Spell out the number");

        let missing = FeatureTarget {
            project_id: target.project_id.clone(),
            feature_id: "gone".into(),
        };
        let err = load(&mut gui, missing, DiffContext::Standard).unwrap_err();
        assert_eq!(err.kind, GuiErrorKind::NotFound);
    }
}
