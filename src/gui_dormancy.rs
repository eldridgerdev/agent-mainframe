//! Desktop dormancy: which running features are idle *and* unattended, and an
//! explicitly confirmed stop for the ones the user picks.
//!
//! Detection is the TUI's own engine (`app::dormant`: tmux `window_activity`
//! for idleness, `Feature::last_accessed` for attention, the configured
//! thresholds, and the same "a missing signal is never dormant" rule).
//! Stopping is the TUI's own stop (`App::stop_feature_reporting`): custom
//! session `on_stop`, the feature's tmux session, plan instructions and
//! tracked-editor cleanup with every one of its ownership rules. Nothing here
//! signals a process itself.
//!
//! What this module adds is what a *delayed* confirmation needs. A listed row
//! carries the readings it was listed on ([`DormantObservation`]), and the
//! stop re-checks each selected feature against current state immediately
//! before stopping it. A feature that was deleted, already stopped,
//! restarted, opened, produced output, or simply is no longer dormant is
//! refused with a stated reason rather than stopped on the strength of an old
//! list.

use std::collections::HashSet;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::app::editor_ops::{EditorKillReport, SkipReason};
use crate::gui_contract::{FeatureTarget, GuiHandle, GuiResult, SessionTarget};
use crate::project::ProjectStatus;

#[derive(Debug, Clone, Serialize)]
pub struct DormancyView {
    /// `false` when either threshold is `0` in config, which switches the
    /// whole check off (dormancy is an AND of both halves).
    pub enabled: bool,
    pub idle_minutes: u64,
    pub unattended_hours: u64,
    /// Whether stopping also closes editors AMF opened (`kill_editor_on_stop`).
    pub kill_editor_on_stop: bool,
    pub checked_at: DateTime<Utc>,
    /// Longest-idle first, as the TUI lists them.
    pub features: Vec<DormantFeatureView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DormantFeatureView {
    pub project_name: String,
    pub workdir: String,
    pub is_worktree: bool,
    /// A tracked editor of this feature is still running.
    pub editor_alive: bool,
    /// Seconds since the agent last produced output.
    pub idle_secs: u64,
    /// Seconds since the feature was last opened in AMF.
    pub unattended_secs: u64,
    pub observation: DormantObservation,
}

/// The readings a row was listed on. The frontend sends these back verbatim
/// to stop the selection, and each is compared with the current state first.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DormantObservation {
    pub target: FeatureTarget,
    /// Display only; never used to find the feature.
    pub feature_name: String,
    pub tmux_session: String,
    pub last_activity: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DormancyStopResult {
    pub target: FeatureTarget,
    pub feature_name: String,
    #[serde(flatten)]
    pub outcome: DormancyStopOutcome,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DormancyStopOutcome {
    /// `editors` is `None` when `kill_editor_on_stop` is off, so no editor
    /// was examined at all.
    Stopped {
        editors: Option<EditorCleanupView>,
    },
    Refused {
        reason: DormancyRefusal,
        message: String,
    },
    Failed {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DormancyRefusal {
    /// The same feature appears more than once in one request.
    Duplicate,
    Deleted,
    AlreadyStopped,
    /// AMF still records it as running but its tmux session no longer exists
    /// (a crash or a tmux server restart), so there is nothing left to stop.
    SessionGone,
    /// Its tmux session is not the one that was listed.
    Restarted,
    /// Several features share the live tmux session; stopping it would stop
    /// features the user did not select.
    SharedSession,
    /// Dormancy was switched off in config.
    DormancyOff,
    /// Opened in AMF since the list was loaded.
    Opened,
    /// The agent produced output since the list was loaded.
    Output,
    /// No longer idle and unattended for any other reason.
    NoLongerDormant,
}

/// [`EditorKillReport`], whole: the status line's summary drops "already
/// closed" entries, which a per-feature result can afford to show.
#[derive(Debug, Clone, Default, Serialize)]
pub struct EditorCleanupView {
    pub killed: Vec<KilledEditor>,
    pub skipped: Vec<SkippedEditor>,
    /// Windows still opening; their own resolver closes them once it can
    /// name them.
    pub pending: Vec<String>,
    /// The TUI's one-line summary, when there is anything worth saying.
    pub summary: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct KilledEditor {
    pub name: String,
    pub processes: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkippedEditor {
    pub name: String,
    pub reason: &'static str,
    /// Left running on purpose, as opposed to already closed.
    pub deliberate: bool,
}

impl From<&EditorKillReport> for EditorCleanupView {
    fn from(report: &EditorKillReport) -> Self {
        Self {
            killed: report
                .killed
                .iter()
                .map(|(name, pids)| KilledEditor {
                    name: name.clone(),
                    processes: pids.len(),
                })
                .collect(),
            skipped: report
                .skipped
                .iter()
                .map(|(name, reason)| SkippedEditor {
                    name: name.clone(),
                    reason: reason.explain(),
                    deliberate: *reason != SkipReason::AlreadyGone,
                })
                .collect(),
            pending: report.pending.clone(),
            summary: report.summary(),
        }
    }
}

/// The dormant features right now. Read-only: listing never stops, touches
/// or saves anything.
pub fn load(gui: &mut GuiHandle) -> GuiResult<DormancyView> {
    gui.refresh_snapshot()?;
    let app = gui.app_for_workflow();
    let scan = app.scan_dormant_features();
    let mut features = Vec::new();
    if let Some(scan) = &scan {
        for dormant in &scan.features {
            let project = &app.store.projects[dormant.pi];
            let feature = &project.features[dormant.fi];
            // Dormancy is only ever decided on a reading; a row without one
            // cannot exist, but would have nothing to be re-checked against.
            let Some(last_activity) = scan.activity.get(&feature.tmux_session) else {
                continue;
            };
            features.push(DormantFeatureView {
                project_name: dormant.project_name.clone(),
                workdir: dormant.workdir.to_string_lossy().into_owned(),
                is_worktree: dormant.is_worktree,
                editor_alive: dormant.editor_alive,
                idle_secs: dormant.idle.as_secs(),
                unattended_secs: dormant.unattended.as_secs(),
                observation: DormantObservation {
                    target: FeatureTarget {
                        project_id: project.id.clone(),
                        feature_id: feature.id.clone(),
                    },
                    feature_name: feature.name.clone(),
                    tmux_session: feature.tmux_session.clone(),
                    last_activity: *last_activity,
                    last_accessed: feature.last_accessed,
                },
            });
        }
    }
    Ok(DormancyView {
        enabled: scan.is_some(),
        idle_minutes: app.config.dormant_idle_minutes,
        unattended_hours: app.config.dormant_last_accessed_hours,
        kill_editor_on_stop: app.config.kill_editor_on_stop,
        checked_at: scan.map(|scan| scan.now).unwrap_or_else(Utc::now),
        features,
    })
}

/// Stop the confirmed selection, one feature at a time, each re-checked
/// against current state immediately before its stop. One feature's refusal
/// or failure never stops the rest from being considered; every selected row
/// gets its own result, in request order.
///
/// The handle is locked per feature, not for the batch: a stop can wait on
/// editor cleanup's grace period, and every other command (terminal input
/// included) needs the same lock. Releasing it between features is safe
/// because each one is re-checked from a fresh snapshot anyway.
pub fn stop(
    gui: &Mutex<GuiHandle>,
    selection: Vec<DormantObservation>,
) -> GuiResult<Vec<DormancyStopResult>> {
    if selection.is_empty() {
        return Err(crate::gui_contract::GuiError::conflict(
            "Select at least one dormant feature to stop",
        ));
    }
    let mut seen = HashSet::new();
    let mut results = Vec::with_capacity(selection.len());
    for observation in selection {
        let outcome = if seen.insert(observation.target.feature_id.clone()) {
            stop_one(
                &mut gui.lock().expect("gui handle mutex poisoned"),
                &observation,
            )
        } else {
            refused(
                DormancyRefusal::Duplicate,
                "Selected more than once in this request; it is stopped at most once",
            )
        };
        results.push(DormancyStopResult {
            target: observation.target,
            feature_name: observation.feature_name,
            outcome,
        });
    }
    Ok(results)
}

fn refused(reason: DormancyRefusal, message: impl Into<String>) -> DormancyStopOutcome {
    DormancyStopOutcome::Refused {
        reason,
        message: message.into(),
    }
}

fn stop_one(gui: &mut GuiHandle, observed: &DormantObservation) -> DormancyStopOutcome {
    // Adopt anything another AMF process committed (a TUI stop, delete or
    // open) before judging, so "already stopped" means what it says.
    if let Err(error) = gui.refresh_snapshot() {
        return DormancyStopOutcome::Failed {
            message: error.message,
        };
    }
    let app = gui.app_for_workflow();
    let Some((pi, fi)) = app.store.locate_feature_by_id(
        Some(&observed.target.project_id),
        &observed.target.feature_id,
    ) else {
        return refused(
            DormancyRefusal::Deleted,
            "It was deleted after the list was loaded",
        );
    };
    let feature = &app.store.projects[pi].features[fi];
    if feature.status == ProjectStatus::Stopped {
        return refused(DormancyRefusal::AlreadyStopped, "It is already stopped");
    }
    if !app.tmux.session_exists(&feature.tmux_session) {
        return refused(
            DormancyRefusal::SessionGone,
            "Its tmux session is gone (it may have crashed, or the tmux server restarted), so there is nothing left to stop",
        );
    }
    if feature.tmux_session != observed.tmux_session {
        return refused(
            DormancyRefusal::Restarted,
            "Its tmux session changed after the list was loaded",
        );
    }
    if app.feature_tmux_session_is_shared(pi, fi) {
        return refused(
            DormancyRefusal::SharedSession,
            "Other features share its live tmux session, so stopping it would stop them too",
        );
    }
    if feature.last_accessed > observed.last_accessed {
        return refused(
            DormancyRefusal::Opened,
            "It was opened in AMF after the list was loaded",
        );
    }
    let Some(scan) = app.scan_dormant_features() else {
        return refused(
            DormancyRefusal::DormancyOff,
            "Dormancy detection is switched off in config",
        );
    };
    if scan
        .activity
        .get(&feature.tmux_session)
        .is_some_and(|at| *at > observed.last_activity)
    {
        return refused(
            DormancyRefusal::Output,
            "Its agent produced output after the list was loaded",
        );
    }
    if !scan
        .features
        .iter()
        .any(|dormant| dormant.pi == pi && dormant.fi == fi)
    {
        return refused(
            DormancyRefusal::NoLongerDormant,
            "It is no longer idle and unattended",
        );
    }

    match app.stop_feature_reporting(pi, fi) {
        Ok(report) => DormancyStopOutcome::Stopped {
            editors: report.as_ref().map(EditorCleanupView::from),
        },
        Err(error) => DormancyStopOutcome::Failed {
            message: error.to_string(),
        },
    }
}

/// The GUI's counterpart of the TUI's `enter_view` touch: showing a session's
/// terminal is opening the feature, so it counts as attention for dormancy.
/// Only `last_accessed` changes; the TUI's view-only `Active` status is not
/// copied, since the GUI derives running state from tmux. Best effort — a
/// feature deleted meanwhile is left alone and the attach still proceeds.
pub fn note_opened(gui: &mut GuiHandle, target: &SessionTarget) -> GuiResult<()> {
    let app = gui.app_for_workflow();
    let touch = |store: &mut crate::project::ProjectStore| {
        let Some((pi, fi)) =
            store.locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        else {
            return false;
        };
        store.projects[pi].features[fi].touch();
        true
    };
    if !touch(&mut app.store) {
        return Ok(());
    }
    app.save_reapplying(touch)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::gui_contract::GuiErrorKind;
    use crate::project::{AgentKind, Feature, Project, ProjectStore, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    const PROJECT: &str = "proj-1";

    fn feature(id: &str, unattended_hours: i64) -> Feature {
        let mut feature = Feature::new_for_project(
            "demo",
            id.to_string(),
            id.to_string(),
            PathBuf::from("/tmp").join(id),
            true,
            VibeMode::default(),
            false,
            false,
            AgentKind::default(),
            false,
            false,
        );
        feature.id = id.to_string();
        feature.tmux_session = format!("amf-{id}");
        feature.status = ProjectStatus::Idle;
        feature.last_accessed = Utc::now() - chrono::Duration::hours(unattended_hours);
        feature
    }

    fn store(features: Vec<Feature>) -> ProjectStore {
        let mut project = Project::new(
            "demo".to_string(),
            PathBuf::from("/tmp/demo-repo"),
            true,
            AgentKind::default(),
        );
        project.id = PROJECT.to_string();
        project.features = features;
        let mut store = ProjectStore::empty();
        store.projects.push(project);
        store
    }

    /// tmux activity, shared with the mock so a test can move it after the
    /// list was loaded, as an agent producing output would.
    type Activity = Arc<Mutex<Vec<(String, String, i64)>>>;

    fn idle_for(session: &str, minutes: i64) -> (String, String, i64) {
        (
            session.to_string(),
            "claude".to_string(),
            (Utc::now() - chrono::Duration::minutes(minutes)).timestamp(),
        )
    }

    /// A tmux whose sessions are live while listed in `live`, and which
    /// records every kill so a test can assert exactly what was stopped.
    fn tmux(activity: Activity, live: Arc<Mutex<HashSet<String>>>) -> (MockTmuxOps, Activity) {
        let killed: Activity = Arc::new(Mutex::new(Vec::new()));
        let mut tmux = MockTmuxOps::new();
        tmux.expect_window_activity()
            .returning(move || activity.lock().unwrap().clone());
        let exists = live.clone();
        tmux.expect_session_exists()
            .returning(move |session| exists.lock().unwrap().contains(session));
        let record = killed.clone();
        tmux.expect_kill_session().returning(move |session| {
            live.lock().unwrap().remove(session);
            record
                .lock()
                .unwrap()
                .push((session.to_string(), String::new(), 0));
            Ok(())
        });
        (tmux, killed)
    }

    struct Fixture {
        gui: Mutex<GuiHandle>,
        activity: Activity,
        killed: Activity,
    }

    impl Fixture {
        fn gui(&mut self) -> &mut GuiHandle {
            self.gui.get_mut().unwrap()
        }

        fn killed(&self) -> Vec<String> {
            self.killed
                .lock()
                .unwrap()
                .iter()
                .map(|(session, _, _)| session.clone())
                .collect()
        }

        /// Persist the current store to a fresh database and attach it, so
        /// confirm-time refreshes reload exactly this store.
        fn attach_db(&mut self) -> tempfile::NamedTempFile {
            let file = tempfile::NamedTempFile::new().unwrap();
            let db = crate::db::AmfDb::open(file.path()).unwrap();
            let app = self.gui().app_for_workflow();
            db.save_store(&app.store).unwrap();
            app.db = Some(db);
            file
        }

        fn status(&mut self, id: &str) -> ProjectStatus {
            let app = self.gui().app_for_workflow();
            let (pi, fi) = app.store.locate_feature_by_id(None, id).unwrap();
            app.store.projects[pi].features[fi].status.clone()
        }
    }

    /// `quiet` and `quieter` are dormant; `busy` is producing output and
    /// `opened` was opened a minute ago.
    fn fixture() -> Fixture {
        let features = vec![
            feature("quiet", 10),
            feature("quieter", 20),
            feature("busy", 20),
            feature("opened", 0),
        ];
        let activity: Activity = Arc::new(Mutex::new(vec![
            idle_for("amf-quiet", 120),
            idle_for("amf-quieter", 360),
            idle_for("amf-busy", 0),
            idle_for("amf-opened", 360),
        ]));
        let live = Arc::new(Mutex::new(
            ["amf-quiet", "amf-quieter", "amf-busy", "amf-opened"]
                .into_iter()
                .map(str::to_string)
                .collect(),
        ));
        let (tmux, killed) = tmux(activity.clone(), live);
        let app = App::new_for_test(
            store(features),
            Box::new(tmux),
            Box::new(MockWorktreeOps::new()),
        );
        Fixture {
            gui: Mutex::new(GuiHandle::from_app(app)),
            activity,
            killed,
        }
    }

    fn observation(view: &DormancyView, id: &str) -> DormantObservation {
        view.features
            .iter()
            .find(|row| row.observation.target.feature_id == id)
            .unwrap_or_else(|| panic!("{id} should be listed"))
            .observation
            .clone()
    }

    fn outcome(results: &[DormancyStopResult], id: &str) -> DormancyStopOutcome {
        results
            .iter()
            .find(|result| result.target.feature_id == id)
            .unwrap()
            .outcome
            .clone()
    }

    fn refusal(outcome: &DormancyStopOutcome) -> Option<DormancyRefusal> {
        match outcome {
            DormancyStopOutcome::Refused { reason, .. } => Some(*reason),
            _ => None,
        }
    }

    #[test]
    fn lists_only_idle_and_unattended_features_longest_idle_first_with_why() {
        let mut f = fixture();

        let view = load(f.gui()).unwrap();

        assert!(view.enabled);
        assert_eq!((view.idle_minutes, view.unattended_hours), (60, 4));
        let ids: Vec<_> = view
            .features
            .iter()
            .map(|row| row.observation.target.feature_id.as_str())
            .collect();
        assert_eq!(ids, vec!["quieter", "quiet"]);
        let quieter = &view.features[0];
        assert!(quieter.idle_secs >= 6 * 3600);
        assert!(quieter.unattended_secs >= 20 * 3600);
        assert_eq!(quieter.observation.tmux_session, "amf-quieter");
        assert_eq!(quieter.observation.feature_name, "quieter");
        assert!(quieter.is_worktree);
        // Listing is read-only.
        assert!(f.killed().is_empty());
    }

    #[test]
    fn switched_off_thresholds_list_nothing_and_say_so() {
        let mut f = fixture();
        f.gui().app_for_workflow().config.dormant_idle_minutes = 0;

        let view = load(f.gui()).unwrap();

        assert!(!view.enabled);
        assert!(view.features.is_empty());
    }

    #[test]
    fn stops_the_confirmed_selection_through_the_shared_stop() {
        let mut f = fixture();
        let view = load(f.gui()).unwrap();

        let results = stop(
            &f.gui,
            vec![observation(&view, "quiet"), observation(&view, "quieter")],
        )
        .unwrap();

        assert_eq!(results.len(), 2);
        for id in ["quiet", "quieter"] {
            assert!(
                matches!(
                    outcome(&results, id),
                    DormancyStopOutcome::Stopped { editors: Some(_) }
                ),
                "{id}: {:?}",
                outcome(&results, id)
            );
            assert_eq!(f.status(id), ProjectStatus::Stopped);
        }
        assert_eq!(f.killed(), vec!["amf-quiet", "amf-quieter"]);
        // The unselected and non-dormant features are untouched.
        assert_eq!(f.status("busy"), ProjectStatus::Idle);
        assert_eq!(f.status("opened"), ProjectStatus::Idle);
    }

    #[test]
    fn a_repeated_stop_is_refused_rather_than_stopping_twice() {
        let mut f = fixture();
        let view = load(f.gui()).unwrap();
        let quiet = observation(&view, "quiet");

        let first = stop(&f.gui, vec![quiet.clone(), quiet.clone()]).unwrap();
        assert!(matches!(
            first[0].outcome,
            DormancyStopOutcome::Stopped { .. }
        ));
        assert_eq!(refusal(&first[1].outcome), Some(DormancyRefusal::Duplicate));

        let again = stop(&f.gui, vec![quiet]).unwrap();
        assert_eq!(
            refusal(&again[0].outcome),
            Some(DormancyRefusal::AlreadyStopped)
        );
        assert_eq!(f.killed(), vec!["amf-quiet"]);
    }

    #[test]
    fn stale_selections_are_refused_with_their_reason_and_never_stopped() {
        let mut f = fixture();
        let view = load(f.gui()).unwrap();
        let quiet = observation(&view, "quiet");
        let quieter = observation(&view, "quieter");

        // quiet's agent prints something; quieter is opened elsewhere.
        f.activity.lock().unwrap()[0] = idle_for("amf-quiet", 0);
        {
            let app = f.gui().app_for_workflow();
            let (pi, fi) = app.store.locate_feature_by_id(None, "quieter").unwrap();
            app.store.projects[pi].features[fi].touch();
        }

        let results = stop(&f.gui, vec![quiet, quieter]).unwrap();

        assert_eq!(
            refusal(&outcome(&results, "quiet")),
            Some(DormancyRefusal::Output)
        );
        assert_eq!(
            refusal(&outcome(&results, "quieter")),
            Some(DormancyRefusal::Opened)
        );
        assert!(f.killed().is_empty());
    }

    #[test]
    fn deleted_restarted_and_no_longer_dormant_targets_are_refused() {
        let mut f = fixture();
        let view = load(f.gui()).unwrap();
        let quiet = observation(&view, "quiet");
        let quieter = observation(&view, "quieter");
        let mut gone = quiet.clone();
        gone.target.feature_id = "deleted-meanwhile".to_string();
        let mut renamed = quieter.clone();
        renamed.tmux_session = "amf-some-older-session".to_string();

        // Thresholds tightened after listing: quiet (idle 2h) no longer qualifies.
        f.gui().app_for_workflow().config.dormant_idle_minutes = 5 * 60;

        let results = stop(&f.gui, vec![gone, renamed, quiet]).unwrap();

        assert_eq!(refusal(&results[0].outcome), Some(DormancyRefusal::Deleted));
        assert_eq!(
            refusal(&results[1].outcome),
            Some(DormancyRefusal::Restarted)
        );
        assert_eq!(
            refusal(&results[2].outcome),
            Some(DormancyRefusal::NoLongerDormant)
        );
        assert!(f.killed().is_empty());
    }

    #[test]
    fn a_stopped_feature_or_vanished_session_is_refused_and_an_empty_request_rejected() {
        let mut f = fixture();
        let view = load(f.gui()).unwrap();
        let quiet = observation(&view, "quiet");
        let quieter = observation(&view, "quieter");
        {
            let app = f.gui().app_for_workflow();
            let (pi, fi) = app.store.locate_feature_by_id(None, "quiet").unwrap();
            app.store.projects[pi].features[fi].status = ProjectStatus::Stopped;
        }
        // quieter's tmux session vanished while AMF still records it running
        // (a crash or a tmux server restart).
        let (tmux, _) = tmux(f.activity.clone(), Arc::new(Mutex::new(HashSet::new())));
        f.gui().app_for_workflow().tmux = Box::new(tmux);

        let results = stop(&f.gui, vec![quiet, quieter]).unwrap();

        assert_eq!(
            refusal(&outcome(&results, "quiet")),
            Some(DormancyRefusal::AlreadyStopped)
        );
        assert_eq!(
            refusal(&outcome(&results, "quieter")),
            Some(DormancyRefusal::SessionGone)
        );
        assert_eq!(
            stop(&f.gui, Vec::new()).unwrap_err().kind,
            GuiErrorKind::Conflict
        );
    }

    #[test]
    fn a_shared_live_tmux_session_is_refused() {
        let mut f = fixture();
        let view = load(f.gui()).unwrap();
        let quiet = observation(&view, "quiet");
        {
            let app = f.gui().app_for_workflow();
            let (pi, fi) = app.store.locate_feature_by_id(None, "opened").unwrap();
            app.store.projects[pi].features[fi].tmux_session = "amf-quiet".to_string();
        }

        let results = stop(&f.gui, vec![quiet]).unwrap();

        assert_eq!(
            refusal(&results[0].outcome),
            Some(DormancyRefusal::SharedSession)
        );
        assert!(f.killed().is_empty());
    }

    /// A stand-in VS Code window: Bash linked as `code`, with a child the way
    /// a real window holds a language server.
    fn fake_editor(dir: &Path, workdir: &Path) -> crate::resources::test_support::TestChild {
        let fake = dir.join("code");
        std::os::unix::fs::symlink("/bin/bash", &fake).unwrap();
        std::process::Command::new(&fake)
            .args([
                "-c".as_ref(),
                "sleep 60 & wait".as_ref(),
                "--new-window".as_ref(),
                workdir.as_os_str(),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map(crate::resources::test_support::TestChild::new)
            .unwrap()
    }

    #[test]
    fn editor_cleanup_reports_killed_and_skipped_windows_and_leaves_unowned_ones() {
        let tmp = tempfile::TempDir::new().unwrap();
        let owned_dir = tmp.path().join("owned");
        let shared_dir = tmp.path().join("shared");
        std::fs::create_dir_all(owned_dir.join("worktree")).unwrap();
        std::fs::create_dir_all(shared_dir.join("worktree")).unwrap();
        let mut owned = fake_editor(&owned_dir, &owned_dir.join("worktree"));
        let mut unowned = fake_editor(&shared_dir, &shared_dir.join("worktree"));
        std::thread::sleep(std::time::Duration::from_millis(300));

        let mut f = fixture();
        {
            let app = f.gui().app_for_workflow();
            for (id, dir) in [("quiet", &owned_dir), ("quieter", &shared_dir)] {
                let (pi, fi) = app.store.locate_feature_by_id(None, id).unwrap();
                app.store.projects[pi].features[fi].workdir = dir.join("worktree");
            }
        }
        let _db_file = f.attach_db();
        {
            let db = f.gui().app_for_workflow().db.as_ref().unwrap();
            db.record_launched_editor(
                "quiet",
                None,
                crate::db::editors::EditorKind::Vscode,
                owned.id() as i64,
                &owned_dir.join("worktree"),
                true,
                "code --new-window",
            )
            .unwrap();
            // Handed to a window the user opened: never AMF's to close.
            db.record_launched_editor(
                "quieter",
                None,
                crate::db::editors::EditorKind::Vscode,
                unowned.id() as i64,
                &shared_dir.join("worktree"),
                false,
                "code",
            )
            .unwrap();
        }
        let view = load(f.gui()).unwrap();
        assert_eq!(view.features.len(), 2);
        assert!(view.features.iter().all(|row| row.editor_alive));

        let results = stop(
            &f.gui,
            vec![observation(&view, "quiet"), observation(&view, "quieter")],
        )
        .unwrap();
        let _ = owned.wait();

        let DormancyStopOutcome::Stopped {
            editors: Some(closed),
        } = outcome(&results, "quiet")
        else {
            panic!("{:?}", outcome(&results, "quiet"));
        };
        assert_eq!(closed.killed.len(), 1);
        assert!(closed.killed[0].processes >= 1);
        assert!(closed.summary.unwrap().contains("closed 1 editor"));

        let DormancyStopOutcome::Stopped {
            editors: Some(left),
        } = outcome(&results, "quieter")
        else {
            panic!("{:?}", outcome(&results, "quieter"));
        };
        assert!(left.killed.is_empty());
        assert_eq!(left.skipped[0].reason, "AMF did not open this window");
        assert!(left.skipped[0].deliberate);
        assert!(crate::resources::procs::pid_alive(unowned.id() as i64));

        let _ = unowned.kill();
        let _ = unowned.wait();
    }

    #[test]
    fn editor_cleanup_switched_off_examines_no_editor() {
        let mut f = fixture();
        f.gui().app_for_workflow().config.kill_editor_on_stop = false;
        let view = load(f.gui()).unwrap();
        assert!(!view.kill_editor_on_stop);

        let results = stop(&f.gui, vec![observation(&view, "quiet")]).unwrap();

        assert!(matches!(
            results[0].outcome,
            DormancyStopOutcome::Stopped { editors: None }
        ));
    }

    #[test]
    fn opening_a_session_counts_as_attention_and_persists() {
        let mut f = fixture();
        let db_file = f.attach_db();
        let before = load(f.gui()).unwrap();
        assert!(
            before
                .features
                .iter()
                .any(|row| row.observation.feature_name == "quiet")
        );

        note_opened(
            f.gui(),
            &SessionTarget {
                project_id: PROJECT.to_string(),
                feature_id: "quiet".to_string(),
                session_id: "any".to_string(),
            },
        )
        .unwrap();

        let after = load(f.gui()).unwrap();
        assert!(
            after
                .features
                .iter()
                .all(|row| row.observation.feature_name != "quiet")
        );
        let persisted = crate::db::AmfDb::open(db_file.path())
            .unwrap()
            .load_store()
            .unwrap();
        let (pi, fi) = persisted.locate_feature_by_id(None, "quiet").unwrap();
        assert!(
            Utc::now() - persisted.projects[pi].features[fi].last_accessed
                < chrono::Duration::minutes(1)
        );
        // An unknown target is left alone rather than failing the attach.
        note_opened(
            f.gui(),
            &SessionTarget {
                project_id: PROJECT.to_string(),
                feature_id: "gone".to_string(),
                session_id: "any".to_string(),
            },
        )
        .unwrap();
    }
}
