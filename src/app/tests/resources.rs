use super::support::*;
use crate::app::*;
use crate::project::SessionKind;
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use std::sync::atomic::{AtomicBool, Ordering};

// ---------------------------------------------------------------------------
// Pre-start resource gate
// ---------------------------------------------------------------------------

/// A store with one running feature (holding a live Claude harness) and one
/// stopped feature, so the gate has something to count and something to start.
fn store_with_running_and_stopped_features() -> ProjectStore {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "Primary Claude".into());

    let mut stopped = store.projects[0].features[0].clone();
    stopped.id = "feat-2".to_string();
    stopped.name = "other-feat".to_string();
    stopped.branch = "other-feat".to_string();
    stopped.tmux_session = "amf-other-feat".to_string();
    stopped.sessions = vec![];
    stopped.status = ProjectStatus::Stopped;
    store.projects[0].features.push(stopped);
    store
}

/// Census expectations: the running feature's `claude` window has a live
/// harness in it. The returned guard must be held for the test's duration.
#[must_use]
fn expect_one_live_harness(tmux: &mut MockTmuxOps) -> BusyPane {
    tmux.expect_list_sessions()
        .returning(|| Ok(vec!["amf-my-feat".to_string()]));

    // `& wait` keeps the shell itself alive as the parent instead of exec'ing
    // away, so the pane pid really does have a child.
    let child = std::process::Command::new("sh")
        .args(["-c", "sleep 60 & wait"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("sh should be available");
    let pane_pid = child.id() as i64;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        let procs = crate::resources::procs::list_processes();
        if crate::resources::procs::process_tree(&procs, pane_pid).len() > 1 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    tmux.expect_list_panes()
        .returning(move || vec![("amf-my-feat".to_string(), "claude".to_string(), pane_pid)]);
    BusyPane(child)
}

/// Everything `ensure_feature_running` needs to bring the stopped feature up.
fn expect_feature_start(tmux: &mut MockTmuxOps) {
    tmux.expect_session_exists().return_const(false);
    tmux.expect_create_session_with_window()
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));
}

#[test]
fn starting_a_feature_past_the_agent_limit_asks_first() {
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = 0;
    app.selection = Selection::Feature(0, 1);

    app.start_feature().unwrap();

    match &app.mode {
        AppMode::ConfirmResourceStart(state) => {
            let over = state
                .over_limit
                .expect("the limit gate should have tripped");
            assert_eq!(over.active, 1);
            assert_eq!(over.limit, 1);
            assert!(state.low_memory.is_none());
            assert_eq!(state.pending, PendingStart::Feature { pi: 0, fi: 1 });
        }
        other => panic!(
            "expected a resource confirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    // Nothing started while the question is open.
    assert_eq!(
        app.store.projects[0].features[1].status,
        ProjectStatus::Stopped
    );
}

#[test]
fn confirming_the_resource_warning_starts_the_feature() {
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);
    expect_feature_start(&mut tmux);

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = 0;
    app.selection = Selection::Feature(0, 1);

    app.start_feature().unwrap();
    assert!(matches!(app.mode, AppMode::ConfirmResourceStart(_)));

    app.confirm_pending_start().unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(
        app.store.projects[0].features[1].status,
        ProjectStatus::Idle
    );
}

#[test]
fn cancelling_the_resource_warning_starts_nothing() {
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);
    // No start expectations: cancelling must not touch tmux at all.

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = 0;
    app.selection = Selection::Feature(0, 1);

    app.start_feature().unwrap();
    app.cancel_pending_start();

    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(
        app.store.projects[0].features[1].status,
        ProjectStatus::Stopped
    );
}

#[test]
fn starting_a_feature_under_the_limit_does_not_ask() {
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);
    expect_feature_start(&mut tmux);

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 4;
    app.config.low_memory_warn_mb = 0;
    app.selection = Selection::Feature(0, 1);

    app.start_feature().unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(
        app.store.projects[0].features[1].status,
        ProjectStatus::Idle
    );
}

#[test]
fn adding_an_agent_session_past_the_limit_asks_first() {
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = 0;

    app.add_builtin_session_with_label(0, 0, SessionKind::Claude, "Second Claude".into())
        .unwrap();

    match &app.mode {
        AppMode::ConfirmResourceStart(state) => assert_eq!(
            state.pending,
            PendingStart::BuiltinSession {
                pi: 0,
                fi: 0,
                kind: SessionKind::Claude,
                label: Some("Second Claude".into()),
            }
        ),
        other => panic!(
            "expected a resource confirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    // The session is not created until the warning is answered.
    assert_eq!(app.store.projects[0].features[0].sessions.len(), 1);
}

#[test]
fn adding_a_terminal_session_to_a_running_feature_skips_the_gate() {
    // Terminals cost the machine little, and this feature is already up, so
    // the add launches nothing — note there are no census expectations on the
    // mock at all here.
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 1;

    app.add_builtin_session_with_label(0, 0, SessionKind::Terminal, "Shell".into())
        .unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(app.store.projects[0].features[0].sessions.len(), 2);
}

/// The gate lives in the launch primitives, not on the `c` keybinding, so
/// every other way of reaching them is covered too. These four are the routes
/// that used to start harnesses silently.
#[test]
fn adding_a_terminal_to_a_stopped_feature_asks_first() {
    // A terminal is not an agent, but bringing a stopped feature up to host
    // one launches every agent that feature has saved.
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);
    tmux.expect_session_exists()
        .withf(|session| session == "amf-other-feat")
        .return_const(false);

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = 0;

    app.add_builtin_session_with_label(0, 1, SessionKind::Terminal, "Shell".into())
        .unwrap();

    match &app.mode {
        AppMode::ConfirmResourceStart(state) => assert_eq!(
            state.pending,
            PendingStart::BuiltinSession {
                pi: 0,
                fi: 1,
                kind: SessionKind::Terminal,
                label: Some("Shell".into()),
            }
        ),
        other => panic!(
            "expected a resource confirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    assert!(app.store.projects[0].features[1].sessions.is_empty());
}

#[test]
fn entering_a_stopped_feature_asks_first() {
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);
    tmux.expect_session_exists()
        .withf(|session| session == "amf-other-feat")
        .return_const(false);

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = 0;
    app.selection = Selection::Feature(0, 1);

    app.enter_view().unwrap();

    match &app.mode {
        AppMode::ConfirmResourceStart(state) => assert_eq!(
            state.pending,
            PendingStart::EnterView { auto_compose: true }
        ),
        other => panic!(
            "expected a resource confirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    assert_eq!(
        app.store.projects[0].features[1].status,
        ProjectStatus::Stopped,
        "nothing should have started while the question is on screen"
    );
}

#[test]
fn entering_a_running_feature_never_asks() {
    // The harness is already up and already counted: re-entering its view must
    // not put a question in the way.
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    tmux.expect_resize_pane().returning(|_, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = u64::MAX;
    app.selection = Selection::Feature(0, 0);

    app.enter_view_without_auto_compose().unwrap();

    assert!(
        matches!(app.mode, AppMode::Viewing(_)),
        "got {:?}",
        std::mem::discriminant(&app.mode)
    );
}

#[test]
fn confirming_the_warning_opens_the_stopped_feature() {
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);
    // False while the gate looks, true once the session has been created.
    let created = std::sync::Arc::new(AtomicBool::new(false));
    let seen = created.clone();
    tmux.expect_session_exists()
        .withf(|session| session == "amf-other-feat")
        .returning(move |_| seen.load(Ordering::SeqCst));
    tmux.expect_create_session_with_window()
        .returning(move |_, _, _| {
            created.store(true, Ordering::SeqCst);
            Ok(())
        });
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_resize_pane().returning(|_, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = 0;
    app.selection = Selection::Feature(0, 1);

    app.enter_view_without_auto_compose().unwrap();
    match &app.mode {
        // The flag the caller opened with travels through the dialog, so the
        // replay is the same operation and not a lookalike.
        AppMode::ConfirmResourceStart(state) => assert_eq!(
            state.pending,
            PendingStart::EnterView {
                auto_compose: false
            }
        ),
        other => panic!(
            "expected a resource confirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    app.confirm_pending_start().unwrap();

    // Replayed as the original operation, not just as a bare feature start:
    // the user asked to open the feature, so they land in its view.
    assert!(
        matches!(app.mode, AppMode::Viewing(_)),
        "got {:?}",
        std::mem::discriminant(&app.mode)
    );
    assert_eq!(
        app.store.projects[0].features[1].status,
        ProjectStatus::Active
    );
}

#[test]
fn a_start_that_cannot_be_parked_warns_and_goes_ahead() {
    // Spawning an agent from a TODO, the PR-triage hand-off, and the
    // saved-transcript pickers all keep the state they need to resume in the
    // very mode the dialog would replace. They get a toast instead of a modal
    // — but never silence.
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);
    tmux.expect_session_exists().return_const(true);
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = 0;

    let si = app
        .create_agent_session_labeled(
            0,
            0,
            "TODO: rename things",
            None,
            StartIntent::Warn("the agent for this TODO"),
        )
        .expect("the start should go ahead");

    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(app.store.projects[0].features[0].sessions.len(), 2);
    assert_eq!(si, 1);
    let toast = app
        .toasts
        .last()
        .map(|toast| toast.message.clone())
        .unwrap_or_default();
    assert!(
        toast.contains("the agent for this TODO"),
        "the warning should name what it is starting, got {toast:?}"
    );
    assert!(toast.contains("1 agent already running"), "got {toast:?}");
}

#[test]
fn low_memory_alone_raises_the_warning() {
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    // Concurrency is fine; the memory floor is set above any real machine.
    app.config.max_concurrent_agents = 0;
    app.config.low_memory_warn_mb = u64::MAX;
    app.selection = Selection::Feature(0, 1);

    app.start_feature().unwrap();

    match &app.mode {
        AppMode::ConfirmResourceStart(state) => {
            assert!(state.over_limit.is_none());
            // Only assert the shape: a platform with no memory signal
            // legitimately has nothing to warn about.
            assert_eq!(
                state.low_memory.is_some(),
                crate::resources::mem::probe().is_some()
            );
        }
        AppMode::Normal => assert!(
            crate::resources::mem::probe().is_none(),
            "a machine with a memory signal must have warned"
        ),
        other => panic!("unexpected mode {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn autostart_skips_with_a_warning_instead_of_prompting() {
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = 0;

    assert!(!app.autostart_allowed("other-feat"));

    // Creation paths must never raise the modal: a batch create would queue
    // one per feature and the automation API has nobody to answer them.
    assert!(matches!(app.mode, AppMode::Normal));
    // Reported once, as a toast -- not also duplicated into the status line.
    assert!(app.message.is_none(), "got {:?}", app.message);
    let toast = app
        .toasts
        .last()
        .map(|toast| toast.message.clone())
        .unwrap_or_default();
    assert!(toast.contains("other-feat"), "got {toast:?}");
    // Singular here on purpose: one agent, one limit.
    assert!(toast.contains("1 agent already running"), "got {toast:?}");
    assert!(toast.contains("Press c to start it"), "got {toast:?}");
}

#[test]
fn autostart_proceeds_when_there_is_room() {
    let mut tmux = MockTmuxOps::new();
    let _pane = expect_one_live_harness(&mut tmux);

    let mut app = App::new_for_test(
        store_with_running_and_stopped_features(),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.max_concurrent_agents = 4;
    app.config.low_memory_warn_mb = 0;

    assert!(app.autostart_allowed("other-feat"));
    assert!(matches!(app.mode, AppMode::Normal));
}
