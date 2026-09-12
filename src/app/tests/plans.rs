use super::support::*;
use crate::app::plan_interview::PLAN_KICKOFF_PROMPT;
use crate::app::*;
use crate::extension::FeaturePreset;
use crate::project::{AgentKind, Project, SessionKind};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tempfile::NamedTempFile;
use tempfile::TempDir;

#[test]
fn create_feature_with_plan_mode_defers_launch_into_interview() {
    let repo = TempDir::new().unwrap();
    let now = Utc::now();
    let store = ProjectStore {
        version: 5,
        projects: vec![Project {
            id: "proj-1".into(),
            name: "my-project".into(),
            repo: repo.path().to_path_buf(),
            collapsed: false,
            features: vec![],
            created_at: now,
            preferred_agent: AgentKind::Claude,
            is_git: true,
        }],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: vec![],
        extra: HashMap::new(),
    };
    let mut state = CreateFeatureState::new(
        "my-project".into(),
        repo.path().to_path_buf(),
        Vec::new(),
        true,
    );
    state.branch = "planned-feature".into();
    state.plan_mode = true;
    state.use_worktree = false;
    state.session_name = "Claude 1".into();

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::CreatingFeature(state);
    app.create_feature().unwrap();

    assert!(app.store.projects[0].features.is_empty());
    match &app.mode {
        AppMode::PlanInterview(interview) => {
            assert_eq!(interview.feature_name, "planned-feature");
            assert_eq!(interview.phase, PlanInterviewPhase::Brief);
            let pending = interview.pending_launch.as_ref().unwrap();
            assert_eq!(pending.branch, "planned-feature");
            assert!(pending.plan_mode);
            assert_eq!(pending.workdir, repo.path());
        }
        _ => panic!("expected plan interview before feature launch"),
    }
}

#[test]
fn plan_mode_preset_defers_feature_launch_into_interview() {
    let repo = TempDir::new().unwrap();
    let now = Utc::now();
    let store = ProjectStore {
        version: 5,
        projects: vec![Project {
            id: "proj-1".into(),
            name: "my-project".into(),
            repo: repo.path().to_path_buf(),
            collapsed: false,
            features: vec![],
            created_at: now,
            preferred_agent: AgentKind::Claude,
            is_git: true,
        }],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: vec![],
        extra: HashMap::new(),
    };
    let mut state = CreateFeatureState::new(
        "my-project".into(),
        repo.path().to_path_buf(),
        Vec::new(),
        true,
    );
    state.step = CreateFeatureStep::SelectPreset;
    state.feature_presets = vec![FeaturePreset {
        name: "Plan first".into(),
        plan_mode: true,
        ..Default::default()
    }];

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::CreatingFeature(state);

    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Enter).unwrap();
    match &mut app.mode {
        AppMode::CreatingFeature(state) => {
            assert!(state.plan_mode, "the preset must enable plan mode");
            state.branch = "preset-planned-feature".into();
            state.step = CreateFeatureStep::SessionName;
        }
        _ => panic!("expected the preset to return to feature creation"),
    }

    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Enter).unwrap();

    assert!(app.store.projects[0].features.is_empty());
    match &app.mode {
        AppMode::PlanInterview(interview) => {
            assert_eq!(interview.feature_name, "preset-planned-feature");
            assert_eq!(interview.phase, PlanInterviewPhase::Brief);
            assert!(interview.pending_launch.as_ref().unwrap().plan_mode);
        }
        _ => panic!("expected a plan-mode preset to open the interview"),
    }
}

#[test]
fn plan_interview_abort_can_resume_or_cancel_feature_creation() {
    let repo = TempDir::new().unwrap();
    let store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();
    app.finish_feature_launch(PreparedFeatureLaunch {
        project_name: "my-project".into(),
        branch: "planned-feature".into(),
        workdir: repo.path().join(".worktrees/planned-feature"),
        is_worktree: true,
        mode: VibeMode::default(),
        review: false,
        plan_mode: true,
        quick_plan: false,
        agent: AgentKind::Claude,
        create_terminal: false,
        session_name: "Claude 1".into(),
        enable_chrome: false,
        remote_control: false,
        steering_enabled: false,
        hook_succeeded: None,
        startup_prompt: None,
        todo_origin: None,
    })
    .unwrap();

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(matches!(&app.mode, AppMode::PlanInterview(state) if state.abort_confirmation));
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(matches!(&app.mode, AppMode::PlanInterview(state) if !state.abort_confirmation));

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('n'))).unwrap();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("worktree kept")
    );
    assert_eq!(app.store.projects[0].features.len(), 1);
}

/// An app sitting on the selected feature with no interview running — the
/// starting point for the on-demand trigger, which plans a feature that
/// already exists rather than deferring a launch.
fn app_on_selected_feature() -> (App, tempfile::NamedTempFile, TempDir) {
    let repo = TempDir::new().unwrap();
    let store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();
    app.selection = Selection::Feature(0, 0);
    (app, store_file, repo)
}

#[test]
fn on_demand_plan_interview_plans_the_selected_feature_without_a_launch() {
    let (mut app, _store_file, repo) = app_on_selected_feature();

    crate::handlers::handle_normal_key(&mut app, ke(KeyCode::Char('P'))).unwrap();

    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.feature_name, "my-feat");
            // Keyed by the feature's id, so the accepted transcript is filed
            // where a later re-run on this feature will look for it.
            assert_eq!(state.interview_key, "feat-1");
            assert_eq!(state.workdir, repo.path());
            assert!(state.pending_launch.is_none());
            assert_eq!(state.phase, PlanInterviewPhase::Brief);
        }
        _ => panic!("expected plan interview mode"),
    }
}

#[test]
fn plan_interview_can_pause_to_its_dashboard_row_and_resume_with_enter() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.editor = crate::editor::TextEditor::new("Uncommitted research question".into());
    }

    crate::handlers::handle_plan_interview_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL),
    )
    .unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(matches!(app.selection, Selection::Project(0)));
    assert!(app.paused_plan_interview_matches_selection());
    assert_eq!(
        app.paused_plan_interview.as_ref().unwrap().editor.text(),
        "Uncommitted research question"
    );

    crate::handlers::handle_normal_key(&mut app, ke(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.editor.text(), "Uncommitted research question");
        }
        _ => panic!("Enter on the marked project must resume its interview"),
    }
    assert!(app.paused_plan_interview.is_none());
}

#[test]
fn dashboard_quit_reopens_the_parked_interview_abort_confirmation() {
    for key in [KeyCode::Char('q'), KeyCode::Esc] {
        let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
        if let AppMode::PlanInterview(state) = &mut app.mode {
            state.editor = crate::editor::TextEditor::new("Uncommitted answer".into());
        }
        app.pause_plan_interview();

        crate::handlers::handle_normal_key(&mut app, ke(key)).unwrap();

        assert!(!app.should_quit);
        assert!(app.paused_plan_interview.is_none());
        match &app.mode {
            AppMode::PlanInterview(state) => {
                assert!(state.abort_confirmation);
                assert_eq!(state.editor.text(), "Uncommitted answer");
            }
            _ => panic!("dashboard quit must return to the interview abort gate"),
        }
    }
}

#[test]
fn parked_plan_interview_blocks_another_feature_and_owning_project_deletion() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    app.pause_plan_interview();

    app.start_create_feature();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.paused_plan_interview.is_some());
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("before creating another feature")
    );

    crate::handlers::handle_normal_key(&mut app, ke(KeyCode::Char('d'))).unwrap();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.paused_plan_interview.is_some());
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("before deleting its project")
    );
}

#[test]
fn parked_plan_interview_blocks_owning_feature_deletion() {
    let (mut app, _store_file, _repo) = app_on_selected_feature();
    app.start_plan_interview_for_selected_feature();
    app.pause_plan_interview();

    crate::handlers::handle_normal_key(&mut app, ke(KeyCode::Char('d'))).unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.paused_plan_interview.is_some());
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("before deleting its feature")
    );
}

#[test]
fn pause_plan_interview_never_overwrites_an_existing_parked_interview() {
    let (mut app, _store_file, _repo) = app_on_selected_feature();
    app.start_plan_interview_for_selected_feature();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.editor = crate::editor::TextEditor::new("First interview".into());
    }
    app.pause_plan_interview();

    let mut second =
        crate::app::PlanInterviewState::new("second".into(), "second-id".into(), Vec::new(), None);
    second.editor = crate::editor::TextEditor::new("Second interview".into());
    app.mode = AppMode::PlanInterview(second);
    app.pause_plan_interview();

    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state) if state.editor.text() == "Second interview"
    ));
    assert_eq!(
        app.paused_plan_interview.as_ref().unwrap().editor.text(),
        "First interview"
    );
}

#[test]
fn leaving_a_session_returns_to_the_paused_plan_interview() {
    let (mut app, _store_file, _repo) = app_on_selected_feature();
    app.start_plan_interview_for_selected_feature();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.editor = crate::editor::TextEditor::new("Look up the router first".into());
    }
    app.pause_plan_interview();

    // `exit_view` is the common destination of Ctrl+Q from every embedded
    // session. The parked interview takes precedence over the usual dashboard
    // return and retains even text that has not yet been committed as an answer.
    app.exit_view();

    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.editor.text(), "Look up the router first")
        }
        _ => panic!("leaving the inspection session must restore the interview"),
    }
    assert!(app.paused_plan_interview.is_none());
}

#[test]
fn plan_interview_does_not_pause_while_a_paid_operation_is_running() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    let (_tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_synthesis_bg = Some(rx);

    app.pause_plan_interview();

    assert!(matches!(app.mode, AppMode::PlanInterview(_)));
    assert!(app.paused_plan_interview.is_none());
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("operation to finish")
    );
}

#[test]
fn accepting_an_on_demand_plan_writes_it_into_the_features_own_workdir() {
    let (mut app, _store_file, repo) = app_on_selected_feature();
    std::fs::write(repo.path().join("PLAN.md"), "# Repository roadmap\n").unwrap();
    app.start_plan_interview_for_selected_feature();
    force_plan_interview_raw_fallback(&mut app);

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.brief = "Tighten the sidebar".into();
        state.synthesis_requested = true;
        state.phase = PlanInterviewPhase::Done;
    } else {
        panic!("expected plan interview mode");
    }
    app.continue_plan_interview_after_done().unwrap();
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    let plan = std::fs::read_to_string(repo.path().join("AMF_PLAN.md")).unwrap();
    assert!(plan.contains("Tighten the sidebar"));
    assert_eq!(
        std::fs::read_to_string(repo.path().join("PLAN.md")).unwrap(),
        "# Repository roadmap\n"
    );
    // Writing the file is not enough on its own: without the instruction block
    // the agent is never told the plan exists, and without the flag a restart
    // would stop injecting it.
    assert!(app.store.projects[0].features[0].plan_mode);
    let instructions = std::fs::read_to_string(repo.path().join("CLAUDE.local.md")).unwrap();
    assert!(instructions.contains("AMF_PLAN.md"));
}

/// `app_on_selected_feature`, but the feature's agent session is live — the
/// case the accepted plan has somewhere to hand off to. Permissive tmux
/// expectations: these tests care about the handoff decision, not the call
/// sequence entering an already-running session makes.
fn app_on_running_selected_feature() -> (App, tempfile::NamedTempFile, TempDir) {
    let repo = TempDir::new().unwrap();
    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active);
    store.projects[0].features[0]
        .sessions
        .push(make_session("Claude 1", None));

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);
    tmux.expect_window_exists().returning(|_, _| true);
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();
    app.selection = Selection::Feature(0, 0);
    (app, store_file, repo)
}

/// Drive an on-demand interview to an accepted raw-fallback plan, which is the
/// point the live-session handoff is decided.
fn accept_on_demand_plan_for_test(app: &mut App, brief: &str) {
    app.start_plan_interview_for_selected_feature();
    force_plan_interview_raw_fallback(app);

    let AppMode::PlanInterview(state) = &mut app.mode else {
        panic!("expected plan interview mode");
    };
    state.brief = brief.into();
    state.synthesis_requested = true;
    state.phase = PlanInterviewPhase::Done;

    app.continue_plan_interview_after_done().unwrap();
    crate::handlers::handle_plan_interview_key(app, ke(KeyCode::Enter)).unwrap();
}

/// A running agent read its instruction file once, at startup, so a plan
/// written underneath it goes unnoticed until something says so.
#[test]
fn accepting_an_on_demand_plan_offers_the_kickoff_to_a_running_session() {
    let (mut app, _store_file, repo) = app_on_running_selected_feature();
    accept_on_demand_plan_for_test(&mut app, "Tighten the sidebar");

    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.phase, PlanInterviewPhase::KickoffHandoff);
            let target = state.kickoff_handoff.as_ref().unwrap();
            assert_eq!(target.session_label, "Claude 1");
            assert_eq!(target.session_id, "session-Claude 1");
            assert_eq!(target.plan_path, repo.path().join("AMF_PLAN.md"));
        }
        _ => panic!("expected the handoff prompt"),
    }
    // The offer comes after the accept has fully landed, so declining it can
    // never cost the plan.
    assert!(
        std::fs::read_to_string(repo.path().join("AMF_PLAN.md"))
            .unwrap()
            .contains("Tighten the sidebar")
    );
    assert!(app.store.projects[0].features[0].plan_mode);
}

#[test]
fn declining_the_kickoff_handoff_leaves_the_running_session_alone() {
    let (mut app, _store_file, repo) = app_on_running_selected_feature();
    accept_on_demand_plan_for_test(&mut app, "Tighten the sidebar");

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('n'))).unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains(&repo.path().join("AMF_PLAN.md").display().to_string())
    );
}

#[test]
fn accepting_the_kickoff_handoff_seeds_the_running_sessions_composer() {
    let (mut app, _store_file, _repo) = app_on_running_selected_feature();
    accept_on_demand_plan_for_test(&mut app, "Tighten the sidebar");

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('y'))).unwrap();

    // Seeded, never submitted: the session may be mid-task, so when the prompt
    // lands stays the user's call.
    match &app.mode {
        AppMode::Compose(state) => {
            let seed = state.editor.text();
            assert!(seed.contains("AMF_PLAN.md"));
            assert!(seed.contains("decisions are settled"));
            assert_eq!(state.view.feature_name, "my-feat");
        }
        _ => panic!("expected the live session's composer to be seeded"),
    }
}

/// The handoff is only offered against a session tmux still has. AMF's status
/// is reconciled every few seconds, so a session killed outside AMF still reads
/// as running until the next sync — trusting the flag alone would offer to type
/// into nothing.
#[test]
fn a_feature_marked_active_without_a_tmux_session_gets_no_handoff_offer() {
    let (mut app, _store_file, repo) = app_on_running_selected_feature();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| false);
    app.tmux = Box::new(tmux);

    accept_on_demand_plan_for_test(&mut app, "Tighten the sidebar");

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains(&repo.path().join("AMF_PLAN.md").display().to_string())
    );
}

/// A live tmux session is not a live agent: the terminal window alone keeps the
/// session up after the harness exits, so the session-level check passes while
/// there is nothing left to hand the plan to.
#[test]
fn a_feature_whose_agent_window_exited_gets_no_handoff_offer() {
    let (mut app, _store_file, repo) = app_on_running_selected_feature();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);
    tmux.expect_window_exists().returning(|_, _| false);
    app.tmux = Box::new(tmux);

    accept_on_demand_plan_for_test(&mut app, "Tighten the sidebar");

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.message
            .as_deref()
            .unwrap()
            .contains(&repo.path().join("AMF_PLAN.md").display().to_string())
    );
}

/// With several harnesses configured, declaration order is the wrong tiebreak:
/// the first one may be long dead while a later one is doing the work.
#[test]
fn the_handoff_targets_the_harness_that_is_actually_running() {
    let (mut app, _store_file, _repo) = app_on_running_selected_feature();
    app.store.projects[0].features[0]
        .sessions
        .push(make_session("Claude 2", None));

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);
    tmux.expect_window_exists()
        .returning(|_, window| window == "Claude 2");
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));
    app.tmux = Box::new(tmux);

    accept_on_demand_plan_for_test(&mut app, "Tighten the sidebar");

    match &app.mode {
        AppMode::PlanInterview(state) => {
            let target = state.kickoff_handoff.as_ref().unwrap();
            assert_eq!(target.session_label, "Claude 2");
        }
        _ => panic!("expected the handoff prompt"),
    }
}

/// The offer sits on screen for as long as the user takes to answer it, and the
/// harness can exit in that window. Entering it then would recreate the session
/// — a far bigger action than the one being offered.
#[test]
fn a_harness_that_exits_while_the_offer_is_up_is_not_reopened() {
    let (mut app, _store_file, repo) = app_on_running_selected_feature();
    accept_on_demand_plan_for_test(&mut app, "Tighten the sidebar");

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);
    tmux.expect_window_exists().returning(|_, _| false);
    app.tmux = Box::new(tmux);

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('y'))).unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    let message = app.message.as_deref().unwrap();
    assert!(message.contains(&repo.path().join("AMF_PLAN.md").display().to_string()));
    assert!(message.contains("no longer running"));
}

#[test]
fn aborting_an_on_demand_interview_has_no_feature_to_cancel() {
    let (mut app, _store_file, repo) = app_on_selected_feature();
    app.start_plan_interview_for_selected_feature();

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(matches!(&app.mode, AppMode::PlanInterview(state) if state.abort_confirmation));

    // `n` cancels feature creation, which an on-demand interview never started;
    // the dialog does not offer it, so it must not exit the interview either.
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('n'))).unwrap();
    assert!(matches!(&app.mode, AppMode::PlanInterview(state) if state.abort_confirmation));

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('y'))).unwrap();
    assert!(matches!(app.mode, AppMode::Normal));
    // Leaving is non-destructive: the feature keeps whatever plan it had.
    assert!(!repo.path().join("AMF_PLAN.md").exists());
    assert!(!app.store.projects[0].features[0].plan_mode);
    assert_eq!(app.store.projects[0].features.len(), 1);
}

#[test]
fn command_picker_offers_the_plan_interview_only_with_a_feature_in_hand() {
    let (mut app, _store_file, _repo) = app_on_selected_feature();

    app.open_command_picker(None);
    let offered_on_feature = matches!(&app.mode, AppMode::CommandPicker(state)
        if state.commands.iter().any(|entry| entry.name == "plan-interview"));
    assert!(offered_on_feature);

    // A project row has no workdir to plan against.
    app.mode = AppMode::Normal;
    app.selection = Selection::Project(0);
    app.open_command_picker(None);
    let offered_on_project = matches!(&app.mode, AppMode::CommandPicker(state)
        if state.commands.iter().any(|entry| entry.name == "plan-interview"));
    assert!(!offered_on_project);
}

/// Common setup for the `poll_plan_interview_ai_bg` tests below: a feature
/// launch deferred into a plan interview, exactly like the abort test above.
fn app_with_deferred_plan_interview() -> (App, tempfile::NamedTempFile, TempDir) {
    plan_interview_app(None)
}

/// `app_with_deferred_plan_interview` plus a real SQLite database, so the
/// draft-persistence path writes and reads actual rows. The extra `TempDir`
/// holds the database file and must outlive the app.
fn app_with_deferred_plan_interview_and_db() -> (App, tempfile::NamedTempFile, TempDir, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let (app, store_file, repo) = plan_interview_app(Some(&db_dir.path().join("amf.db")));
    (app, store_file, repo, db_dir)
}

fn plan_interview_app(
    db_path: Option<&std::path::Path>,
) -> (App, tempfile::NamedTempFile, TempDir) {
    plan_interview_app_for_agent(db_path, AgentKind::Claude)
}

fn plan_interview_app_for_agent(
    db_path: Option<&std::path::Path>,
    agent: AgentKind,
) -> (App, tempfile::NamedTempFile, TempDir) {
    let mode = if agent == AgentKind::Codex {
        VibeMode::Vibe
    } else {
        VibeMode::default()
    };
    let repo = TempDir::new().unwrap();
    let store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    // Permissive rather than strict-sequence expectations: these tests care
    // about the plan-interview state machine, not the exact tmux call
    // sequence a real launch makes (already covered elsewhere).
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| false);
    tmux.expect_create_session_with_window()
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_create_window().returning(|_, _, _| Ok(()));
    match agent {
        AgentKind::Claude => {
            tmux.expect_launch_claude()
                .returning(|_, _, _, _, _| Ok(()));
        }
        AgentKind::Codex => {
            tmux.expect_launch_codex().returning(|_, _, _, _, _| Ok(()));
        }
        _ => unreachable!("this helper only covers plan kickoff harnesses"),
    }
    tmux.expect_select_window().returning(|_, _| Ok(()));
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();
    if let Some(db_path) = db_path {
        app.db = Some(crate::db::AmfDb::open(db_path).unwrap());
    }
    app.finish_feature_launch(PreparedFeatureLaunch {
        project_name: "my-project".into(),
        branch: "planned-feature".into(),
        workdir: repo.path().join(".worktrees/planned-feature"),
        is_worktree: true,
        mode,
        review: false,
        plan_mode: true,
        quick_plan: false,
        session_name: format!("{} 1", agent.display_name()),
        agent,
        create_terminal: false,
        enable_chrome: false,
        remote_control: false,
        steering_enabled: false,
        hook_succeeded: None,
        startup_prompt: None,
        todo_origin: None,
    })
    .unwrap();
    // The NamedTempFile return value keeps the store file alive for the
    // caller; `repo` is returned too so the workdir it created stays alive
    // rather than being deleted the instant this function returns.
    (app, store_file, repo)
}

#[test]
fn accepting_a_codex_plan_seeds_the_approved_plan_kickoff_prompt() {
    let (mut app, _store_file, _repo) = plan_interview_app_for_agent(None, AgentKind::Codex);
    app.config.max_concurrent_agents = 0;
    force_plan_interview_raw_fallback(&mut app);

    let workdir = match &mut app.mode {
        AppMode::PlanInterview(state) => {
            // The approved plan is itself the startup direction, so its
            // kickoff must win over the wizard's optional blank steering box.
            state.pending_launch.as_mut().unwrap().steering_enabled = true;
            state.brief = "Implement this with Codex".into();
            state.synthesis_requested = true;
            state.phase = PlanInterviewPhase::Done;
            state.workdir.clone()
        }
        _ => panic!("expected plan interview mode"),
    };
    app.continue_plan_interview_after_done().unwrap();
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert!(
        matches!(app.mode, AppMode::Compose(_)),
        "expected composer after Codex launch; message: {:?}",
        app.message
    );
    assert!(workdir.join("AMF_PLAN.md").is_file());
    assert_eq!(app.store.projects[0].features[1].agent, AgentKind::Codex);
    assert!(app.store.projects[0].features[1].plan_mode);
    assert_eq!(app.store.projects[0].features[1].workdir, workdir);
    assert!(
        std::fs::read_to_string(workdir.join("AGENTS.md"))
            .unwrap()
            .contains("`AMF_PLAN.md`")
    );
    match &app.mode {
        AppMode::Compose(state) => {
            assert_eq!(state.view.session_kind, SessionKind::Codex);
            assert!(state.editor.text().contains("Read `AMF_PLAN.md`"));
            assert!(state.editor.text().contains("decisions are settled"));
        }
        _ => panic!("expected Codex's composer to contain the kickoff prompt"),
    }
}

/// Keep completion-path tests deterministic and offline. `Some(None)` means
/// harness resolution has already run and no synthesis engine is available,
/// so the interview must use its raw-Q&A fallback.
fn force_plan_interview_raw_fallback(app: &mut App) {
    let AppMode::PlanInterview(state) = &mut app.mode else {
        panic!("expected plan interview mode");
    };
    state.ai_harness = Some(None);
}

fn synthesized_plan_response() -> String {
    "# Plan: planned-feature\n\n## Goal\nShip a useful feature.\n\n\
     ## Decisions\n- Use the native TUI.\n\n\
     ## Architecture\nNo changes identified.\n\n\
     ## UI\nAdd a plan-mode dialog.\n\n\
     ## Tasks\n- [ ] Implement the feature\n- [ ] Verify it\n\n\
     ## Risks / open questions\n- None identified.\n"
        .to_string()
}

/// Tmux behavior needed by an accepted plan that first opens the resource
/// confirmation and then launches after approval. A real sleeping child makes
/// the existing feature's mocked pane count as busy without touching the
/// process-global headless lease used by concurrently-running tests.
fn plan_resource_gate_tmux(expect_launch: bool) -> (MockTmuxOps, BusyPane) {
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

    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_panes()
        .returning(move || vec![("amf-my-feat".to_string(), "claude".to_string(), pane_pid)]);
    if expect_launch {
        let created = Arc::new(AtomicBool::new(false));
        let seen = created.clone();
        tmux.expect_session_exists()
            .returning(move |_| seen.load(Ordering::SeqCst));
        tmux.expect_create_session_with_window()
            .returning(move |_, _, _| {
                created.store(true, Ordering::SeqCst);
                Ok(())
            });
        tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
        tmux.expect_launch_claude()
            .returning(|_, _, _, _, _| Ok(()));
        tmux.expect_select_window().returning(|_, _| Ok(()));
    }
    (tmux, BusyPane(child))
}

#[test]
fn accepted_plan_over_limit_asks_then_resumes_exact_launch_once() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    app.store.projects[0].features[0].status = ProjectStatus::Active;
    app.store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Existing Claude".into());
    let (tmux, _pane) = plan_resource_gate_tmux(true);
    app.tmux = Box::new(tmux);
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = 0;

    let (workdir, expected_plan) = match &mut app.mode {
        AppMode::PlanInterview(state) => {
            let plan = synthesized_plan_response();
            state.apply_synthesis(plan.clone());
            (state.workdir.clone(), plan)
        }
        _ => panic!("expected plan review"),
    };

    app.complete_plan_interview().unwrap();

    match &app.mode {
        AppMode::ConfirmResourceStart(state) => {
            assert_eq!(state.over_limit.unwrap().limit, 1);
            let PendingStart::PlannedFeature(pending) = &state.pending else {
                panic!("expected the accepted plan launch to be parked");
            };
            assert_eq!(pending.plan, expected_plan);
            assert_eq!(pending.prepared.workdir, workdir);
            assert_eq!(
                pending.prepared.startup_prompt.as_deref(),
                Some(PLAN_KICKOFF_PROMPT)
            );
            assert!(matches!(
                state.plan_interview.as_ref(),
                Some(interview)
                    if interview.phase == PlanInterviewPhase::Review
                        && interview.synthesized_plan.as_deref() == Some(expected_plan.as_str())
                        && interview.pending_launch.is_some()
            ));
        }
        _ => panic!("expected the resource confirmation"),
    }
    assert!(workdir.join("AMF_PLAN.md").is_file());
    assert!(
        !app.store.projects[0]
            .features
            .iter()
            .any(|feature| feature.name == "planned-feature")
    );
    assert!(
        !app.toasts
            .iter()
            .any(|toast| toast.message.contains("Press c to start it"))
    );

    crate::handlers::handle_resource_confirm_key(&mut app, KeyCode::Enter).unwrap();

    assert_eq!(
        app.store.projects[0]
            .features
            .iter()
            .filter(|feature| feature.name == "planned-feature")
            .count(),
        1
    );
    match &app.mode {
        AppMode::Compose(state) => {
            assert_eq!(state.editor.text(), PLAN_KICKOFF_PROMPT);
            assert_eq!(state.view.feature_name, "planned-feature");
        }
        _ => panic!("confirmed plan should land in the seeded composer"),
    }

    // The dialog payload was consumed before replay. A repeated confirmation
    // call is a no-op and cannot create a duplicate feature or second harness.
    app.confirm_pending_start().unwrap();
    assert_eq!(
        app.store.projects[0]
            .features
            .iter()
            .filter(|feature| feature.name == "planned-feature")
            .count(),
        1
    );
}

#[test]
fn cancelling_over_limit_plan_start_restores_completed_review() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    app.store.projects[0].features[0].status = ProjectStatus::Active;
    app.store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Existing Claude".into());
    let (tmux, _pane) = plan_resource_gate_tmux(false);
    app.tmux = Box::new(tmux);
    app.config.max_concurrent_agents = 1;
    app.config.low_memory_warn_mb = 0;

    let (workdir, expected_plan) = match &mut app.mode {
        AppMode::PlanInterview(state) => {
            let plan = synthesized_plan_response();
            state.apply_synthesis(plan.clone());
            (state.workdir.clone(), plan)
        }
        _ => panic!("expected plan review"),
    };
    app.complete_plan_interview().unwrap();
    assert!(matches!(app.mode, AppMode::ConfirmResourceStart(_)));

    crate::handlers::handle_resource_confirm_key(&mut app, KeyCode::Esc).unwrap();

    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesized_plan.as_deref() == Some(expected_plan.as_str())
                && state.pending_launch.is_some()
    ));
    assert_eq!(
        app.message.as_deref(),
        Some("Planned feature start cancelled; plan kept for review")
    );
    assert_eq!(
        std::fs::read_to_string(workdir.join("AMF_PLAN.md")).unwrap(),
        expected_plan
    );
    assert!(
        !app.store.projects[0]
            .features
            .iter()
            .any(|feature| feature.name == "planned-feature")
    );
}

fn plan_critique_response() -> String {
    "# Plan review: planned-feature\n\n\
     ## Summary\nReady with caveats.\n\n\
     ## Gaps\n- No rollback story.\n\n\
     ## Risks\n- None identified.\n\n\
     ## Contradictions\n- None identified.\n\n\
     ## Unclear decisions\n- None identified.\n\n\
     ## Missing acceptance criteria\n- None identified.\n"
        .to_string()
}

/// Drop straight into an in-flight agent review, as
/// `start_plan_interview_critique` would leave it, without spawning a real
/// headless call.
fn begin_plan_critique_for_test(app: &mut App) -> std::sync::mpsc::Sender<anyhow::Result<String>> {
    let AppMode::PlanInterview(state) = &mut app.mode else {
        panic!("expected plan interview mode");
    };
    assert!(state.begin_critique(500));
    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_critique_bg = Some(rx);
    tx
}

/// Drop straight into an in-flight directed revision without launching a real
/// harness. The production path builds the same state immediately before it
/// spawns the read-only worker.
fn begin_directed_plan_revision_for_test(
    app: &mut App,
) -> std::sync::mpsc::Sender<anyhow::Result<String>> {
    let AppMode::PlanInterview(state) = &mut app.mode else {
        panic!("expected plan interview mode");
    };
    assert!(state.begin_directed_feedback());
    state.editor = crate::editor::TextEditor::new(
        "Inspect the router and add the concrete files to Tasks.".into(),
    );
    assert!(state.begin_directed_feedback_loading(650));
    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_directed_feedback_bg = Some(rx);
    tx
}

/// Drop straight into an in-flight isolated investigation without launching
/// real read-only or merge harnesses.
fn begin_plan_investigation_for_test(
    app: &mut App,
) -> std::sync::mpsc::Sender<anyhow::Result<crate::plan_interview::PlanInvestigationOutcome>> {
    let AppMode::PlanInterview(state) = &mut app.mode else {
        panic!("expected plan interview mode");
    };
    assert!(state.begin_investigation());
    state.editor = crate::editor::TextEditor::new(
        "Trace the session launch boundary and identify its tests.".into(),
    );
    assert!(state.begin_investigation_loading(1_200));
    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_investigation_bg = Some(rx);
    tx
}

/// A merge result in which every investigator completed.
fn investigation_outcome(
    merge_response: String,
) -> crate::plan_interview::PlanInvestigationOutcome {
    crate::plan_interview::PlanInvestigationOutcome {
        merge_response,
        failed_focuses: Vec::new(),
    }
}

#[test]
fn directed_plan_feedback_replaces_the_draft_then_returns_to_review() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    let original = synthesized_plan_response();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(original.clone());
    }

    let tx = begin_directed_plan_revision_for_test(&mut app);
    let revised = original.replace("Implement the feature", "Update src/handlers/router.rs");
    tx.send(Ok(revised.clone())).unwrap();

    assert!(app.poll_plan_interview_directed_feedback_bg());
    assert!(app.plan_interview_directed_feedback_bg.is_none());
    assert_eq!(
        app.message.as_deref(),
        Some("Plan revised from your feedback; review the changes")
    );
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesized_plan.as_deref() == Some(revised.as_str())
    ));
}

#[test]
fn unusable_directed_feedback_preserves_the_instruction_and_plan_for_retry() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    let original = synthesized_plan_response();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(original.clone());
    }

    let tx = begin_directed_plan_revision_for_test(&mut app);
    tx.send(Ok("I inspected the repository and have suggestions.".into()))
        .unwrap();

    assert!(app.poll_plan_interview_directed_feedback_bg());
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::DirectedFeedback
                && state.editor.text().contains("Inspect the router")
                && state.synthesized_plan.as_deref() == Some(original.as_str())
    ));
    assert_eq!(
        app.message.as_deref(),
        Some("Directed revision returned no usable plan; your instruction is preserved")
    );
}

#[test]
fn dismissing_directed_feedback_discards_its_late_revision() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    let original = synthesized_plan_response();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(original.clone());
    }

    let tx = begin_directed_plan_revision_for_test(&mut app);
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Review
    ));

    let revised = original.replace("Implement the feature", "Update src/handlers/router.rs");
    tx.send(Ok(revised)).unwrap();
    assert!(app.poll_plan_interview_directed_feedback_bg());
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesized_plan.as_deref() == Some(original.as_str())
    ));
}

#[test]
fn isolated_investigation_merges_findings_then_returns_to_review() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    let original = synthesized_plan_response();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(original.clone());
    }

    let tx = begin_plan_investigation_for_test(&mut app);
    let revised = original.replace("Implement the feature", "Update src/app/feature_ops.rs");
    tx.send(Ok(investigation_outcome(revised.clone()))).unwrap();

    assert!(app.poll_plan_interview_investigation_bg());
    assert!(app.plan_interview_investigation_bg.is_none());
    assert_eq!(
        app.message.as_deref(),
        Some("Investigation findings merged into the draft; review the changes")
    );
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesized_plan.as_deref() == Some(revised.as_str())
    ));
}

#[test]
fn partly_failed_isolated_investigation_still_merges_what_completed() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    let original = synthesized_plan_response();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(original.clone());
    }

    let tx = begin_plan_investigation_for_test(&mut app);
    let revised = original.replace("Implement the feature", "Update src/app/feature_ops.rs");
    tx.send(Ok(crate::plan_interview::PlanInvestigationOutcome {
        merge_response: revised.clone(),
        failed_focuses: vec!["Trace the notification hooks.".into()],
    }))
    .unwrap();

    assert!(app.poll_plan_interview_investigation_bg());
    assert_eq!(
        app.message.as_deref(),
        Some("Investigation merged with 1 focus(es) unresearched; review the changes")
    );
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesized_plan.as_deref() == Some(revised.as_str())
    ));
}

#[test]
fn pasting_into_the_investigation_focus_editor_inserts_text() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(synthesized_plan_response());
        assert!(state.begin_investigation());
    }

    crate::handlers::handle_paste(&mut app, "Trace the session launch boundary.").unwrap();

    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Investigation
                && state.editor.text() == "Trace the session launch boundary."
                && state.edit_sync_to_cursor
    ));
}

#[test]
fn failed_isolated_investigation_preserves_focus_and_plan_for_retry() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    let original = synthesized_plan_response();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(original.clone());
    }

    let tx = begin_plan_investigation_for_test(&mut app);
    tx.send(Err(anyhow::anyhow!("investigator failed")))
        .unwrap();

    assert!(app.poll_plan_interview_investigation_bg());
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Investigation
                && state.editor.text().contains("Trace the session launch boundary")
                && state.synthesized_plan.as_deref() == Some(original.as_str())
    ));
    assert_eq!(
        app.message.as_deref(),
        Some("Isolated investigation failed; your research request is preserved")
    );
}

#[test]
fn dismissing_isolated_investigation_discards_its_late_revision() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    let original = synthesized_plan_response();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(original.clone());
    }

    let tx = begin_plan_investigation_for_test(&mut app);
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Review
    ));

    let revised = original.replace("Implement the feature", "Update src/app/feature_ops.rs");
    tx.send(Ok(investigation_outcome(revised))).unwrap();
    assert!(app.poll_plan_interview_investigation_bg());
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesized_plan.as_deref() == Some(original.as_str())
    ));
}

#[test]
fn agent_review_is_advisory_and_leaves_the_plan_untouched() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(synthesized_plan_response());
    }

    let tx = begin_plan_critique_for_test(&mut app);
    tx.send(Ok(plan_critique_response())).unwrap();

    assert!(app.poll_plan_interview_critique_bg());
    assert!(app.plan_interview_critique_bg.is_none());
    let expected_plan = synthesized_plan_response();
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Critique
                && state.critique.as_deref() == Some(plan_critique_response().as_str())
                // The whole point of the action: the reviewed plan is the
                // plan the user still has.
                && state.synthesized_plan.as_deref() == Some(expected_plan.as_str())
    ));

    // Leaving the review returns to that same plan.
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && !state.abort_confirmation
                && state.synthesized_plan.as_deref() == Some(expected_plan.as_str())
    ));
}

#[test]
fn unusable_agent_review_returns_to_the_plan_with_a_notice() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(synthesized_plan_response());
    }

    let tx = begin_plan_critique_for_test(&mut app);
    tx.send(Ok("I cannot help with that.".to_string())).unwrap();

    assert!(app.poll_plan_interview_critique_bg());
    assert_eq!(
        app.message.as_deref(),
        Some("Plan review returned no usable analysis")
    );
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.critique.is_none()
                && state.synthesized_plan.as_deref()
                    == Some(synthesized_plan_response().as_str())
    ));
}

#[test]
fn a_failed_agent_review_call_is_reported_separately_from_bad_output() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(synthesized_plan_response());
    }

    let tx = begin_plan_critique_for_test(&mut app);
    tx.send(Err(anyhow::anyhow!("claude exited with status 1")))
        .unwrap();

    assert!(app.poll_plan_interview_critique_bg());
    // A call that never ran and a call that answered off-contract need
    // different fixes, so they must not share one catch-all message.
    assert_eq!(
        app.message.as_deref(),
        Some("Plan review failed; the plan is unchanged")
    );
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review && state.critique.is_none()
    ));
}

#[test]
fn dismissing_an_in_flight_agent_review_keeps_its_late_result_without_reopening_it() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(synthesized_plan_response());
    }

    let tx = begin_plan_critique_for_test(&mut app);
    // Esc during the review must return to the plan, not open the
    // abort-the-whole-interview confirmation that would risk the plan.
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review && !state.abort_confirmation
    ));

    tx.send(Ok(plan_critique_response())).unwrap();
    app.poll_plan_interview_critique_bg();

    // The call was already paid for: the result is kept where `a` can reach it,
    // but the user is not yanked back into a screen they just dismissed.
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.critique.as_deref() == Some(plan_critique_response().as_str())
    ));

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('a'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Critique
    ));
}

#[test]
fn a_dismissed_agent_review_reopens_instead_of_paying_for_a_second_call() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(synthesized_plan_response());
    }
    // No harness: any path that actually spends tokens would bail out here
    // with a notice instead of re-opening what is already in hand.
    force_plan_interview_raw_fallback(&mut app);

    let tx = begin_plan_critique_for_test(&mut app);
    tx.send(Ok(plan_critique_response())).unwrap();
    assert!(app.poll_plan_interview_critique_bg());
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('a'))).unwrap();

    assert_eq!(app.message, None);
    assert!(app.plan_interview_critique_bg.is_none());
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Critique
                && state.critique.as_deref() == Some(plan_critique_response().as_str())
    ));
}

#[test]
fn a_stale_agent_review_result_is_dropped_rather_than_kept_against_a_new_plan() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(synthesized_plan_response());
    }

    let tx = begin_plan_critique_for_test(&mut app);
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();
    // The plan moves on while the dismissed review is still running.
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis("# Plan: replaced\n\n## Goal\nSomething else.\n".into());
    }

    tx.send(Ok(plan_critique_response())).unwrap();
    app.poll_plan_interview_critique_bg();

    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            // The findings describe a draft the user no longer has.
            if state.critique.is_none()
                && state.synthesized_plan.as_deref()
                    == Some("# Plan: replaced\n\n## Goal\nSomething else.\n")
    ));
}

#[test]
fn a_second_agent_review_is_not_started_while_the_first_is_still_running() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(synthesized_plan_response());
    }

    let _tx = begin_plan_critique_for_test(&mut app);
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();

    // Dismissing leaves the worker running; `a` must not spend a second time
    // for the analysis already on its way.
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('a'))).unwrap();

    assert_eq!(app.message.as_deref(), Some("Plan review still running"));
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Review
    ));
}

#[test]
fn a_revision_that_cannot_run_keeps_the_feedback_and_the_plan() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(synthesized_plan_response());
    }
    force_plan_interview_raw_fallback(&mut app);

    let tx = begin_plan_critique_for_test(&mut app);
    tx.send(Ok(plan_critique_response())).unwrap();
    assert!(app.poll_plan_interview_critique_bg());

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('r'))).unwrap();

    // No harness here, so the revision never runs. Consuming the feedback
    // anyway would throw away the review the user asked to act on, leaving
    // "revise with this" with nothing behind it.
    assert_eq!(
        app.message.as_deref(),
        Some("No headless-capable harness available; the plan and its review are unchanged")
    );
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.revision_critique.as_deref()
                    == Some(plan_critique_response().as_str())
                && state.critique.as_deref() == Some(plan_critique_response().as_str())
                && state.synthesized_plan.as_deref()
                    == Some(synthesized_plan_response().as_str())
    ));

    // And the review itself is still one keypress away.
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('a'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Critique
    ));
}

#[test]
fn editing_the_plan_drops_a_review_of_the_superseded_draft() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(synthesized_plan_response());
    }

    let tx = begin_plan_critique_for_test(&mut app);
    tx.send(Ok(plan_critique_response())).unwrap();
    assert!(app.poll_plan_interview_critique_bg());
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('e'))).unwrap();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.editor = crate::editor::TextEditor::new("# Plan: edited".into());
    }
    crate::handlers::handle_plan_interview_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
    )
    .unwrap();

    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesized_plan.as_deref() == Some("# Plan: edited\n")
                // The findings described the draft the user just replaced.
                && state.critique.is_none()
    ));
}

#[test]
fn plan_interview_done_without_ai_consent_uses_raw_fallback_without_headless_work() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.brief = "A useful feature".into();
        state.phase = PlanInterviewPhase::Done;
        assert!(!state.ai_followups_opted_in);
        assert!(!state.synthesis_requested);
    } else {
        panic!("expected plan interview mode");
    }

    app.continue_plan_interview_after_done().unwrap();

    assert!(app.plan_interview_ai_bg.is_none());
    assert!(app.plan_interview_synthesis_bg.is_none());
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state
                    .synthesized_plan
                    .as_deref()
                    .is_some_and(|plan| plan.contains("## Feature brief\n\nA useful feature"))
    ));
}

#[test]
fn ctrl_f_from_the_brief_is_a_brief_only_synthesis_fast_path() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    force_plan_interview_raw_fallback(&mut app);

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.questions.clear();
        state.answers.clear();
        state.editor = crate::editor::TextEditor::new("Plan directly from this brief.".into());
    } else {
        panic!("expected plan interview mode");
    }

    crate::handlers::handle_plan_interview_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL),
    )
    .unwrap();

    assert!(app.plan_interview_ai_bg.is_none());
    assert!(app.plan_interview_synthesis_bg.is_none());
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesis_requested
                && state.questions.is_empty()
                && state.synthesized_plan.as_deref()
                    == Some("# Plan: planned-feature\n\n## Feature brief\n\nPlan directly from this brief.\n")
    ));
}

#[test]
fn poll_plan_interview_synthesis_bg_pauses_for_review_then_accepts() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();

    let workdir = match &mut app.mode {
        AppMode::PlanInterview(state) => {
            state.brief = "Ship a useful feature".into();
            state.begin_synthesis(450);
            state.pending_launch.as_ref().unwrap().workdir.clone()
        }
        _ => panic!("expected plan interview mode"),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_synthesis_bg = Some(rx);
    tx.send(Ok(synthesized_plan_response())).unwrap();

    assert!(app.poll_plan_interview_synthesis_bg());

    assert!(app.plan_interview_synthesis_bg.is_none());
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesized_plan.as_deref()
                    == Some(synthesized_plan_response().as_str())
    ));
    assert!(!workdir.join("AMF_PLAN.md").exists());
    assert!(
        !app.store.projects[0]
            .features
            .iter()
            .any(|f| f.name == "planned-feature" && !f.pending_worktree_script)
    );

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert_eq!(
        std::fs::read_to_string(workdir.join("AMF_PLAN.md")).unwrap(),
        synthesized_plan_response()
    );
    assert!(
        app.store.projects[0]
            .features
            .iter()
            .any(|f| f.name == "planned-feature" && !f.pending_worktree_script)
    );
    // Accepting lands in the launched session's composer with an editable
    // kickoff prompt — seeded, never submitted.
    match &app.mode {
        AppMode::Compose(state) => {
            let seed = state.editor.text();
            assert!(seed.contains("AMF_PLAN.md"));
            assert!(seed.contains("decisions are settled"));
            assert_eq!(state.view.feature_name, "planned-feature");
        }
        _ => panic!("expected the composer to be seeded"),
    }
}

#[test]
fn poll_plan_interview_synthesis_bg_uses_raw_fallback_for_incomplete_markdown() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();

    let (workdir, first_question) = match &mut app.mode {
        AppMode::PlanInterview(state) => {
            state.brief = "Fallback brief".into();
            // One answered question and the rest skipped: the fallback must
            // carry the answer and drop every question the user passed over.
            state.answers[0] = Some("Answered this one".into());
            state.begin_synthesis(300);
            (
                state.pending_launch.as_ref().unwrap().workdir.clone(),
                state.questions[0].text.clone(),
            )
        }
        _ => panic!("expected plan interview mode"),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_synthesis_bg = Some(rx);
    tx.send(Ok("# Plan: incomplete".into())).unwrap();

    assert!(app.poll_plan_interview_synthesis_bg());

    let plan = match &app.mode {
        AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::Review => {
            state.synthesized_plan.as_deref().unwrap()
        }
        _ => panic!("expected raw fallback at the review gate"),
    };
    assert!(plan.contains("## Feature brief\n\nFallback brief"));
    assert!(plan.contains("## Q&A"));
    assert!(plan.contains(&format!("### {first_question}\n\nAnswered this one")));
    assert!(!plan.contains("_Skipped._"));
    assert!(!workdir.join("AMF_PLAN.md").exists());
    assert!(
        app.debug_log
            .entries()
            .iter()
            .any(|entry| entry.message.contains("incomplete markdown"))
    );
}

#[test]
fn poll_plan_interview_synthesis_bg_defers_while_abort_confirmation_is_open() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.begin_synthesis(300);
        state.abort_confirmation = true;
    } else {
        panic!("expected plan interview mode");
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_synthesis_bg = Some(rx);
    tx.send(Ok(synthesized_plan_response())).unwrap();

    assert!(!app.poll_plan_interview_synthesis_bg());
    assert!(app.plan_interview_synthesis_bg.is_some());
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::SynthesisLoading
                && state.abort_confirmation
    ));

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.abort_confirmation = false;
    }
    assert!(app.poll_plan_interview_synthesis_bg());
    assert!(app.plan_interview_synthesis_bg.is_none());
    assert!(matches!(
        app.mode,
        AppMode::PlanInterview(ref state) if state.phase == PlanInterviewPhase::Review
    ));
}

#[test]
fn accepting_review_surfaces_write_failure_and_keeps_generated_plan() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();

    let workdir = match &app.mode {
        AppMode::PlanInterview(state) => state.pending_launch.as_ref().unwrap().workdir.clone(),
        _ => panic!("expected plan interview mode"),
    };
    std::fs::create_dir_all(workdir.parent().unwrap()).unwrap();
    std::fs::write(&workdir, b"not a directory").unwrap();

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.begin_synthesis(300);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_synthesis_bg = Some(rx);
    tx.send(Ok(synthesized_plan_response())).unwrap();

    assert!(app.poll_plan_interview_synthesis_bg());
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Enter)).unwrap();

    let message = app.message.clone().unwrap_or_default();
    assert!(
        message.contains("Failed to accept plan interview"),
        "expected a surfaced failure message, got: {message:?}"
    );
    let expected_plan = synthesized_plan_response();
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesis_attempted
                && state.synthesized_plan.as_deref() == Some(expected_plan.as_str())
                && state.pending_launch.is_some()
    ));
}

#[test]
fn plan_review_can_edit_save_regenerate_fallback_and_open_abort_confirmation() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis(synthesized_plan_response());
        state.ai_harness = Some(None);
    }

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('e'))).unwrap();
    assert!(matches!(
        app.mode,
        AppMode::PlanInterview(ref state) if state.phase == PlanInterviewPhase::Editing
    ));
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.editor = crate::editor::TextEditor::new("# Plan: edited".into());
    }
    crate::handlers::handle_plan_interview_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
    )
    .unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesized_plan.as_deref() == Some("# Plan: edited\n")
    ));

    // With no headless harness available, regeneration keeps the
    // already-reviewed plan instead of discarding the user's edit, and says so
    // rather than looking like an unbound key.
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('r'))).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesized_plan.as_deref() == Some("# Plan: edited\n")
    ));
    assert_eq!(
        app.message.as_deref(),
        Some("No headless-capable harness available; keeping current plan")
    );
    app.message = None;

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review && state.abort_confirmation
    ));
}

#[test]
fn requested_synthesis_without_a_harness_explains_the_raw_fallback() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    force_plan_interview_raw_fallback(&mut app);

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.brief = "Explain the fallback".into();
        state.synthesis_requested = true;
        state.phase = PlanInterviewPhase::Done;
    } else {
        panic!("expected plan interview mode");
    }

    app.continue_plan_interview_after_done().unwrap();

    assert!(app.plan_interview_synthesis_bg.is_none());
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state
                    .synthesized_plan
                    .as_deref()
                    .is_some_and(|plan| plan.contains("## Feature brief\n\nExplain the fallback"))
    ));
    assert_eq!(
        app.message.as_deref(),
        Some("No headless-capable harness available; using the raw Q&A plan")
    );
}

/// No current transition re-enters `Done` after synthesis has been attempted,
/// but if one is ever added it must re-open the plan already paid for instead
/// of starting a second headless pass.
#[test]
fn done_after_synthesis_reopens_the_existing_plan_without_spending_tokens() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.ai_followups_opted_in = true;
        state.skip_ai_rounds = true;
        state.apply_synthesis(synthesized_plan_response());
        assert!(state.synthesis_attempted);
        state.phase = PlanInterviewPhase::Done;
    } else {
        panic!("expected plan interview mode");
    }

    app.continue_plan_interview_after_done().unwrap();

    assert!(app.plan_interview_ai_bg.is_none());
    assert!(app.plan_interview_synthesis_bg.is_none());
    assert!(matches!(
        &app.mode,
        AppMode::PlanInterview(state)
            if state.phase == PlanInterviewPhase::Review
                && state.synthesized_plan.as_deref()
                    == Some(synthesized_plan_response().as_str())
    ));
}

/// Task 11: the plan-interview AI round is a *user-initiated* headless call,
/// so it gates behind the blocking pre-call notice. Rounds fire one at a time
/// (the user answers questions between them, and a poll-triggered round opens
/// the notice and returns), so there is no queue of un-dismissed modals to
/// deadlock on. Declining leaves the interview in a clean, resumable state.
#[test]
fn plan_interview_ai_round_gates_behind_the_pre_call_notice_and_declines_cleanly() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        // A resolved interview harness so the round reaches its gate rather
        // than probing for an installed CLI.
        state.ai_harness = Some(Some(AgentKind::Claude));
        state.ai_followups_opted_in = true;
    } else {
        panic!("expected plan interview mode");
    }

    app.start_next_plan_interview_ai_round().unwrap();

    assert!(
        matches!(&app.mode, AppMode::PromptPrecall(p)
            if p.prompt_id == crate::prompts::PromptId::PlanInterviewRound),
        "the round opens the pre-call notice"
    );
    assert!(
        app.plan_interview_ai_bg.is_none(),
        "nothing dispatched while the notice is up"
    );

    app.precall_cancel();
    assert!(matches!(&app.mode, AppMode::PlanInterview(s)
        if s.phase != PlanInterviewPhase::AiLoading));
    assert!(app.plan_interview_ai_bg.is_none());
    assert!(
        app.precall_cleared.is_none(),
        "no stale clearance left behind"
    );
}

#[test]
fn poll_plan_interview_ai_bg_appends_follow_ups_and_resumes_questions() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();

    // Drop straight into an in-flight AI round, as
    // `start_next_plan_interview_ai_round` would leave it, without spawning
    // a real headless call.
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.begin_ai_round(400);
    } else {
        panic!("expected plan interview mode");
    }

    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_ai_bg = Some(rx);
    let response = "```json\n{\"questions\":[{\"id\":\"retry-policy\",\"text\":\"How should retries behave?\",\"kind\":\"free_text\"}]}\n```".to_string();
    tx.send((1, Ok(response))).unwrap();

    assert!(app.poll_plan_interview_ai_bg());

    assert!(app.plan_interview_ai_bg.is_none());
    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.phase, PlanInterviewPhase::StaticQuestions);
            assert_eq!(state.ai_rounds_completed, 1);
            assert_eq!(state.questions.last().unwrap().id, "retry-policy");
            assert_eq!(state.current_question().unwrap().id, "retry-policy");
        }
        _ => panic!("expected plan interview mode"),
    }
}

#[test]
fn poll_plan_interview_ai_bg_opens_review_after_the_final_round() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    force_plan_interview_raw_fallback(&mut app);

    if let AppMode::PlanInterview(state) = &mut app.mode {
        // Simulate having already spent every round but one.
        state.ai_rounds_completed = crate::plan_interview::MAX_AI_ROUNDS - 1;
        state.begin_ai_round(300);
    } else {
        panic!("expected plan interview mode");
    }

    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_ai_bg = Some(rx);
    tx.send((
        crate::plan_interview::MAX_AI_ROUNDS,
        Ok("```json\n{\"questions\":[]}\n```".to_string()),
    ))
    .unwrap();

    assert!(app.poll_plan_interview_ai_bg());

    assert!(app.plan_interview_ai_bg.is_none());
    assert!(matches!(
        app.mode,
        AppMode::PlanInterview(ref state) if state.phase == PlanInterviewPhase::Review
    ));
    assert!(
        !app.store.projects[0]
            .features
            .iter()
            .any(|f| f.name == "planned-feature" && !f.pending_worktree_script)
    );
}

#[test]
fn poll_plan_interview_ai_bg_discards_a_result_that_arrives_after_navigating_away() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.begin_ai_round(200);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_ai_bg = Some(rx);
    // The user aborted out of the interview while the round was in flight.
    app.mode = AppMode::Normal;
    tx.send((
        1,
        Ok("```json\n{\"questions\":[{\"id\":\"late\",\"text\":\"Late?\",\"kind\":\"free_text\"}]}\n```".to_string()),
    ))
    .unwrap();

    assert!(!app.poll_plan_interview_ai_bg());

    assert!(app.plan_interview_ai_bg.is_none());
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn poll_plan_interview_ai_bg_defers_while_abort_confirmation_is_open() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    force_plan_interview_raw_fallback(&mut app);

    if let AppMode::PlanInterview(state) = &mut app.mode {
        // Simulate having already spent every round but one, so applying
        // the queued result would complete the interview and launch the
        // feature if the guard didn't hold.
        state.ai_rounds_completed = crate::plan_interview::MAX_AI_ROUNDS - 1;
        state.begin_ai_round(300);
        // The user pressed Esc during AiLoading, which sets this without
        // changing `phase`.
        state.abort_confirmation = true;
    } else {
        panic!("expected plan interview mode");
    }

    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_ai_bg = Some(rx);
    tx.send((
        crate::plan_interview::MAX_AI_ROUNDS,
        Ok("```json\n{\"questions\":[]}\n```".to_string()),
    ))
    .unwrap();

    // While the abort dialog is open, the result must not be applied.
    assert!(!app.poll_plan_interview_ai_bg());
    assert!(app.plan_interview_ai_bg.is_some());
    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.phase, PlanInterviewPhase::AiLoading);
            assert_eq!(
                state.ai_rounds_completed,
                crate::plan_interview::MAX_AI_ROUNDS - 1
            );
            assert!(state.abort_confirmation);
        }
        _ => panic!("expected plan interview mode"),
    }
    assert!(matches!(app.mode, AppMode::PlanInterview(_)));

    // Esc (resume) clears the flag; the deferred result is picked up on the
    // very next poll, exactly as if the dialog had never appeared.
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.abort_confirmation = false;
    }
    assert!(app.poll_plan_interview_ai_bg());
    assert!(app.plan_interview_ai_bg.is_none());
    assert!(matches!(
        app.mode,
        AppMode::PlanInterview(ref state) if state.phase == PlanInterviewPhase::Review
    ));
    assert!(
        !app.store.projects[0]
            .features
            .iter()
            .any(|f| f.name == "planned-feature" && !f.pending_worktree_script)
    );
}

#[test]
fn final_ai_round_waits_for_accept_before_surface_write_failure() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    force_plan_interview_raw_fallback(&mut app);

    let workdir = match &app.mode {
        AppMode::PlanInterview(state) => state.pending_launch.as_ref().unwrap().workdir.clone(),
        _ => panic!("expected plan interview mode"),
    };
    // A plain file where `write_plan_file` needs to create a `.claude`
    // directory forces `complete_plan_interview` to fail for real, instead
    // of relying on the previous silent-discard behavior as a baseline.
    // The workdir's own parent tree was never created (the launch above
    // only touches the store/mock tmux), so recreate it before shadowing
    // `workdir` itself with a file.
    std::fs::create_dir_all(workdir.parent().unwrap()).unwrap();
    std::fs::write(&workdir, b"not a directory").unwrap();

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.ai_rounds_completed = crate::plan_interview::MAX_AI_ROUNDS - 1;
        state.begin_ai_round(200);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_ai_bg = Some(rx);
    tx.send((
        crate::plan_interview::MAX_AI_ROUNDS,
        Ok("```json\n{\"questions\":[]}\n```".to_string()),
    ))
    .unwrap();

    assert!(app.poll_plan_interview_ai_bg());
    assert!(matches!(
        app.mode,
        AppMode::PlanInterview(ref state) if state.phase == PlanInterviewPhase::Review
    ));
    assert!(app.message.is_none());

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Enter)).unwrap();

    let message = app.message.clone().unwrap_or_default();
    assert!(
        message.contains("Failed to accept plan interview"),
        "expected a surfaced failure message, got: {message:?}"
    );
    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert!(
                state.pending_launch.is_some(),
                "the pending launch must survive a write failure so the user can retry or abort"
            );
        }
        _ => panic!("a plan-file write failure must keep the interview open"),
    }
}

#[test]
fn disconnected_ai_worker_waits_for_accept_before_surface_write_failure() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    force_plan_interview_raw_fallback(&mut app);

    let workdir = match &app.mode {
        AppMode::PlanInterview(state) => state.pending_launch.as_ref().unwrap().workdir.clone(),
        _ => panic!("expected plan interview mode"),
    };
    std::fs::create_dir_all(workdir.parent().unwrap()).unwrap();
    std::fs::write(&workdir, b"not a directory").unwrap();

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.begin_ai_round(200);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_ai_bg = Some(rx);
    drop(tx);

    assert!(app.poll_plan_interview_ai_bg());
    assert!(matches!(
        app.mode,
        AppMode::PlanInterview(ref state) if state.phase == PlanInterviewPhase::Review
    ));
    assert!(app.message.is_none());

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Enter)).unwrap();

    let message = app.message.clone().unwrap_or_default();
    assert!(
        message.contains("Failed to accept plan interview"),
        "expected a surfaced failure message, got: {message:?}"
    );
    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert!(
                state.pending_launch.is_some(),
                "the pending launch must survive a write failure so the user can retry or abort"
            );
        }
        _ => panic!("a plan-file write failure must keep the interview open"),
    }
}

#[test]
fn poll_plan_interview_ai_bg_treats_a_dropped_worker_as_round_exhaustion() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    force_plan_interview_raw_fallback(&mut app);

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.begin_ai_round(200);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.plan_interview_ai_bg = Some(rx);
    drop(tx);

    assert!(app.poll_plan_interview_ai_bg());

    assert!(app.plan_interview_ai_bg.is_none());
    assert!(matches!(
        app.mode,
        AppMode::PlanInterview(ref state) if state.phase == PlanInterviewPhase::Review
    ));
}

/// The key under which a feature-creation interview's draft is filed, before
/// the feature (and its id) exists.
const PENDING_INTERVIEW_KEY: &str = "pending:my-project/planned-feature";

/// Walk the brief and the first question, so there is something worth resuming.
fn answer_two_plan_interview_steps(app: &mut App) {
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.editor = crate::editor::TextEditor::new("Persist my answers.".into());
    }
    crate::handlers::handle_plan_interview_key(app, ke(KeyCode::Enter)).unwrap();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.editor = crate::editor::TextEditor::new("Only the TUI.".into());
    }
    crate::handlers::handle_plan_interview_key(app, ke(KeyCode::Enter)).unwrap();
}

#[test]
fn plan_interview_answers_are_saved_as_a_resumable_draft() {
    let (mut app, _store_file, _repo, _db_dir) = app_with_deferred_plan_interview_and_db();
    answer_two_plan_interview_steps(&mut app);

    let draft = app
        .db
        .as_ref()
        .unwrap()
        .plan_interview_draft(PENDING_INTERVIEW_KEY)
        .unwrap()
        .expect("answers must be saved as they are given");
    assert_eq!(draft.brief, "Persist my answers.");
    assert_eq!(draft.feature_name, "planned-feature");
    assert_eq!(draft.answer_for("scope"), Some("Only the TUI."));
    assert!(draft.plan.is_none());
}

/// The point of the draft: abandoning the interview and coming back to create
/// the same feature must not cost the user their answers.
#[test]
fn re_entering_an_abandoned_plan_interview_offers_to_resume_it() {
    let (mut app, _store_file, repo, _db_dir) = app_with_deferred_plan_interview_and_db();
    answer_two_plan_interview_steps(&mut app);

    // Abandon: abort, then cancel the feature entirely.
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('n'))).unwrap();
    assert!(matches!(app.mode, AppMode::Normal));

    app.finish_feature_launch(PreparedFeatureLaunch {
        project_name: "my-project".into(),
        branch: "planned-feature".into(),
        workdir: repo.path().join(".worktrees/planned-feature"),
        is_worktree: true,
        mode: VibeMode::default(),
        review: false,
        plan_mode: true,
        quick_plan: false,
        agent: AgentKind::Claude,
        create_terminal: false,
        session_name: "Claude 1".into(),
        enable_chrome: false,
        remote_control: false,
        steering_enabled: false,
        hook_succeeded: None,
        startup_prompt: None,
        todo_origin: None,
    })
    .unwrap();

    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.phase, PlanInterviewPhase::ResumePrompt);
            assert_eq!(state.interview_key, PENDING_INTERVIEW_KEY);
            // Nothing is restored until the user chooses to resume.
            assert!(state.brief.is_empty());
        }
        _ => panic!("expected the resume prompt for the saved draft"),
    }

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('r'))).unwrap();

    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.phase, PlanInterviewPhase::StaticQuestions);
            assert_eq!(state.brief, "Persist my answers.");
            assert_eq!(state.answers[0].as_deref(), Some("Only the TUI."));
            // Resumed at the first question still unanswered.
            assert_eq!(state.question_index, 1);
        }
        _ => panic!("resuming must restore the saved interview"),
    }
}

#[test]
fn discarding_the_offered_draft_deletes_the_saved_row() {
    let (mut app, _store_file, _repo, _db_dir) = app_with_deferred_plan_interview_and_db();
    answer_two_plan_interview_steps(&mut app);
    if let AppMode::PlanInterview(state) = &mut app.mode {
        let draft = app
            .db
            .as_ref()
            .unwrap()
            .plan_interview_draft(PENDING_INTERVIEW_KEY)
            .unwrap()
            .unwrap();
        state.offer_resume(draft);
    }

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('d'))).unwrap();

    assert!(
        app.db
            .as_ref()
            .unwrap()
            .plan_interview_draft(PENDING_INTERVIEW_KEY)
            .unwrap()
            .is_none(),
        "discarding must remove the stored draft, not just hide it"
    );
    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.phase, PlanInterviewPhase::Brief);
            assert!(state.brief.is_empty());
        }
        _ => panic!("discarding must start the interview over"),
    }
}

/// On accept the transcript moves off the pending key and onto the feature the
/// launch just created — that id is where a later re-run looks for it.
#[test]
fn accepting_a_plan_files_the_transcript_under_the_created_feature_id() {
    let (mut app, _store_file, _repo, _db_dir) = app_with_deferred_plan_interview_and_db();
    force_plan_interview_raw_fallback(&mut app);
    answer_two_plan_interview_steps(&mut app);
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis("# Plan: planned-feature\n".into());
    }

    app.complete_plan_interview().unwrap();

    let db = app.db.as_ref().unwrap();
    assert!(
        db.plan_interview_draft(PENDING_INTERVIEW_KEY)
            .unwrap()
            .is_none(),
        "the accepted draft must not be offered for resume again"
    );
    let feature_id = app.store.projects[0]
        .features
        .iter()
        .find(|feature| feature.name == "planned-feature")
        .map(|feature| feature.id.clone())
        .expect("accept launches the feature");
    let transcript = db
        .plan_interview_final(&feature_id)
        .unwrap()
        .expect("accept must save the transcript under the feature's id");
    assert_eq!(
        transcript.plan.as_deref(),
        Some("# Plan: planned-feature\n")
    );
    assert_eq!(transcript.answer_for("scope"), Some("Only the TUI."));
}

/// `plan_interviews.feature_id` has no foreign key, so deletion is explicit.
/// Both keys a feature's interviews can live under have to be cleared.
#[test]
fn deleting_a_feature_drops_its_stored_interviews() {
    let (mut app, _store_file, _repo, _db_dir) = app_with_deferred_plan_interview_and_db();
    force_plan_interview_raw_fallback(&mut app);
    answer_two_plan_interview_steps(&mut app);
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.apply_synthesis("# Plan: planned-feature\n".into());
    }
    app.complete_plan_interview().unwrap();

    let feature_id = app.store.projects[0]
        .features
        .iter()
        .find(|feature| feature.name == "planned-feature")
        .map(|feature| feature.id.clone())
        .unwrap();
    // A second, abandoned interview for the same feature name still sits on the
    // pending key; deletion has to reach that too.
    app.db
        .as_ref()
        .unwrap()
        .save_plan_interview(&crate::db::plan_interviews::PlanInterviewRecord {
            feature_id: PENDING_INTERVIEW_KEY.into(),
            feature_name: "planned-feature".into(),
            brief: "Abandoned second pass.".into(),
            ..Default::default()
        })
        .unwrap();

    app.delete_plan_interviews_for_deleted_feature(
        "my-project",
        "planned-feature",
        &Some(feature_id.clone()),
    );

    let db = app.db.as_ref().unwrap();
    assert!(db.plan_interview_final(&feature_id).unwrap().is_none());
    assert!(db.plan_interview_draft(&feature_id).unwrap().is_none());
    assert!(
        db.plan_interview_draft(PENDING_INTERVIEW_KEY)
            .unwrap()
            .is_none()
    );
}

/// `app_on_selected_feature` plus a real SQLite database, so the re-run path
/// reads an actual stored transcript. The extra `TempDir` holds the database
/// file and must outlive the app.
fn app_on_selected_feature_with_db() -> (App, tempfile::NamedTempFile, TempDir, TempDir) {
    let db_dir = TempDir::new().unwrap();
    let (mut app, store_file, repo) = app_on_selected_feature();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    (app, store_file, repo, db_dir)
}

/// The transcript a previously accepted plan leaves behind for `feat-1`: one
/// built-in question the current bank still asks, and one AI follow-up it
/// cannot contain.
fn save_accepted_transcript(app: &App) {
    use crate::db::plan_interviews::{PlanInterviewRecord, PlanInterviewStage};
    use crate::plan_interview::{PlanQuestionKind, QuestionSource};

    app.db
        .as_ref()
        .unwrap()
        .save_plan_interview(&PlanInterviewRecord {
            feature_id: "feat-1".into(),
            stage: PlanInterviewStage::Final,
            feature_name: "my-feat".into(),
            brief: "Tighten the sidebar.".into(),
            questions: vec![
                crate::plan_interview::PlanQuestion {
                    id: "scope".into(),
                    text: "What is in scope?".into(),
                    kind: PlanQuestionKind::FreeText,
                    source: QuestionSource::Builtin,
                    optional: true,
                },
                crate::plan_interview::PlanQuestion {
                    id: "cache-invalidation".into(),
                    text: "When is the preview invalidated?".into(),
                    kind: PlanQuestionKind::FreeText,
                    source: QuestionSource::Ai { round: 1 },
                    optional: true,
                },
            ],
            answers: vec![Some("Sidebar only.".into()), Some("On every save.".into())],
            plan: Some("# Plan: my-feat\n".into()),
            ai_rounds_completed: 1,
            ..Default::default()
        })
        .unwrap();
}

/// The point of the re-run: planning a feature again starts from the answers
/// behind the plan already accepted for it, not from a blank interview.
#[test]
fn re_running_the_interview_pre_fills_the_accepted_answers() {
    let (mut app, _store_file, _repo, _db_dir) = app_on_selected_feature_with_db();
    save_accepted_transcript(&app);

    app.start_plan_interview_for_selected_feature();

    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.phase, PlanInterviewPhase::Brief);
            assert_eq!(state.brief, "Tighten the sidebar.");
            // The brief is in the editor too, so Enter keeps it.
            assert_eq!(state.editor.text(), "Tighten the sidebar.");
            let scope = state
                .questions
                .iter()
                .position(|question| question.id == "scope")
                .expect("the built-in bank still asks about scope");
            assert_eq!(state.answers[scope].as_deref(), Some("Sidebar only."));
            // The previous run's AI question is not in the current bank, so it is
            // carried onto the end with the answer it collected.
            let ai = state
                .questions
                .iter()
                .position(|question| question.id == "cache-invalidation")
                .expect("a paid-for AI question must not be dropped on a re-run");
            assert_eq!(state.answers[ai].as_deref(), Some("On every save."));
            // Adaptive rounds are not carried: the re-run gets its own opt-in
            // and its own budget.
            assert_eq!(state.ai_rounds_completed, 0);
            assert!(!state.ai_followups_opted_in);
        }
        _ => panic!("expected plan interview mode"),
    }
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("pre-filled")
    );
}

/// Per-question keep/change: Enter keeps the pre-filled answer, typing changes
/// it, and Ctrl+R puts the previous one back.
#[test]
fn a_re_run_keeps_changes_or_restores_each_answer() {
    let (mut app, _store_file, _repo, _db_dir) = app_on_selected_feature_with_db();
    save_accepted_transcript(&app);
    app.start_plan_interview_for_selected_feature();

    // Brief: pre-filled and reported as kept, and Enter carries it forward.
    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.prior_answer_state(), Some(PriorAnswerState::Kept));
        }
        _ => panic!("expected plan interview mode"),
    }
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Enter)).unwrap();

    // First question is "scope", pre-filled from the transcript.
    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.phase, PlanInterviewPhase::StaticQuestions);
            assert_eq!(state.questions[state.question_index].id, "scope");
            assert_eq!(state.editor.text(), "Sidebar only.");
            assert_eq!(state.prior_answer_state(), Some(PriorAnswerState::Kept));
        }
        _ => panic!("expected the first question"),
    }

    // Typing is a change, and the previous answer stays restorable.
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('!'))).unwrap();
    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.editor.text(), "Sidebar only.!");
            assert_eq!(state.prior_answer_state(), Some(PriorAnswerState::Changed));
        }
        _ => panic!("expected the first question"),
    }

    crate::handlers::handle_plan_interview_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
    )
    .unwrap();
    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.editor.text(), "Sidebar only.");
            assert_eq!(state.prior_answer_state(), Some(PriorAnswerState::Kept));
        }
        _ => panic!("expected the first question"),
    }
    assert!(app.message.is_none());

    // Enter records the kept answer and moves on.
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Enter)).unwrap();
    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.answers[0].as_deref(), Some("Sidebar only."));
            assert_eq!(state.question_index, 1);
            // The next built-in question was never answered before, so there is
            // nothing to keep and nothing to restore.
            assert_eq!(state.prior_answer_state(), None);
        }
        _ => panic!("expected the second question"),
    }

    crate::handlers::handle_plan_interview_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL),
    )
    .unwrap();
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("No previous answer")
    );
}

/// Clearing a pre-filled answer skips the question, which is how a re-run drops
/// an answer that no longer applies.
#[test]
fn clearing_a_pre_filled_answer_records_it_as_skipped() {
    let (mut app, _store_file, _repo, _db_dir) = app_on_selected_feature_with_db();
    save_accepted_transcript(&app);
    app.start_plan_interview_for_selected_feature();
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Enter)).unwrap();

    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.editor = crate::editor::TextEditor::new(String::new());
        assert_eq!(state.prior_answer_state(), Some(PriorAnswerState::Cleared));
    } else {
        panic!("expected the first question");
    }
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::PlanInterview(state) => assert_eq!(state.answers[0], None),
        _ => panic!("expected the second question"),
    }
}

/// A stale draft and an accepted transcript can both exist for one feature.
/// Discarding the draft must not also throw away the accepted answers it was
/// revising.
#[test]
fn discarding_a_draft_on_a_re_run_falls_back_to_the_accepted_answers() {
    use crate::db::plan_interviews::PlanInterviewRecord;

    let (mut app, _store_file, _repo, _db_dir) = app_on_selected_feature_with_db();
    save_accepted_transcript(&app);
    app.db
        .as_ref()
        .unwrap()
        .save_plan_interview(&PlanInterviewRecord {
            feature_id: "feat-1".into(),
            feature_name: "my-feat".into(),
            brief: "Abandoned second pass.".into(),
            ..Default::default()
        })
        .unwrap();

    app.start_plan_interview_for_selected_feature();
    assert!(
        matches!(&app.mode, AppMode::PlanInterview(state) if state.phase == PlanInterviewPhase::ResumePrompt)
    );

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('d'))).unwrap();

    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.phase, PlanInterviewPhase::Brief);
            assert_eq!(state.brief, "Tighten the sidebar.");
            assert_eq!(state.answers[0].as_deref(), Some("Sidebar only."));
        }
        _ => panic!("discarding must fall back to the accepted transcript"),
    }
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .plan_interview_draft("feat-1")
            .unwrap()
            .is_none()
    );
    // The transcript is not the draft: discarding must leave it alone.
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .plan_interview_final("feat-1")
            .unwrap()
            .is_some()
    );
}

/// Resuming a draft on a re-run takes the draft's answers, which are newer than
/// the accepted transcript's.
#[test]
fn resuming_a_draft_on_a_re_run_wins_over_the_accepted_answers() {
    use crate::db::plan_interviews::PlanInterviewRecord;
    use crate::plan_interview::{PlanQuestionKind, QuestionSource};

    let (mut app, _store_file, _repo, _db_dir) = app_on_selected_feature_with_db();
    save_accepted_transcript(&app);
    app.db
        .as_ref()
        .unwrap()
        .save_plan_interview(&PlanInterviewRecord {
            feature_id: "feat-1".into(),
            feature_name: "my-feat".into(),
            brief: "Second pass.".into(),
            questions: vec![crate::plan_interview::PlanQuestion {
                id: "scope".into(),
                text: "What is in scope?".into(),
                kind: PlanQuestionKind::FreeText,
                source: QuestionSource::Builtin,
                optional: true,
            }],
            answers: vec![Some("Sidebar and header.".into())],
            ..Default::default()
        })
        .unwrap();

    app.start_plan_interview_for_selected_feature();
    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Char('r'))).unwrap();

    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.brief, "Second pass.");
            assert_eq!(state.answers[0].as_deref(), Some("Sidebar and header."));
            // The accepted answer is still what Ctrl+R restores.
            assert_eq!(
                state.prior_answers.get("scope").map(String::as_str),
                Some("Sidebar only.")
            );
        }
        _ => panic!("expected the resumed draft"),
    }
}

/// Persistence is a convenience layered over an in-memory flow; without a
/// database the interview still has to work end to end.
#[test]
fn plan_interview_runs_without_a_database() {
    let (mut app, _store_file, _repo) = app_with_deferred_plan_interview();
    assert!(app.db.is_none());

    answer_two_plan_interview_steps(&mut app);

    match &app.mode {
        AppMode::PlanInterview(state) => {
            assert_eq!(state.brief, "Persist my answers.");
            assert_eq!(state.answers[0].as_deref(), Some("Only the TUI."));
        }
        _ => panic!("expected the interview to advance normally without a DB"),
    }
}
