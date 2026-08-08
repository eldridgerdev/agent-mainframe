//! Reclaiming editors AMF launched for a feature.
//!
//! An editor window is usually the largest thing a feature leaves behind — a
//! language server on a medium Rust repo outweighs every agent harness on the
//! machine put together — and, unlike everything else a feature owns, it is not
//! in tmux, so stopping the feature never touched it.
//!
//! The rule is ownership, not tidiness: AMF only ever signals a window it
//! opened itself with `--new-window` *and* can still identify (see
//! [`crate::db::editors`]). Anything else is reported as skipped, never killed.

use std::time::Duration;

use crate::app::App;
use crate::db::editors::LaunchedEditor;
use crate::resources::procs;

/// How long a signalled editor gets to exit on its own before `SIGKILL`.
/// Polled, so a quick exit costs a fraction of this.
const EDITOR_KILL_GRACE: Duration = Duration::from_secs(2);

/// Why an editor AMF knew about was left running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// AMF handed the folder to an instance it did not open (or the window
    /// never appeared locally, as with a remote/WSL editor).
    NotOwned,
    /// The process is already gone.
    AlreadyGone,
    /// The PID is alive but is no longer the editor AMF launched — the OS
    /// recycled it. Signalling would hit an unrelated process.
    PidRecycled,
}

impl SkipReason {
    pub fn explain(self) -> &'static str {
        match self {
            SkipReason::NotOwned => "AMF did not open this window",
            SkipReason::AlreadyGone => "already closed",
            SkipReason::PidRecycled => "pid was recycled",
        }
    }
}

/// What [`App::kill_tracked_editors`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EditorKillReport {
    /// `(editor name, pids ended)` per window closed.
    pub killed: Vec<(String, Vec<i64>)>,
    /// `(editor name, why)` per window left alone.
    pub skipped: Vec<(String, SkipReason)>,
}

impl EditorKillReport {
    /// One-line summary for the status bar, or `None` when there is nothing
    /// worth saying (no tracked editors at all, or only ones already closed).
    pub fn summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.killed.is_empty() {
            let processes: usize = self.killed.iter().map(|(_, pids)| pids.len()).sum();
            parts.push(format!(
                "closed {} editor{} ({processes} process{})",
                self.killed.len(),
                if self.killed.len() == 1 { "" } else { "s" },
                if processes == 1 { "" } else { "es" }
            ));
        }
        // "Already gone" is the ordinary case and not worth reporting; a window
        // deliberately left alone is.
        for (name, reason) in &self.skipped {
            if *reason != SkipReason::AlreadyGone {
                parts.push(format!("left {name} running ({})", reason.explain()));
            }
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }
}

/// Whether a live PID is still the editor that was recorded.
///
/// A stored PID outlives AMF restarts and reboots, so liveness alone proves
/// nothing: the number may have been handed to something else entirely. The
/// process must still look like the same editor on the same worktree.
fn still_the_same_editor(editor: &LaunchedEditor) -> bool {
    match procs::args_for_pid(editor.pid) {
        Some(args) => procs::is_vscode_for_workdir(&args, &editor.worktree_path),
        None => false,
    }
}

impl App {
    /// Close the editors AMF opened for a feature, and forget the records it
    /// resolved either way. Records for windows that are alive but not AMF's
    /// are kept, so `amf doctor` can still point at them.
    pub fn kill_tracked_editors(&mut self, feature_id: &str) -> EditorKillReport {
        let mut report = EditorKillReport::default();
        let Some(db) = self.db.as_ref() else {
            return report;
        };
        let editors = match db.launched_editors_for_feature(feature_id) {
            Ok(editors) => editors,
            Err(err) => {
                self.log_warn("editor", format!("could not read tracked editors: {err}"));
                return report;
            }
        };

        for editor in editors {
            let name = editor.kind.display_name().to_string();

            if !editor.dedicated {
                report.skipped.push((name, SkipReason::NotOwned));
                continue;
            }
            if !procs::pid_alive(editor.pid) {
                report.skipped.push((name, SkipReason::AlreadyGone));
                self.forget_editor(&editor);
                continue;
            }
            if !still_the_same_editor(&editor) {
                report.skipped.push((name, SkipReason::PidRecycled));
                self.log_warn(
                    "editor",
                    format!(
                        "pid {} is alive but is no longer {} for {} - not signalling it",
                        editor.pid,
                        editor.kind.display_name(),
                        editor.worktree_path.display()
                    ),
                );
                self.forget_editor(&editor);
                continue;
            }

            // The window's children are the point: language servers are what
            // actually hold the memory.
            let ended = procs::terminate_tree(editor.pid, EDITOR_KILL_GRACE);
            self.log_info(
                "editor",
                format!(
                    "closed {} (pid {}) for {} - {} process(es) ended",
                    editor.kind.display_name(),
                    editor.pid,
                    editor.worktree_path.display(),
                    ended.len()
                ),
            );
            report.killed.push((name, ended));
            self.forget_editor(&editor);
        }

        report
    }

    fn forget_editor(&mut self, editor: &LaunchedEditor) {
        if let Some(db) = self.db.as_ref()
            && let Err(err) = db.delete_launched_editor(&editor.id)
        {
            self.log_warn("editor", format!("could not forget editor record: {err}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::editors::EditorKind;

    #[test]
    fn summary_reports_closed_windows() {
        let report = EditorKillReport {
            killed: vec![("VS Code".into(), vec![1, 2, 3])],
            skipped: Vec::new(),
        };
        assert_eq!(
            report.summary().as_deref(),
            Some("closed 1 editor (3 processes)")
        );
    }

    #[test]
    fn summary_stays_quiet_about_editors_that_had_already_exited() {
        let report = EditorKillReport {
            killed: Vec::new(),
            skipped: vec![("VS Code".into(), SkipReason::AlreadyGone)],
        };
        assert_eq!(report.summary(), None);
    }

    #[test]
    fn summary_says_when_a_window_was_deliberately_left_alone() {
        let report = EditorKillReport {
            killed: Vec::new(),
            skipped: vec![("VS Code".into(), SkipReason::NotOwned)],
        };
        assert_eq!(
            report.summary().as_deref(),
            Some("left VS Code running (AMF did not open this window)")
        );
    }

    #[test]
    fn a_dead_pid_is_never_taken_for_the_recorded_editor() {
        // PID 0 stands in for "no owner resolved": the identity check must
        // fail rather than fall through to a kill.
        let editor = LaunchedEditor {
            id: "e1".into(),
            feature_id: "f1".into(),
            session_id: None,
            kind: EditorKind::Vscode,
            pid: 0,
            worktree_path: std::path::PathBuf::from("/tmp/wt"),
            dedicated: true,
            command: "code --new-window /tmp/wt".into(),
            started_at: chrono::Utc::now(),
        };
        assert!(!still_the_same_editor(&editor));
    }

    #[test]
    fn an_unrelated_live_process_fails_the_identity_check() {
        // This test process is alive, but it is not VS Code on that worktree.
        let editor = LaunchedEditor {
            id: "e1".into(),
            feature_id: "f1".into(),
            session_id: None,
            kind: EditorKind::Vscode,
            pid: std::process::id() as i64,
            worktree_path: std::path::PathBuf::from("/tmp/wt"),
            dedicated: true,
            command: "code --new-window /tmp/wt".into(),
            started_at: chrono::Utc::now(),
        };
        assert!(
            !still_the_same_editor(&editor),
            "a recycled pid must never be signalled"
        );
    }
}
