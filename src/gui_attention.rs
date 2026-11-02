//! The desktop GUI's needs-attention list (the TUI's `i` overlay).
//!
//! # What this process can and cannot know
//!
//! The TUI's overlay rows come from two layers (`app/attention.rs`):
//!
//! * the **attention layer** (`Question` / `Completed` / `Waiting`), which is
//!   built from `type: "attention"` hook events delivered over the TUI's IPC
//!   socket, plus the TUI's own observation of a thinking marker dropping.
//!   Nothing persists it: no file, no `amf.db` row, no `session_status`
//!   column. A GUI process with no TUI therefore **cannot** know whether a
//!   stopped agent asked a question or finished, and this module never
//!   guesses;
//! * **pending inputs**, which are the notification files hooks leave on disk
//!   (`.claude/notifications/` in each worktree, and the global notifications
//!   directory). Those are persisted, and the GUI reads them with the same
//!   reader the TUI scan uses (`App::read_notification_files`).
//!
//! The list here is therefore the second layer only: each row names the
//! on-disk request kind (waiting for input, turn ended, edit review, ...), its
//! message, how old the file is, and where to jump. The first layer is stated
//! as a limit in [`NeedsAttentionView::limits`] instead of being faked.
//!
//! # Writes
//!
//! Reading removes or answers nothing and starts no agent. The one write is
//! [`dismiss`], the TUI's `x`: it deletes the notification file. It is offered
//! only for session waits (`input-request` / `stop`) because an edit review
//! or change reason has a hook blocked on an answer, which belongs to the
//! Supervised edits panel; the TUI's `x` removes those files too, but the GUI
//! refuses and says why.

use std::path::Path;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::app::{App, PendingInput};
use crate::gui_contract::{GuiError, GuiHandle, GuiResult, SessionTarget};
use crate::project::{Feature, Project, ProjectStatus};
use crate::ui::list::format_age;

/// What the panel says about the part of the TUI's list it cannot reproduce.
pub const TUI_ONLY_LIMIT: &str = "Question, Completed and Waiting labels come from live hook \
events that only a running TUI receives and keeps in memory; they are not saved anywhere the \
desktop app can read. Rows below are the requests hooks left on disk, so a stopped agent that \
reported only to a TUI is not listed.";

const MESSAGE_MAX_CHARS: usize = 240;

#[derive(Debug, Clone, Serialize)]
pub struct AttentionRow {
    /// Stable identity of the request: its notification file.
    pub id: String,
    /// Modification time of the file in milliseconds, the dismiss guard.
    pub modified_ms: Option<i64>,
    pub project_id: Option<String>,
    pub feature_id: Option<String>,
    pub project_name: String,
    pub feature_name: String,
    /// The raw notification type on disk (`input-request`, `stop`, ...).
    pub kind: String,
    pub reason: String,
    pub message: String,
    pub age: Option<String>,
    pub age_secs: Option<i64>,
    /// Older than the configured `waiting_stale_minutes`.
    pub stale: bool,
    /// The session to open. `None` when the request cannot be tied to a
    /// feature; `jump_note` then says why.
    pub session: Option<SessionTarget>,
    pub jump_note: Option<String>,
    pub dismissable: bool,
    /// Why a row cannot be dismissed here.
    pub dismiss_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct NeedsAttentionView {
    pub rows: Vec<AttentionRow>,
    /// Session waits not listed because the feature is stopped.
    pub hidden_stopped: usize,
    /// Session waits not listed because the agent is working again; the TUI
    /// retires these on the same signal.
    pub hidden_resumed: usize,
    pub limits: Vec<String>,
}

fn is_session_wait(kind: &str) -> bool {
    matches!(kind, "input-request" | "stop")
}

fn reason_label(kind: &str) -> String {
    match kind {
        "input-request" => "Waiting for input".into(),
        "stop" => "Turn ended".into(),
        "diff-review" | "change-reason" => "Edit waiting for review".into(),
        "review-ready" => "Review fixes ready".into(),
        "" => "Pending request".into(),
        other => format!("Request: {other}"),
    }
}

fn truncate(message: &str) -> String {
    let flat = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= MESSAGE_MAX_CHARS {
        return flat;
    }
    let mut cut: String = flat.chars().take(MESSAGE_MAX_CHARS - 1).collect();
    cut.push('…');
    cut
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn resolve<'a>(app: &'a App, input: &PendingInput) -> Option<(&'a Project, &'a Feature)> {
    let project = app
        .store
        .projects
        .iter()
        .find(|p| Some(p.name.as_str()) == input.project_name.as_deref())?;
    let feature = project
        .features
        .iter()
        .find(|f| Some(f.name.as_str()) == input.feature_name.as_deref())?;
    Some((project, feature))
}

fn build(app: &App) -> GuiResult<NeedsAttentionView> {
    let live: std::collections::HashSet<String> = app
        .tmux
        .list_sessions()
        .map_err(GuiError::from)?
        .into_iter()
        .collect();
    let stale_after = app.config.waiting_stale_threshold();
    let mut view = NeedsAttentionView {
        limits: vec![TUI_ONLY_LIMIT.to_string()],
        ..Default::default()
    };
    let now = SystemTime::now();

    let mut rows: Vec<(bool, Option<SystemTime>, AttentionRow)> = Vec::new();
    for input in app.read_notification_files() {
        let kind = input.notification_type.clone();
        let wait = is_session_wait(&kind);
        let resolved = resolve(app, &input);

        if let (true, Some((_, feature))) = (wait, resolved) {
            if !live.contains(&feature.tmux_session) || feature.status == ProjectStatus::Stopped {
                view.hidden_stopped += 1;
                continue;
            }
            if app.thinking_from_shared_sources(&feature.tmux_session, &feature.agent) {
                view.hidden_resumed += 1;
                continue;
            }
        }

        let at = modified(&input.file_path);
        let age_secs = at
            .and_then(|t| now.duration_since(t).ok())
            .map(|d| d.as_secs() as i64);
        let age = at.map(|t| format_age(DateTime::<Utc>::from(t)));
        let stale = match (stale_after, age_secs) {
            (Some(limit), Some(secs)) => secs >= limit.as_secs() as i64,
            _ => false,
        };
        let session = resolved.and_then(|(project, feature)| {
            feature
                .sessions
                .iter()
                .find(|s| s.kind.is_agent_harness())
                .map(|s| SessionTarget {
                    project_id: project.id.clone(),
                    feature_id: feature.id.clone(),
                    session_id: s.id.clone(),
                })
        });
        let jump_note = match (resolved, &session) {
            (None, _) => Some(
                "This request could not be matched to a feature (its session name and folder \
                 match no feature), so there is nowhere to jump."
                    .to_string(),
            ),
            (Some(_), None) => Some("The feature has no agent session to open.".to_string()),
            (Some((_, feature)), Some(_))
                if feature
                    .sessions
                    .iter()
                    .filter(|s| s.kind.is_agent_harness())
                    .count()
                    > 1 =>
            {
                Some(
                    "The request does not say which of this feature's agent sessions raised \
                     it; this opens the first one."
                        .to_string(),
                )
            }
            _ => None,
        };
        let dismiss_note = (!wait).then(|| {
            "An agent is blocked on this request. Answer it in Supervised edits instead of \
             deleting it here."
                .to_string()
        });
        rows.push((
            wait,
            at,
            AttentionRow {
                id: input.file_path.to_string_lossy().into_owned(),
                modified_ms: at
                    .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
                    .map(|d| d.as_millis() as i64),
                project_id: resolved.map(|(p, _)| p.id.clone()),
                feature_id: resolved.map(|(_, f)| f.id.clone()),
                project_name: input
                    .project_name
                    .clone()
                    .unwrap_or_else(|| "unknown".into()),
                feature_name: input
                    .feature_name
                    .clone()
                    .unwrap_or_else(|| "unknown".into()),
                reason: reason_label(&kind),
                kind,
                message: truncate(&input.message),
                age,
                age_secs,
                stale,
                session,
                jump_note,
                dismissable: wait,
                dismiss_note,
            },
        ));
    }
    // Session waits first (the TUI lists explained stops before other
    // pending inputs), oldest first within each group.
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)).then(a.2.id.cmp(&b.2.id)));
    view.rows = rows.into_iter().map(|(_, _, row)| row).collect();
    if view.hidden_stopped > 0 {
        view.limits.push(format!(
            "{} waiting request(s) on stopped features are not listed; a stopped agent is not waiting.",
            view.hidden_stopped
        ));
    }
    if view.hidden_resumed > 0 {
        view.limits.push(format!(
            "{} waiting request(s) are not listed because the agent is working again.",
            view.hidden_resumed
        ));
    }
    Ok(view)
}

/// The current list. Read-only.
pub fn load(gui: &mut GuiHandle) -> GuiResult<NeedsAttentionView> {
    gui.refresh_store()?;
    build(gui.app_for_workflow())
}

/// The TUI's `x`: delete one session-wait notification file. The request is
/// re-read and matched on file path **and** modification time, so a request
/// that was replaced since the list loaded is not removed by a stale click.
pub fn dismiss(
    gui: &mut GuiHandle,
    id: &str,
    modified_ms: Option<i64>,
) -> GuiResult<NeedsAttentionView> {
    gui.refresh_store()?;
    let app = gui.app_for_workflow();
    // Already answered or retired elsewhere: the goal state holds.
    if let Some(input) = app
        .read_notification_files()
        .into_iter()
        .find(|input| input.file_path.to_string_lossy() == id)
    {
        if !is_session_wait(&input.notification_type) {
            return Err(GuiError::conflict(
                "Only waiting-for-input and turn-ended requests can be dismissed here; \
                 answer edit reviews in Supervised edits.",
            ));
        }
        let now_ms = modified(&input.file_path)
            .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64);
        if now_ms != modified_ms {
            return Err(GuiError::conflict(
                "That request changed since the list loaded. Review it again before dismissing.",
            ));
        }
        match std::fs::remove_file(&input.file_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(GuiError::conflict(format!(
                    "Could not remove the request file: {error}"
                )));
            }
        }
    }
    build(gui.app_for_workflow())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{AgentKind, ProjectStore, SessionKind, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};

    struct Fx {
        _dir: tempfile::TempDir,
        gui: GuiHandle,
        notif: std::path::PathBuf,
        target: SessionTarget,
    }

    fn fixture(live: bool, second_agent: bool) -> Fx {
        let dir = tempfile::tempdir().unwrap();
        let mut project = Project::new("Proj".into(), dir.path().into(), true, AgentKind::Claude);
        let mut feature = Feature::new_for_project(
            "Proj",
            "feat".into(),
            "feat".into(),
            dir.path().into(),
            true,
            VibeMode::default(),
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        let session_id = feature.add_session(SessionKind::Claude).id.clone();
        if second_agent {
            feature.add_session(SessionKind::Claude);
        }
        feature.status = ProjectStatus::Idle;
        let tmux_session = feature.tmux_session.clone();
        let target = SessionTarget {
            project_id: project.id.clone(),
            feature_id: feature.id.clone(),
            session_id,
        };
        project.features.push(feature);
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        let mut tmux = MockTmuxOps::new();
        tmux.expect_list_sessions().returning(move || {
            Ok(if live {
                vec![tmux_session.clone()]
            } else {
                vec![]
            })
        });
        let app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        let notif = dir.path().join(".claude").join("notifications");
        std::fs::create_dir_all(&notif).unwrap();
        Fx {
            _dir: dir,
            gui: GuiHandle::from_app(app),
            notif,
            target,
        }
    }

    fn write(fx: &Fx, name: &str, kind: &str, message: &str) -> std::path::PathBuf {
        let path = fx.notif.join(name);
        std::fs::write(
            &path,
            serde_json::json!({"type": kind, "message": message}).to_string(),
        )
        .unwrap();
        path
    }

    #[test]
    fn lists_disk_requests_with_reason_age_and_exact_jump_target() {
        let mut fx = fixture(true, false);
        write(&fx, "a.json", "input-request", "Which database?");
        let view = load(&mut fx.gui).unwrap();
        assert_eq!(view.rows.len(), 1);
        let row = &view.rows[0];
        assert_eq!(row.reason, "Waiting for input");
        assert_eq!(row.message, "Which database?");
        assert_eq!(row.age.as_deref(), Some("just now"));
        assert!(row.dismissable);
        assert!(row.jump_note.is_none());
        let session = row.session.as_ref().unwrap();
        assert_eq!(session.session_id, fx.target.session_id);
        assert_eq!(session.feature_id, fx.target.feature_id);
        // The TUI-only limit is always stated.
        assert!(view.limits[0].contains("only a running TUI"));
    }

    #[test]
    fn never_claims_question_or_completed_and_empty_dir_is_empty() {
        let mut fx = fixture(true, false);
        assert!(load(&mut fx.gui).unwrap().rows.is_empty());
        write(&fx, "s.json", "stop", "done");
        let view = load(&mut fx.gui).unwrap();
        assert_eq!(view.rows[0].reason, "Turn ended");
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("\"Question\"") && !json.contains("\"Completed\""));
    }

    #[test]
    fn stopped_feature_session_waits_are_hidden_with_a_stated_count() {
        let mut fx = fixture(false, false);
        write(&fx, "a.json", "input-request", "hi");
        write(&fx, "d.json", "diff-review", "edit");
        let view = load(&mut fx.gui).unwrap();
        assert_eq!(view.hidden_stopped, 1);
        assert_eq!(view.rows.len(), 1);
        assert_eq!(view.rows[0].kind, "diff-review");
        assert!(view.limits.iter().any(|l| l.contains("stopped features")));
    }

    #[test]
    fn edit_reviews_are_not_dismissable_and_refuse_with_a_reason() {
        let mut fx = fixture(true, false);
        let path = write(&fx, "d.json", "change-reason", "edit");
        let view = load(&mut fx.gui).unwrap();
        let row = &view.rows[0];
        assert!(!row.dismissable);
        assert!(
            row.dismiss_note
                .as_deref()
                .unwrap()
                .contains("Supervised edits")
        );
        let error = dismiss(&mut fx.gui, &row.id, row.modified_ms).unwrap_err();
        assert!(error.message.contains("Supervised edits"));
        assert!(path.exists());
    }

    #[test]
    fn dismiss_removes_exactly_one_file_and_is_idempotent() {
        let mut fx = fixture(true, false);
        let a = write(&fx, "a.json", "input-request", "one");
        let b = write(&fx, "b.json", "stop", "two");
        let view = load(&mut fx.gui).unwrap();
        let row = view.rows.iter().find(|r| r.message == "one").unwrap();
        let after = dismiss(&mut fx.gui, &row.id, row.modified_ms).unwrap();
        assert!(!a.exists() && b.exists());
        assert_eq!(after.rows.len(), 1);
        // Already gone: succeeds without touching the other request.
        let again = dismiss(&mut fx.gui, &row.id, row.modified_ms).unwrap();
        assert_eq!(again.rows.len(), 1);
        assert!(b.exists());
    }

    #[test]
    fn dismiss_refuses_a_request_replaced_since_the_list_loaded() {
        let mut fx = fixture(true, false);
        let path = write(&fx, "a.json", "input-request", "old");
        let view = load(&mut fx.gui).unwrap();
        let row = view.rows[0].clone();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(
            &path,
            serde_json::json!({"type": "input-request", "message": "new"}).to_string(),
        )
        .unwrap();
        let error = dismiss(&mut fx.gui, &row.id, row.modified_ms).unwrap_err();
        assert!(error.message.contains("changed"));
        assert!(path.exists());
    }

    #[test]
    fn dismiss_cannot_name_an_arbitrary_path() {
        let mut fx = fixture(true, false);
        let outside = fx._dir.path().join("keep.txt");
        std::fs::write(&outside, "x").unwrap();
        let view = dismiss(&mut fx.gui, &outside.to_string_lossy(), None).unwrap();
        assert!(outside.exists());
        assert!(view.rows.is_empty());
    }

    #[test]
    fn multiple_agent_sessions_say_the_jump_is_a_guess_and_oldest_sorts_first() {
        let mut fx = fixture(true, true);
        let old = write(&fx, "old.json", "input-request", "old");
        let file = std::fs::File::options().write(true).open(&old).unwrap();
        file.set_modified(SystemTime::now() - std::time::Duration::from_secs(7200))
            .unwrap();
        write(&fx, "new.json", "input-request", "new");
        let view = load(&mut fx.gui).unwrap();
        // Same feature, same stop: both listed, oldest first.
        assert_eq!(view.rows[0].message, "old");
        assert_eq!(view.rows[0].age.as_deref(), Some("2h ago"));
        assert!(
            view.rows[0]
                .jump_note
                .as_deref()
                .unwrap()
                .contains("first one")
        );
    }

    #[test]
    fn long_messages_are_cut_on_a_character_boundary() {
        let long = "é".repeat(500);
        let cut = truncate(&long);
        assert_eq!(cut.chars().count(), MESSAGE_MAX_CHARS);
        assert!(cut.ends_with('…'));
    }
}
