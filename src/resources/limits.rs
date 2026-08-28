//! How many agent harnesses are running right now, across every project.
//!
//! The store is machine-global, so this census is too: the concurrency limit
//! is about what the host is carrying, not what one project is doing. Only
//! agent-harness sessions count — terminals, editors, and TODOs sessions cost
//! the machine little and are not what the gate is protecting against.
//!
//! "Running" means a process is alive in the session's tmux pane, not merely
//! that the pane exists — see [`LiveHarnesses`].

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::project::{ProjectStatus, ProjectStore, SessionKind};
use crate::resources::procs::{self, ProcInfo};
use crate::traits::TmuxOps;

/// In-flight headless harness runs (plan interviews, AI review, final review).
/// They are invisible to tmux but cost the same memory as an interactive
/// harness, so they count toward the limit while they run.
static IN_FLIGHT_HEADLESS: AtomicUsize = AtomicUsize::new(0);

/// Counts one headless run for as long as it is alive.
///
/// The count is released in `Drop`, so an early `?`, a cancelled run, or a
/// panic inside the run cannot leak a slot — which is the whole reason the
/// accounting is a guard rather than a matched pair of calls.
#[derive(Debug)]
pub struct HeadlessLease {
    counter: &'static AtomicUsize,
}

impl HeadlessLease {
    pub fn acquire() -> Self {
        Self::acquire_on(&IN_FLIGHT_HEADLESS)
    }

    /// Count against a specific counter. Only the process-wide
    /// [`IN_FLIGHT_HEADLESS`] matters in production; tests use their own so
    /// their assertions cannot be disturbed by a real headless run finishing
    /// on some other test's background thread.
    fn acquire_on(counter: &'static AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self { counter }
    }
}

impl Drop for HeadlessLease {
    fn drop(&mut self) {
        // `fetch_update` rather than `fetch_sub` so a hypothetical unbalanced
        // release can never wrap the counter around to usize::MAX and lock the
        // gate on forever.
        let _ = self
            .counter
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |current| {
                Some(current.saturating_sub(1))
            });
    }
}

/// How many headless harness runs are in flight right now.
pub fn in_flight_headless_runs() -> usize {
    IN_FLIGHT_HEADLESS.load(Ordering::SeqCst)
}

/// Serializes tests that assert on the process-global headless count. Without
/// it, two lease tests running in parallel see each other's leases and read a
/// moving baseline.
#[cfg(test)]
pub(crate) fn lock_lease_tests() -> std::sync::MutexGuard<'static, ()> {
    static LEASE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A test that panics on purpose (the cancellation case) poisons the lock;
    // the guarded data is a unit, so recovering is safe.
    LEASE_TEST_LOCK
        .lock()
        .unwrap_or_else(|err| err.into_inner())
}

/// Wait for the global count to come back to `target`.
///
/// The lock above keeps lease *tests* from colliding, but any test that runs a
/// real headless pass holds a lease of its own for as long as it runs. Polling
/// lets those settle instead of racing them.
#[cfg(test)]
pub(crate) fn wait_for_in_flight(target: usize) -> usize {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let current = in_flight_headless_runs();
        if current == target || std::time::Instant::now() >= deadline {
            return current;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The tmux windows that have something running in them right now, grouped by
/// session.
///
/// A feature's stored session list is a record of what was *created*, not what
/// is *running*, and neither is the window list: feature startup creates a
/// window for every saved session even when
/// `max_agent_autostart_sessions` stops it short of launching the harness in
/// them, and an agent the user quit leaves its window behind at a shell
/// prompt. Both look identical to `list-windows`.
///
/// What separates them is the pane's process tree. tmux starts a shell in each
/// pane and reports its pid; AMF launches a harness as a child of that shell
/// (`sh /tmp/amf-agent-launch-*.sh`, which in turn runs the agent), so the
/// shell has a child for exactly as long as the harness lives. A pane sitting
/// at a prompt — never launched, or exited — has none.
///
/// The test is deliberately "any child" rather than "a process that looks like
/// a harness": harness binaries are not reliably identifiable from their argv
/// (Claude Code execs a version-numbered path), and a user who quits and
/// re-runs the agent by hand is still running an agent. Something else being
/// run in an agent window counts too, which is the safe direction to be wrong
/// in for a gate that only ever warns.
#[derive(Debug, Default, Clone)]
pub struct LiveHarnesses {
    windows: HashMap<String, HashSet<String>>,
}

impl LiveHarnesses {
    /// Ask tmux for every pane on the server, then ask `ps` which of those
    /// panes is running something: two process spawns, regardless of how many
    /// sessions exist.
    pub fn probe(tmux: &dyn TmuxOps) -> Self {
        Self::from_census(&tmux.list_panes(), &procs::list_processes())
    }

    /// Decide which windows are busy from an already-gathered census.
    ///
    /// An empty process list means `ps` could not be reached, not that the
    /// machine is idle — there is no signal to filter with, so every live pane
    /// counts. That is the pre-existing over-counting answer, and the right
    /// direction to fail in when the alternative is a gate that silently never
    /// fires.
    pub fn from_census(panes: &[(String, String, i64)], processes: &[ProcInfo]) -> Self {
        let no_process_signal = processes.is_empty();
        let parents: HashSet<i64> = processes.iter().map(|proc| proc.ppid).collect();

        let mut windows: HashMap<String, HashSet<String>> = HashMap::new();
        for (session, window, pane_pid) in panes {
            if no_process_signal || parents.contains(pane_pid) {
                windows
                    .entry(session.clone())
                    .or_default()
                    .insert(window.clone());
            }
        }
        Self { windows }
    }

    /// Whether this window has a live process in it.
    pub fn is_running(&self, session: &str, window: &str) -> bool {
        self.windows
            .get(session)
            .is_some_and(|names| names.contains(window))
    }

    /// Test helper: these windows are running something.
    #[cfg(test)]
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let mut windows: HashMap<String, HashSet<String>> = HashMap::new();
        for (session, window) in pairs {
            windows
                .entry(session.to_string())
                .or_default()
                .insert(window.to_string());
        }
        Self { windows }
    }
}

/// One running harness, identified well enough to name it in a warning dialog
/// or an `amf doctor` report.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveHarness {
    pub project_name: String,
    pub feature_id: String,
    pub feature_name: String,
    pub session_id: String,
    pub session_label: String,
    pub kind: SessionKind,
}

/// Every agent-harness session that is actually running.
pub fn active_harness_sessions(store: &ProjectStore, live: &LiveHarnesses) -> Vec<ActiveHarness> {
    let mut active = Vec::new();
    for project in &store.projects {
        for feature in &project.features {
            // A stopped feature has no tmux session at all; skip it before
            // looking at its saved sessions.
            if feature.status == ProjectStatus::Stopped {
                continue;
            }
            for session in &feature.sessions {
                if !session.kind.is_agent_harness()
                    || !live.is_running(&feature.tmux_session, &session.tmux_window)
                {
                    continue;
                }
                active.push(ActiveHarness {
                    project_name: project.name.clone(),
                    feature_id: feature.id.clone(),
                    feature_name: feature.name.clone(),
                    session_id: session.id.clone(),
                    session_label: session.label.clone(),
                    kind: session.kind.clone(),
                });
            }
        }
    }
    active
}

/// Count of running agent-harness sessions across all projects.
pub fn count_active_harness_sessions(store: &ProjectStore, live: &LiveHarnesses) -> usize {
    active_harness_sessions(store, live).len()
}

/// Upper bound on running harnesses, from the store alone.
///
/// Every session counted by [`count_active_harness_sessions`] is also counted
/// here — the live-pane check can only ever remove sessions — so a caller
/// whose bound is already under the limit can skip asking tmux entirely.
pub fn max_possible_harness_sessions(store: &ProjectStore) -> usize {
    store
        .projects
        .iter()
        .flat_map(|project| project.features.iter())
        .filter(|feature| feature.status != ProjectStatus::Stopped)
        .flat_map(|feature| feature.sessions.iter())
        .filter(|session| session.kind.is_agent_harness())
        .count()
}

/// Everything the concurrency limit is measured against: interactive harness
/// sessions plus the headless runs currently executing.
pub fn total_active_agents(store: &ProjectStore, live: &LiveHarnesses) -> usize {
    count_active_harness_sessions(store, live) + in_flight_headless_runs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Feature, FeatureSession, Project, VibeMode};
    use chrono::Utc;
    use std::path::PathBuf;

    fn session(id: &str, kind: SessionKind, window: &str) -> FeatureSession {
        FeatureSession {
            id: id.to_string(),
            kind,
            label: id.to_string(),
            tmux_window: window.to_string(),
            claude_session_id: None,
            todo_reference: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
        }
    }

    fn feature(name: &str, status: ProjectStatus, sessions: Vec<FeatureSession>) -> Feature {
        Feature {
            id: format!("feat-{name}"),
            name: name.to_string(),
            branch: name.to_string(),
            workdir: PathBuf::from("/tmp").join(name),
            is_worktree: true,
            tmux_session: format!("amf-{name}"),
            sessions,
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
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            summary: None,
            summary_updated_at: None,
            nickname: None,
            selected_plan_path: None,
            triage_source: None,
        }
    }

    fn store(projects: Vec<Project>) -> ProjectStore {
        ProjectStore {
            version: 1,
            projects,
            session_bookmarks: Vec::new(),
            available_harnesses: Vec::new(),
            prompt_templates: Vec::new(),
            extra: Default::default(),
        }
    }

    fn project(name: &str, features: Vec<Feature>) -> Project {
        Project {
            id: format!("proj-{name}"),
            name: name.to_string(),
            repo: PathBuf::from("/tmp").join(name),
            collapsed: false,
            features,
            created_at: Utc::now(),
            preferred_agent: Default::default(),
            is_git: true,
        }
    }

    #[test]
    fn counts_only_harness_sessions() {
        let feat = feature(
            "alpha",
            ProjectStatus::Active,
            vec![
                session("s1", SessionKind::Claude, "claude"),
                session("s2", SessionKind::Codex, "codex"),
                session("s3", SessionKind::Terminal, "terminal"),
                session("s4", SessionKind::Nvim, "nvim"),
                session("s5", SessionKind::Vscode, "vscode"),
                session("s6", SessionKind::Custom, "dev-server"),
                session("s7", SessionKind::Todos, "todos"),
            ],
        );
        let live = LiveHarnesses::from_pairs([
            ("amf-alpha", "claude"),
            ("amf-alpha", "codex"),
            ("amf-alpha", "terminal"),
            ("amf-alpha", "nvim"),
            ("amf-alpha", "vscode"),
            ("amf-alpha", "dev-server"),
            ("amf-alpha", "todos"),
        ]);
        let store = store(vec![project("p", vec![feat])]);

        assert_eq!(count_active_harness_sessions(&store, &live), 2);
    }

    #[test]
    fn counts_across_projects_and_features() {
        let store = store(vec![
            project(
                "one",
                vec![
                    feature(
                        "alpha",
                        ProjectStatus::Active,
                        vec![session("s1", SessionKind::Claude, "claude")],
                    ),
                    feature(
                        "beta",
                        ProjectStatus::Idle,
                        vec![session("s2", SessionKind::Opencode, "opencode")],
                    ),
                ],
            ),
            project(
                "two",
                vec![feature(
                    "gamma",
                    ProjectStatus::Active,
                    vec![session("s3", SessionKind::Pi, "pi")],
                )],
            ),
        ]);
        let live = LiveHarnesses::from_pairs([
            ("amf-alpha", "claude"),
            ("amf-beta", "opencode"),
            ("amf-gamma", "pi"),
        ]);

        assert_eq!(count_active_harness_sessions(&store, &live), 3);
    }

    #[test]
    fn skips_stopped_features() {
        let store = store(vec![project(
            "p",
            vec![feature(
                "alpha",
                ProjectStatus::Stopped,
                vec![session("s1", SessionKind::Claude, "claude")],
            )],
        )]);
        // Even if a stale window were reported, a stopped feature never counts.
        let live = LiveHarnesses::from_pairs([("amf-alpha", "claude")]);

        assert_eq!(count_active_harness_sessions(&store, &live), 0);
    }

    #[test]
    fn skips_saved_sessions_whose_harness_is_not_running() {
        // Second harness pane was recreated but left at a shell prompt, or was
        // never recreated at all: only panes with a process count.
        let store = store(vec![project(
            "p",
            vec![feature(
                "alpha",
                ProjectStatus::Active,
                vec![
                    session("s1", SessionKind::Claude, "claude"),
                    session("s2", SessionKind::Claude, "claude-2"),
                ],
            )],
        )]);
        let live = LiveHarnesses::from_pairs([("amf-alpha", "claude")]);

        assert_eq!(count_active_harness_sessions(&store, &live), 1);
    }

    fn proc(pid: i64, ppid: i64, args: &str) -> ProcInfo {
        ProcInfo {
            pid,
            ppid,
            args: args.to_string(),
        }
    }

    /// The census's whole job: three windows that all exist, only one of which
    /// has a harness in it. The other two are the two ways a window outlives
    /// its agent — created past `max_agent_autostart_sessions` and so never
    /// launched, and launched but since exited back to the prompt.
    #[test]
    fn an_idle_pane_is_not_a_running_harness() {
        let panes = vec![
            ("amf-alpha".to_string(), "claude".to_string(), 100),
            ("amf-alpha".to_string(), "claude-2".to_string(), 200),
            ("amf-alpha".to_string(), "claude-3".to_string(), 300),
        ];
        let processes = vec![
            proc(100, 1, "-zsh"),
            proc(101, 100, "sh /tmp/amf-agent-launch-abc.sh"),
            proc(102, 101, "/home/me/.local/share/claude/versions/2.1.226"),
            // Never launched: tmux made the window, nothing was ever run.
            proc(200, 1, "-zsh"),
            // Launched once, then quit: the shell is back in the foreground.
            proc(300, 1, "-zsh"),
        ];

        let live = LiveHarnesses::from_census(&panes, &processes);

        assert!(live.is_running("amf-alpha", "claude"));
        assert!(!live.is_running("amf-alpha", "claude-2"));
        assert!(!live.is_running("amf-alpha", "claude-3"));
    }

    #[test]
    fn a_window_tmux_no_longer_reports_is_not_running() {
        let live = LiveHarnesses::from_census(&[], &[proc(100, 1, "-zsh")]);

        assert!(!live.is_running("amf-alpha", "claude"));
    }

    /// A window with several panes counts as busy when any one of them is.
    #[test]
    fn a_split_window_counts_once_if_either_pane_is_busy() {
        let panes = vec![
            ("amf-alpha".to_string(), "claude".to_string(), 100),
            ("amf-alpha".to_string(), "claude".to_string(), 200),
        ];
        let processes = vec![
            proc(100, 1, "-zsh"),
            proc(200, 1, "-zsh"),
            proc(201, 200, "sh /tmp/amf-agent-launch-abc.sh"),
        ];

        let store = store(vec![project(
            "p",
            vec![feature(
                "alpha",
                ProjectStatus::Active,
                vec![session("s1", SessionKind::Claude, "claude")],
            )],
        )]);

        let live = LiveHarnesses::from_census(&panes, &processes);
        assert_eq!(count_active_harness_sessions(&store, &live), 1);
    }

    /// No `ps` means no signal, which must not read as "nothing is running" —
    /// that would silently switch the gate off.
    #[test]
    fn an_unreadable_process_list_falls_back_to_pane_existence() {
        let panes = vec![("amf-alpha".to_string(), "claude".to_string(), 100)];

        let live = LiveHarnesses::from_census(&panes, &[]);

        assert!(live.is_running("amf-alpha", "claude"));
    }

    #[test]
    fn reports_who_is_running() {
        let store = store(vec![project(
            "mainframe",
            vec![feature(
                "alpha",
                ProjectStatus::Active,
                vec![session("s1", SessionKind::Claude, "claude")],
            )],
        )]);
        let live = LiveHarnesses::from_pairs([("amf-alpha", "claude")]);

        let active = active_harness_sessions(&store, &live);
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].project_name, "mainframe");
        assert_eq!(active[0].feature_name, "alpha");
        assert_eq!(active[0].kind, SessionKind::Claude);
    }

    // The headless counter is process-global, so these assert on deltas from
    // whatever baseline the rest of the suite happens to be at rather than on
    // absolute values.
    // These three assert the guard's semantics, so they use their own counter:
    // a real headless run finishing on another test's thread must not be able
    // to shift what they observe.
    static TEST_COUNTER: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn lease_counts_a_run_while_it_is_alive() {
        let _guard = lock_lease_tests();
        TEST_COUNTER.store(0, Ordering::SeqCst);
        {
            let _lease = HeadlessLease::acquire_on(&TEST_COUNTER);
            assert_eq!(TEST_COUNTER.load(Ordering::SeqCst), 1);
            let _second = HeadlessLease::acquire_on(&TEST_COUNTER);
            assert_eq!(TEST_COUNTER.load(Ordering::SeqCst), 2);
        }
        assert_eq!(TEST_COUNTER.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn lease_is_released_on_an_early_error_return() {
        let _guard = lock_lease_tests();
        fn failing_run() -> Result<(), &'static str> {
            let _lease = HeadlessLease::acquire_on(&TEST_COUNTER);
            Err("harness not installed")?;
            unreachable!()
        }
        TEST_COUNTER.store(0, Ordering::SeqCst);
        assert!(failing_run().is_err());
        assert_eq!(TEST_COUNTER.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn lease_is_released_when_a_run_panics() {
        let _guard = lock_lease_tests();
        TEST_COUNTER.store(0, Ordering::SeqCst);
        let result = std::panic::catch_unwind(|| {
            let _lease = HeadlessLease::acquire_on(&TEST_COUNTER);
            panic!("cancelled mid-run");
        });
        assert!(result.is_err());
        assert_eq!(TEST_COUNTER.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn total_active_agents_includes_headless_runs() {
        let _guard = lock_lease_tests();
        let store = store(vec![project(
            "p",
            vec![feature(
                "alpha",
                ProjectStatus::Active,
                vec![session("s1", SessionKind::Claude, "claude")],
            )],
        )]);
        let live = LiveHarnesses::from_pairs([("amf-alpha", "claude")]);

        // Wait for any lease leaked by an earlier test to drain before reading
        // the baseline. A run abandoned elsewhere in the suite (closing the
        // changeset-overview modal, say) keeps its lease for
        // `ABANDONED_RUN_GRACE` past the end of that test, so capturing the
        // count directly can record a baseline that then falls away underneath
        // the assertion below.
        let base = wait_for_in_flight(0);
        let _lease = HeadlessLease::acquire();
        // One live harness session in the store, plus the lease just taken.
        assert_eq!(total_active_agents(&store, &live), 1 + base + 1);
        drop(_lease);
        assert_eq!(wait_for_in_flight(base), base);
    }

    #[test]
    fn empty_store_counts_zero() {
        assert_eq!(
            count_active_harness_sessions(&store(Vec::new()), &LiveHarnesses::default()),
            0
        );
    }
}
