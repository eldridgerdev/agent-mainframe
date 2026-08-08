//! Finding features that are holding resources without being worked on.
//!
//! Dormancy is deliberately an **and** of two independent signals:
//!
//! - the agent has produced no output for N minutes (tmux's own
//!   `window_activity`, which survives AMF restarts), and
//! - the feature has not been opened for X hours (`Feature::last_accessed`).
//!
//! Either alone is a bad signal. An agent can sit quiet for an hour while you
//! read its last answer, and a feature you have not clicked into can still be
//! grinding through a long run. Requiring both means a listed feature is one
//! nobody is watching *and* nothing is happening in.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};

use crate::app::state::DormantViewState;
use crate::app::{App, AppMode, Selection};
use crate::project::{ProjectStatus, ProjectStore};
use crate::resources::procs;

/// A feature that is idle, unattended, and still holding things.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DormantFeature {
    pub pi: usize,
    pub fi: usize,
    pub project_name: String,
    pub feature_name: String,
    /// Since the agent last produced output.
    pub idle: Duration,
    /// Since the feature was last opened in AMF.
    pub unattended: Duration,
    pub workdir: PathBuf,
    /// Only a worktree can be reclaimed by deleting it; the repo dir cannot.
    pub is_worktree: bool,
    /// A tracked editor of this feature is still running.
    pub editor_alive: bool,
}

/// The dormancy test itself.
///
/// `last_activity` is `None` when tmux reported nothing for the feature's
/// windows. That is "cannot tell", not "idle forever": a feature is never
/// listed on a missing signal.
pub fn is_dormant(
    now: DateTime<Utc>,
    last_activity: Option<DateTime<Utc>>,
    last_accessed: DateTime<Utc>,
    idle_threshold: Duration,
    unattended_threshold: Duration,
) -> bool {
    let Some(last_activity) = last_activity else {
        return false;
    };
    elapsed(now, last_activity) > idle_threshold && elapsed(now, last_accessed) > unattended_threshold
}

/// Wall-clock gap, clamped at zero: a timestamp in the future (clock skew, a
/// tmux server on another clock) reads as "just now" rather than a huge age.
fn elapsed(now: DateTime<Utc>, then: DateTime<Utc>) -> Duration {
    (now - then).to_std().unwrap_or(Duration::ZERO)
}

/// Most recent activity across a feature's tmux windows, keyed by session name.
///
/// The newest window wins: a feature with a quiet agent but a busy terminal is
/// still being used.
pub fn latest_activity_by_session(
    activity: &[(String, String, i64)],
) -> HashMap<String, DateTime<Utc>> {
    let mut latest: HashMap<String, DateTime<Utc>> = HashMap::new();
    for (session, _window, unix) in activity {
        let Some(when) = Utc.timestamp_opt(*unix, 0).single() else {
            continue;
        };
        latest
            .entry(session.clone())
            .and_modify(|current| {
                if when > *current {
                    *current = when;
                }
            })
            .or_insert(when);
    }
    latest
}

/// Walk the store for dormant features, newest-idle last.
pub fn dormant_features(
    store: &ProjectStore,
    activity: &HashMap<String, DateTime<Utc>>,
    now: DateTime<Utc>,
    idle_threshold: Duration,
    unattended_threshold: Duration,
    editor_alive: &dyn Fn(&str) -> bool,
) -> Vec<DormantFeature> {
    let mut dormant = Vec::new();
    for (pi, project) in store.projects.iter().enumerate() {
        for (fi, feature) in project.features.iter().enumerate() {
            // A stopped feature has no agent to be idle. Whatever it still
            // holds on disk is `amf doctor`'s department.
            if feature.status == ProjectStatus::Stopped {
                continue;
            }
            let last_activity = activity.get(&feature.tmux_session).copied();
            if !is_dormant(
                now,
                last_activity,
                feature.last_accessed,
                idle_threshold,
                unattended_threshold,
            ) {
                continue;
            }
            dormant.push(DormantFeature {
                pi,
                fi,
                project_name: project.name.clone(),
                feature_name: feature.name.clone(),
                idle: last_activity.map(|at| elapsed(now, at)).unwrap_or_default(),
                unattended: elapsed(now, feature.last_accessed),
                workdir: feature.workdir.clone(),
                is_worktree: feature.is_worktree,
                editor_alive: editor_alive(&feature.id),
            });
        }
    }
    // Longest-idle first: the most reclaimable is the one to act on.
    dormant.sort_by_key(|feature| std::cmp::Reverse(feature.idle));
    dormant
}

impl App {
    /// Dormant features right now, or an empty list when dormancy is switched
    /// off in config.
    pub fn find_dormant_features(&self) -> Vec<DormantFeature> {
        let Some((idle_threshold, unattended_threshold)) = self.config.dormant_thresholds() else {
            return Vec::new();
        };
        let activity = latest_activity_by_session(&self.tmux.window_activity());
        let db = self.db.as_ref();
        let editor_alive = |feature_id: &str| -> bool {
            db.and_then(|db| db.launched_editors_for_feature(feature_id).ok())
                .is_some_and(|editors| {
                    editors
                        .iter()
                        .any(|editor| procs::pid_alive(editor.pid))
                })
        };
        dormant_features(
            &self.store,
            &activity,
            Utc::now(),
            idle_threshold,
            unattended_threshold,
            &editor_alive,
        )
    }

    /// Open the dormant-features overlay. Opens even when nothing is dormant —
    /// the empty state names the thresholds, which is the answer to "why is
    /// this empty?".
    pub fn open_dormant_view(&mut self) {
        let features = self.find_dormant_features();
        self.mode = AppMode::Dormant(DormantViewState {
            features,
            selected: 0,
            message: None,
        });
    }

    /// Re-run detection, keeping the cursor in range.
    pub fn refresh_dormant_view(&mut self) {
        let features = self.find_dormant_features();
        if let AppMode::Dormant(state) = &mut self.mode {
            state.features = features;
            state.clamp_selection();
        }
    }

    fn selected_dormant(&self) -> Option<DormantFeature> {
        match &self.mode {
            AppMode::Dormant(state) => state.selected_feature().cloned(),
            _ => None,
        }
    }

    fn set_dormant_message(&mut self, message: impl Into<String>) {
        if let AppMode::Dormant(state) = &mut self.mode {
            state.message = Some(message.into());
        }
    }

    /// Stop the selected feature's tmux session (and, per config, its editor).
    pub fn dormant_stop_selected(&mut self) -> anyhow::Result<()> {
        let Some(target) = self.selected_dormant() else {
            return Ok(());
        };
        self.do_stop_feature(target.pi, target.fi)?;
        // `do_stop_feature` writes the outcome to the status line; carry it
        // into the overlay, which is what the user is actually looking at.
        let outcome = self
            .message
            .clone()
            .unwrap_or_else(|| format!("Stopped '{}'", target.feature_name));
        self.refresh_dormant_view();
        self.set_dormant_message(outcome);
        Ok(())
    }

    /// Close the editor AMF opened for the selected feature, leaving the agent
    /// running — the language server is usually the larger half.
    pub fn dormant_kill_editor_selected(&mut self) {
        let Some(target) = self.selected_dormant() else {
            return;
        };
        let feature_id = self
            .store
            .projects
            .get(target.pi)
            .and_then(|project| project.features.get(target.fi))
            .map(|feature| feature.id.clone());
        let Some(feature_id) = feature_id else {
            return;
        };

        let report = self.kill_tracked_editors(&feature_id);
        let outcome = report
            .summary()
            .unwrap_or_else(|| format!("No editor to close for '{}'", target.feature_name));
        self.refresh_dormant_view();
        self.set_dormant_message(outcome);
    }

    /// Jump into the selected feature's session, leaving the overlay.
    pub fn dormant_jump_selected(&mut self) -> anyhow::Result<()> {
        let Some(target) = self.selected_dormant() else {
            return Ok(());
        };
        self.selection = Selection::Feature(target.pi, target.fi);
        self.mode = AppMode::Normal;
        self.enter_view()
    }

    /// Hand the selected feature to the ordinary delete-feature confirmation,
    /// which is what actually removes the worktree.
    pub fn dormant_delete_selected(&mut self) {
        let Some(target) = self.selected_dormant() else {
            return;
        };
        self.selection = Selection::Feature(target.pi, target.fi);
        self.mode = AppMode::DeletingFeature(target.project_name, target.feature_name);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IDLE: Duration = Duration::from_secs(60 * 60);
    const UNATTENDED: Duration = Duration::from_secs(4 * 3600);

    fn ago(now: DateTime<Utc>, secs: i64) -> DateTime<Utc> {
        now - chrono::Duration::seconds(secs)
    }

    #[test]
    fn dormant_needs_both_halves() {
        let now = Utc::now();
        let long_idle = ago(now, 2 * 3600);
        let long_unattended = ago(now, 8 * 3600);
        let just_now = now;

        assert!(is_dormant(
            now,
            Some(long_idle),
            long_unattended,
            IDLE,
            UNATTENDED
        ));
        // Idle agent, but the feature was just opened: the user is reading it.
        assert!(!is_dormant(now, Some(long_idle), just_now, IDLE, UNATTENDED));
        // Untouched for days, but the agent is producing output right now.
        assert!(!is_dormant(
            now,
            Some(just_now),
            long_unattended,
            IDLE,
            UNATTENDED
        ));
        assert!(!is_dormant(now, Some(just_now), just_now, IDLE, UNATTENDED));
    }

    #[test]
    fn thresholds_are_exclusive_at_the_boundary() {
        let now = Utc::now();
        // Exactly at both thresholds is not yet dormant.
        assert!(!is_dormant(
            now,
            Some(ago(now, 3600)),
            ago(now, 4 * 3600),
            IDLE,
            UNATTENDED
        ));
        // One second past both is.
        assert!(is_dormant(
            now,
            Some(ago(now, 3601)),
            ago(now, 4 * 3600 + 1),
            IDLE,
            UNATTENDED
        ));
    }

    #[test]
    fn a_missing_activity_signal_is_never_dormant() {
        let now = Utc::now();
        assert!(!is_dormant(now, None, ago(now, 100 * 3600), IDLE, UNATTENDED));
    }

    #[test]
    fn future_timestamps_read_as_just_now() {
        let now = Utc::now();
        let future = now + chrono::Duration::hours(1);
        assert!(!is_dormant(now, Some(future), future, IDLE, UNATTENDED));
    }

    #[test]
    fn walks_the_store_skipping_stopped_and_busy_features() {
        use crate::project::{Feature, Project, VibeMode};

        let now = Utc::now();
        let make = |name: &str, status: ProjectStatus, accessed: DateTime<Utc>| Feature {
            id: format!("feat-{name}"),
            name: name.to_string(),
            branch: name.to_string(),
            workdir: PathBuf::from("/tmp").join(name),
            is_worktree: true,
            tmux_session: format!("amf-{name}"),
            sessions: Vec::new(),
            collapsed: true,
            mode: VibeMode::default(),
            review: false,
            plan_mode: false,
            agent: Default::default(),
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: true,
            status,
            created_at: now,
            last_accessed: accessed,
            summary: None,
            summary_updated_at: None,
            nickname: None,
            triage_source: None,
        };

        let store = ProjectStore {
            version: 1,
            projects: vec![Project {
                id: "p".into(),
                name: "proj".into(),
                repo: PathBuf::from("/tmp/proj"),
                collapsed: false,
                features: vec![
                    make("quiet", ProjectStatus::Idle, ago(now, 10 * 3600)),
                    make("quieter", ProjectStatus::Idle, ago(now, 20 * 3600)),
                    make("busy", ProjectStatus::Active, ago(now, 20 * 3600)),
                    make("stopped", ProjectStatus::Stopped, ago(now, 20 * 3600)),
                ],
                created_at: now,
                preferred_agent: Default::default(),
                is_git: true,
            }],
            session_bookmarks: Vec::new(),
            available_harnesses: Vec::new(),
            prompt_templates: Vec::new(),
            extra: Default::default(),
        };

        let activity = HashMap::from([
            ("amf-quiet".to_string(), ago(now, 2 * 3600)),
            ("amf-quieter".to_string(), ago(now, 6 * 3600)),
            ("amf-busy".to_string(), now),
            ("amf-stopped".to_string(), ago(now, 40 * 3600)),
        ]);

        let dormant = dormant_features(&store, &activity, now, IDLE, UNATTENDED, &|_| false);

        let names: Vec<_> = dormant.iter().map(|d| d.feature_name.as_str()).collect();
        // Longest-idle first; the busy and stopped features are absent.
        assert_eq!(names, vec!["quieter", "quiet"]);
        assert_eq!(dormant[0].fi, 1);
        assert!(dormant[0].idle >= Duration::from_secs(6 * 3600));
        assert!(dormant[0].unattended >= Duration::from_secs(20 * 3600));
    }

    #[test]
    fn latest_activity_takes_the_busiest_window_in_a_session() {
        let activity = vec![
            ("amf-a".to_string(), "claude".to_string(), 1_000),
            ("amf-a".to_string(), "terminal".to_string(), 5_000),
            ("amf-b".to_string(), "claude".to_string(), 2_000),
        ];
        let latest = latest_activity_by_session(&activity);
        assert_eq!(latest["amf-a"], Utc.timestamp_opt(5_000, 0).single().unwrap());
        assert_eq!(latest["amf-b"], Utc.timestamp_opt(2_000, 0).single().unwrap());
    }
}
