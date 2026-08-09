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

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::app::App;
use crate::db::editors::{EditorKind, LaunchedEditor};
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
    /// The process AMF opened is now hosting other windows too. VS Code is a
    /// singleton: if AMF's launch is what started the application, every window
    /// the user opened afterwards lives in the same process tree, and killing
    /// it would close their work.
    SharedInstance,
}

impl SkipReason {
    pub fn explain(self) -> &'static str {
        match self {
            SkipReason::NotOwned => "AMF did not open this window",
            SkipReason::AlreadyGone => "already closed",
            SkipReason::PidRecycled => "pid was recycled",
            SkipReason::SharedInstance => "other windows share this instance",
        }
    }
}

/// Where a not-yet-attributed VS Code launch stands.
///
/// The `code` CLI exits before its window exists, so ownership is settled on a
/// background thread. The lock is the handoff: whoever takes it first decides,
/// and the resolver holds it across attributing *and* writing, so a stop can
/// never land in between and lose the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PendingLaunchState {
    /// Still looking for the window.
    Resolving,
    /// The feature was stopped first — close the window on sight instead of
    /// recording it.
    Reclaim,
    /// The resolver is finished; the database row is now the truth.
    Done,
}

/// A VS Code launch whose owner process has not been resolved yet.
#[derive(Debug)]
pub(crate) struct PendingEditorLaunch {
    pub feature_id: String,
    /// The `launched_editors` row the resolver will update.
    pub record_id: String,
    pub kind: EditorKind,
    pub state: Arc<Mutex<PendingLaunchState>>,
}

/// Take the lock, tolerating a panicked holder: a poisoned mutex here means a
/// resolver thread died mid-decision, and the state it left is still the truth.
pub(crate) fn lock_state(
    state: &Mutex<PendingLaunchState>,
) -> std::sync::MutexGuard<'_, PendingLaunchState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What [`App::kill_tracked_editors`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EditorKillReport {
    /// `(editor name, pids ended)` per window closed.
    pub killed: Vec<(String, Vec<i64>)>,
    /// `(editor name, why)` per window left alone.
    pub skipped: Vec<(String, SkipReason)>,
    /// Editor names whose window had not finished opening yet. These are not
    /// skipped: the launch's own resolver closes them as soon as it can name
    /// them.
    pub pending: Vec<String>,
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
        for name in &self.pending {
            parts.push(format!("{name} will close once its window opens"));
        }
        (!parts.is_empty()).then(|| parts.join("; "))
    }
}

/// Whether a live PID is still the editor that was recorded.
///
/// A stored PID outlives AMF restarts and reboots, so liveness alone proves
/// nothing: the number may have been handed to something else entirely. Two
/// things have to hold.
///
/// *Argv* must still name this editor on this worktree — matched on path
/// boundaries, so a window on `…/feature-two` is not taken for one on `…/feat`.
///
/// *Start time* must still be the one recorded when the window was attributed.
/// Argv alone cannot separate AMF's window from a user-opened VS Code that
/// happens to have inherited the PID and be sitting on the same worktree — it
/// looks identical, because it is identical apart from who started it. The
/// process's own start time is the part it cannot reproduce. Records written
/// before this was tracked (and launches whose owner was never resolved) have
/// no start time to compare, and fall back to the argv check.
fn still_the_same_editor(editor: &LaunchedEditor) -> bool {
    let Some(args) = procs::args_for_pid(editor.pid) else {
        return false;
    };
    if !procs::is_vscode_for_workdir(&args, &editor.worktree_path) {
        return false;
    }
    if editor.proc_started_at.is_empty() {
        return true;
    }
    // A missing reading is "cannot tell", and cannot tell is not a licence to
    // kill.
    procs::start_time_for_pid(editor.pid).as_deref() == Some(editor.proc_started_at.as_str())
}

impl App {
    /// Tracked editor windows that are open right now, as
    /// `"VS Code — feature-name"`.
    ///
    /// Used where memory is the thing being explained: an editor is not an
    /// agent and is not counted as one, but its language servers are usually
    /// the larger half of why memory is short, so naming them turns "you are
    /// low on memory" into something the reader can act on.
    pub fn open_tracked_editors(&self) -> Vec<String> {
        let Some(db) = self.db.as_ref() else {
            return Vec::new();
        };
        let Ok(editors) = db.all_launched_editors() else {
            return Vec::new();
        };
        editors
            .iter()
            .filter(|editor| procs::pid_alive(editor.pid))
            .map(|editor| {
                let feature = self
                    .store
                    .projects
                    .iter()
                    .flat_map(|project| project.features.iter())
                    .find(|feature| feature.id == editor.feature_id)
                    .map(|feature| feature.name.clone());
                match feature {
                    Some(name) => format!("{} — {name}", editor.kind.display_name()),
                    None => editor.kind.display_name().to_string(),
                }
            })
            .collect()
    }

    /// Close the editors AMF opened for a feature, and forget the records it
    /// resolved either way. Records for windows that are alive but not AMF's
    /// are kept, so `amf doctor` can still point at them.
    pub fn kill_tracked_editors(&mut self, feature_id: &str) -> EditorKillReport {
        let mut report = EditorKillReport::default();
        // Claim the launches still resolving *before* reading the rows: a
        // launch that finishes in between shows up in the rows as owned and is
        // killed here, and one that has not finished is now the resolver's job.
        let claimed = self.claim_pending_editor_launches(feature_id, &mut report);

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

            if claimed.contains(&editor.id) {
                // Already accounted for as pending; its resolver will close it.
                continue;
            }
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

            // Owning the process is not the same as owning everything in it.
            // VS Code runs one application process for the machine, so if AMF's
            // launch is the one that started it, the user's later windows are
            // in this same tree. Reclaiming memory is never worth closing
            // someone's other editors.
            let windows = procs::vscode_window_count(&procs::list_processes(), editor.pid);
            if windows > 1 {
                report.skipped.push((name, SkipReason::SharedInstance));
                self.log_info(
                    "editor",
                    format!(
                        "pid {} is hosting {windows} windows - leaving it for the user to close",
                        editor.pid
                    ),
                );
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

    /// Hand every still-resolving launch for `feature_id` over to its own
    /// resolver, and report them as pending.
    ///
    /// Returns the record ids claimed, so the row walk can skip them instead of
    /// reporting them as windows AMF does not own — which is what a stop during
    /// the resolve window used to do, leaving a real editor running with a
    /// message saying it was never AMF's.
    fn claim_pending_editor_launches(
        &mut self,
        feature_id: &str,
        report: &mut EditorKillReport,
    ) -> std::collections::HashSet<String> {
        let mut claimed = std::collections::HashSet::new();
        for launch in &self.pending_editor_launches {
            if launch.feature_id != feature_id {
                continue;
            }
            let mut state = lock_state(&launch.state);
            if *state == PendingLaunchState::Resolving {
                *state = PendingLaunchState::Reclaim;
                claimed.insert(launch.record_id.clone());
                report.pending.push(launch.kind.display_name().to_string());
            }
        }
        self.prune_resolved_editor_launches();
        claimed
    }

    /// Forget launches whose resolver has finished. Called from the launch and
    /// stop paths, which is often enough: the list only ever holds launches
    /// from this run of AMF, and only for the seconds it takes a window to
    /// appear.
    pub(crate) fn prune_resolved_editor_launches(&mut self) {
        self.pending_editor_launches
            .retain(|launch| *lock_state(&launch.state) != PendingLaunchState::Done);
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
            ..Default::default()
        };
        assert_eq!(
            report.summary().as_deref(),
            Some("closed 1 editor (3 processes)")
        );
    }

    #[test]
    fn summary_stays_quiet_about_editors_that_had_already_exited() {
        let report = EditorKillReport {
            skipped: vec![("VS Code".into(), SkipReason::AlreadyGone)],
            ..Default::default()
        };
        assert_eq!(report.summary(), None);
    }

    #[test]
    fn summary_says_when_a_window_was_deliberately_left_alone() {
        let report = EditorKillReport {
            skipped: vec![("VS Code".into(), SkipReason::NotOwned)],
            ..Default::default()
        };
        assert_eq!(
            report.summary().as_deref(),
            Some("left VS Code running (AMF did not open this window)")
        );
    }

    #[test]
    fn summary_says_when_other_windows_share_the_instance() {
        let report = EditorKillReport {
            skipped: vec![("VS Code".into(), SkipReason::SharedInstance)],
            ..Default::default()
        };
        assert_eq!(
            report.summary().as_deref(),
            Some("left VS Code running (other windows share this instance)")
        );
    }

    #[test]
    fn summary_says_a_window_still_opening_will_be_closed() {
        // The stop happened inside the resolve window: the honest report is
        // that the editor is still coming, not that it was skipped.
        let report = EditorKillReport {
            pending: vec!["VS Code".into()],
            ..Default::default()
        };
        assert_eq!(
            report.summary().as_deref(),
            Some("VS Code will close once its window opens")
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
            proc_started_at: String::new(),
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
            proc_started_at: String::new(),
            started_at: chrono::Utc::now(),
        };
        assert!(
            !still_the_same_editor(&editor),
            "a recycled pid must never be signalled"
        );
    }

    /// A live process with a VS Code-shaped argv on the worktree — argv alone
    /// cannot tell it apart from the window AMF opened, which is the whole
    /// reason the start time is recorded.
    fn spawn_lookalike(dir: &std::path::Path, workdir: &std::path::Path) -> std::process::Child {
        let fake = dir.join("code");
        std::fs::copy("/bin/sh", &fake).expect("copy sh");
        let mut command = std::process::Command::new(&fake);
        command
            .args([
                "-c".as_ref(),
                "sleep 60".as_ref(),
                "--new-window".as_ref(),
                workdir.as_os_str(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        // Retry `ETXTBSY`: a concurrent test forking between the copy and the
        // exec inherits the write descriptor and makes the exec fail.
        for _ in 0..40 {
            match command.spawn() {
                Ok(child) => return child,
                Err(err) if err.raw_os_error() == Some(26) => {
                    std::thread::sleep(Duration::from_millis(50))
                }
                Err(err) => panic!("stand-in editor should launch: {err}"),
            }
        }
        panic!("stand-in editor stayed busy");
    }

    fn recorded(pid: i64, workdir: &std::path::Path, proc_started_at: &str) -> LaunchedEditor {
        LaunchedEditor {
            id: "e1".into(),
            feature_id: "f1".into(),
            session_id: None,
            kind: EditorKind::Vscode,
            pid,
            worktree_path: workdir.to_path_buf(),
            dedicated: true,
            command: "code --new-window".into(),
            proc_started_at: proc_started_at.to_string(),
            started_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn the_recorded_start_time_confirms_the_editor() {
        let tmp = tempfile::TempDir::new().unwrap();
        let workdir = tmp.path().join("worktree");
        std::fs::create_dir_all(&workdir).unwrap();
        let mut child = spawn_lookalike(tmp.path(), &workdir);
        let pid = child.id() as i64;
        std::thread::sleep(Duration::from_millis(200));

        let started = procs::start_time_for_pid(pid).expect("a live process has a start time");
        assert!(still_the_same_editor(&recorded(pid, &workdir, &started)));
        // Legacy rows carry no start time and fall back to the argv check.
        assert!(still_the_same_editor(&recorded(pid, &workdir, "")));

        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn a_lookalike_that_started_at_a_different_time_is_not_the_editor() {
        // The PID-recycling case argv cannot catch: the user's own VS Code, on
        // the same worktree, holding the number AMF recorded. Only the start
        // time separates it from the window AMF opened.
        let tmp = tempfile::TempDir::new().unwrap();
        let workdir = tmp.path().join("worktree");
        std::fs::create_dir_all(&workdir).unwrap();
        let mut child = spawn_lookalike(tmp.path(), &workdir);
        let pid = child.id() as i64;
        std::thread::sleep(Duration::from_millis(200));

        let editor = recorded(pid, &workdir, "Thu Jan 1 00:00:00 1970");
        assert!(
            !still_the_same_editor(&editor),
            "argv matches, but this is not the process AMF launched"
        );

        let _ = child.kill();
        let _ = child.wait();
    }
}
