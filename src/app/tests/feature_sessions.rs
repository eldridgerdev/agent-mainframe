use super::support::*;
use crate::app::*;
use crate::automation::CreateProjectRequest;
use crate::extension::ExtensionConfig;
use crate::project::{
    AgentKind, Feature, FeatureSession, Project, SessionKind, tmux_session_name, worktree_name,
};
use crate::token_tracking::{TokenUsageProvider, TokenUsageSource};
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

fn store_with_stopped_agent_session(kind: SessionKind, resume_id: Option<&str>) -> ProjectStore {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    let session = store.projects[0].features[0].add_session_named(kind.clone(), "Agent".into());
    match kind {
        SessionKind::Claude => {
            session.claude_session_id = resume_id.map(str::to_string);
        }
        SessionKind::Codex => {
            if let Some(id) = resume_id {
                session.set_token_usage_source_exact(TokenUsageSource {
                    provider: TokenUsageProvider::Codex,
                    id: id.to_string(),
                });
            }
        }
        SessionKind::Opencode => {
            if let Some(id) = resume_id {
                session.set_token_usage_source_exact(TokenUsageSource {
                    provider: TokenUsageProvider::Opencode,
                    id: id.to_string(),
                });
            }
        }
        _ => {}
    }
    store
}

fn dialog_choices(app: &App) -> Vec<StoppedSessionChoice> {
    match &app.mode {
        AppMode::StoppedSessionDialog(state) => state.choices.clone(),
        _ => panic!("expected the stopped-session dialog"),
    }
}

#[test]
fn enter_on_stopped_agent_session_opens_recovery_dialog() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, Some("claude-resume"));
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(1).return_const(false);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);

    crate::handlers::handle_normal_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    assert_eq!(
        dialog_choices(&app),
        vec![
            StoppedSessionChoice::Resume,
            StoppedSessionChoice::Clear,
            StoppedSessionChoice::PickSession,
            StoppedSessionChoice::Cancel,
        ]
    );
    assert!(matches!(
        app.mode,
        AppMode::StoppedSessionDialog(StoppedSessionDialogState { selected: 0, .. })
    ));
}

#[test]
fn uppercase_s_keeps_the_saved_transcript_picker_for_a_stopped_feature() {
    let store = store_with_stopped_agent_session(SessionKind::Codex, Some("codex-resume"));
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(false);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);

    crate::handlers::handle_normal_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE),
    )
    .unwrap();

    assert!(
        !matches!(app.mode, AppMode::StoppedSessionDialog(_)),
        "`S` must stay on the transcript picker so older sessions on disk \
         remain reachable from a stopped feature"
    );
}

#[test]
fn stopped_session_without_resume_metadata_starts_directly() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, None);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Session(0, 0, 0);

    assert!(
        !app.open_stopped_session_dialog().unwrap(),
        "with no saved ID both choices start the same clear session, so the \
         dialog has nothing to ask"
    );
}

#[test]
fn stopped_session_with_blank_resume_id_starts_directly() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, Some("   "));
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Session(0, 0, 0);

    assert!(!app.open_stopped_session_dialog().unwrap());
}

#[test]
fn pi_session_has_no_recovery_dialog() {
    let store = store_with_stopped_agent_session(SessionKind::Pi, Some("ignored"));
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Session(0, 0, 0);

    assert!(
        !app.open_stopped_session_dialog().unwrap(),
        "Pi cannot resume, so it must keep its plain start path rather than \
         showing a dialog with nothing to resume"
    );
}

#[test]
fn feature_stopped_from_the_dashboard_restarts_without_the_dialog() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, Some("claude-resume"));
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Session(0, 0, 0);
    app.user_stopped_features.insert("feat-1".to_string());

    assert!(
        !app.open_stopped_session_dialog().unwrap(),
        "a deliberate `x` stop should restart and resume in one keypress"
    );
}

#[test]
fn ctrl_c_cancels_the_recovery_dialog_instead_of_clearing_the_saved_id() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, Some("keep-me"));
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(1).return_const(false);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);
    assert!(app.open_stopped_session_dialog().unwrap());

    crate::handlers::handle_stopped_session_dialog_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
    )
    .unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(
        app.store.projects[0].features[0].sessions[0].claude_session_id,
        Some("keep-me".to_string()),
        "Ctrl+C must not discard the saved resume ID"
    );
}

#[test]
fn pick_session_choice_leaves_the_dialog_for_the_transcript_picker() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, Some("claude-resume"));
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(1).return_const(false);
    // No launch is expected: the picker owns starting the feature.
    tmux.expect_create_session_with_window().times(0);
    tmux.expect_launch_claude().times(0);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);
    assert!(app.open_stopped_session_dialog().unwrap());

    crate::handlers::handle_stopped_session_dialog_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::NONE),
    )
    .unwrap();

    // The picker itself may find no transcripts in a test environment; what
    // matters is that the dialog handed off instead of launching a harness.
    assert!(matches!(
        app.mode,
        AppMode::Normal | AppMode::ClaudeSessionPicker(_)
    ));
}

#[test]
fn running_agent_session_still_opens_directly() {
    let mut store = store_with_stopped_agent_session(SessionKind::Claude, Some("resume-me"));
    store.projects[0].features[0].status = ProjectStatus::Idle;
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(2).return_const(true);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);

    crate::handlers::handle_normal_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    assert!(
        matches!(app.mode, AppMode::Viewing(_) | AppMode::Compose(_)),
        "running sessions should keep the existing open behavior"
    );
}

#[test]
fn uppercase_s_for_running_agent_keeps_existing_resume_picker_behavior() {
    let mut store = store_with_stopped_agent_session(SessionKind::Claude, Some("resume-me"));
    store.projects[0].features[0].status = ProjectStatus::Idle;
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);

    crate::handlers::handle_normal_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE),
    )
    .unwrap();

    assert!(!matches!(app.mode, AppMode::StoppedSessionDialog(_)));
}

#[test]
fn stopped_session_dialog_cancel_returns_to_dashboard() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, Some("resume-me"));
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::StoppedSessionDialog(StoppedSessionDialogState {
        project_id: "proj-1".into(),
        feature_id: "feat-1".into(),
        session_id: app.store.projects[0].features[0].sessions[0].id.clone(),
        selected: 0,
        choices: vec![
            StoppedSessionChoice::Resume,
            StoppedSessionChoice::Clear,
            StoppedSessionChoice::Cancel,
        ],
        harness_label: "Claude".into(),
    });

    crate::handlers::handle_stopped_session_dialog_key(
        &mut app,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
    )
    .unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn stopped_terminal_session_keeps_existing_start_and_open_behavior() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].add_session_named(SessionKind::Terminal, "Shell".into());
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(1).return_const(false);
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_select_window()
        .times(1)
        .returning(|_, _| Ok(()));
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);

    crate::handlers::handle_normal_key(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();

    assert!(matches!(
        app.mode,
        AppMode::Viewing(ViewState {
            session_kind: SessionKind::Terminal,
            ..
        })
    ));
}

#[test]
fn recovery_resumes_saved_codex_session_and_opens_its_pane() {
    let store = store_with_stopped_agent_session(SessionKind::Codex, Some("codex-resume"));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_for_exists = calls.clone();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists()
        .times(4)
        .returning(move |_| calls_for_exists.fetch_add(1, Ordering::SeqCst) == 3);
    tmux.expect_check_harness_available()
        .withf(|kind| *kind == AgentKind::Codex)
        .times(1)
        .returning(|_| Ok(()));
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_run_shell_command()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_codex()
        .withf(|_, window, _, resume_id, _| {
            window == "codex" && resume_id.as_deref() == Some("codex-resume")
        })
        .times(1)
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window()
        .times(1)
        .returning(|_, _| Ok(()));
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);
    app.open_stopped_session_dialog().unwrap();

    app.confirm_stopped_session_choice(StoppedSessionChoice::Resume);

    assert!(matches!(
        app.mode,
        AppMode::Viewing(ViewState {
            session_kind: SessionKind::Codex,
            ..
        })
    ));
    assert_eq!(app.message.as_deref(), Some("Resumed Codex session"));
}

#[test]
fn clear_recovery_launches_without_resume_and_clears_saved_metadata() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, Some("old-session"));
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls_for_exists = calls.clone();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists()
        .times(4)
        .returning(move |_| calls_for_exists.fetch_add(1, Ordering::SeqCst) == 3);
    tmux.expect_check_harness_available()
        .withf(|kind| *kind == AgentKind::Claude)
        .times(1)
        .returning(|_| Ok(()));
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .withf(|_, window, _, resume_id, _| window == "claude" && resume_id.is_none())
        .times(1)
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window()
        .times(1)
        .returning(|_, _| Ok(()));
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);
    app.open_stopped_session_dialog().unwrap();

    app.confirm_stopped_session_choice(StoppedSessionChoice::Clear);

    let session = &app.store.projects[0].features[0].sessions[0];
    assert_eq!(session.claude_session_id, None);
    assert_eq!(session.token_usage_source, None);
    assert_eq!(app.message.as_deref(), Some("Started clear Claude session"));
}

#[test]
fn recovery_rejects_stale_session_selection_safely() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, Some("resume-me"));
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let session_id = app.store.projects[0].features[0].sessions[0].id.clone();
    app.mode = AppMode::StoppedSessionDialog(StoppedSessionDialogState {
        project_id: "proj-1".into(),
        feature_id: "feat-1".into(),
        session_id,
        selected: 0,
        choices: vec![
            StoppedSessionChoice::Resume,
            StoppedSessionChoice::Clear,
            StoppedSessionChoice::Cancel,
        ],
        harness_label: "Claude".into(),
    });
    app.store.projects[0].features[0].sessions.clear();

    app.confirm_stopped_session_choice(StoppedSessionChoice::Resume);

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.message
            .as_deref()
            .is_some_and(|message| message.contains("no longer exists"))
    );
}

#[test]
fn recovery_surfaces_missing_harness_without_creating_tmux() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, Some("resume-me"));
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(2).return_const(false);
    tmux.expect_check_harness_available()
        .times(1)
        .returning(|_| Err(anyhow::anyhow!("claude CLI not found")));
    tmux.expect_create_session_with_window().times(0);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);
    app.open_stopped_session_dialog().unwrap();

    app.confirm_stopped_session_choice(StoppedSessionChoice::Resume);

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.message
            .as_deref()
            .is_some_and(|message| message.contains("claude CLI not found"))
    );
}

#[test]
fn recovery_launch_failure_removes_partial_tmux_session() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, Some("resume-me"));
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(3).return_const(false);
    tmux.expect_check_harness_available()
        .times(1)
        .returning(|_| Ok(()));
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .times(1)
        .returning(|_, _, _, _, _| Err(anyhow::anyhow!("launch failed")));
    tmux.expect_kill_session().times(1).returning(|_| Ok(()));
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);
    app.open_stopped_session_dialog().unwrap();

    app.confirm_stopped_session_choice(StoppedSessionChoice::Resume);

    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.message
            .as_deref()
            .is_some_and(|message| message.contains("launch failed"))
    );
}

#[test]
fn recovery_tmux_creation_failure_does_not_kill_an_unowned_session() {
    let store = store_with_stopped_agent_session(SessionKind::Claude, Some("resume-me"));
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(3).return_const(false);
    tmux.expect_check_harness_available()
        .times(1)
        .returning(|_| Ok(()));
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Err(anyhow::anyhow!("tmux create failed")));
    tmux.expect_kill_session().times(0);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);
    app.open_stopped_session_dialog().unwrap();

    app.confirm_stopped_session_choice(StoppedSessionChoice::Resume);

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.message
            .as_deref()
            .is_some_and(|message| message.contains("tmux create failed"))
    );
}

// ── create_feature validation ─────────────────────────────────

fn app_in_creating_feature_mode(
    store: ProjectStore,
    project_name: &str,
    branch: &str,
    use_worktree: bool,
) -> App {
    use crate::app::state::{CreateFeatureState, CreateFeatureStep};
    let project_repo = store
        .find_project(project_name)
        .map(|p| p.repo.clone())
        .unwrap_or_default();
    let state = CreateFeatureState {
        project_name: project_name.to_string(),
        project_repo,
        todo_origin: None,
        branch: branch.to_string(),
        branch_error: None,
        allowed_agents: AgentKind::ALL.to_vec(),
        feature_presets: Vec::new(),
        step: CreateFeatureStep::Branch,
        agent: AgentKind::default(),
        agent_index: 0,
        mode: VibeMode::default(),
        mode_index: 0,
        mode_focus: 0,
        review: false,
        plan_mode: false,
        quick_plan: false,
        create_terminal: true,
        session_name: "Claude 1".to_string(),
        source_index: 0,
        worktrees: vec![],
        worktree_index: 0,
        worktree_search_active: false,
        worktree_query: String::new(),
        use_worktree,
        enable_chrome: false,
        remote_control: false,
        remote_control_available: true,
        remote_control_block_reason: None,
        steering_enabled: true,
        preset_index: 0,
        task_prompt: String::new(),
        prompt_analysis: analyze_prompt(""),
        prepared_launch: None,
    };
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::CreatingFeature(state);
    app
}

#[test]
fn create_feature_empty_branch_sets_error_no_external_calls() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(
        store,
        "my-project",
        "", // empty branch
        false,
    );
    app.create_feature().unwrap();

    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("cannot be empty"),
        "got: {:?}",
        app.message
    );
}

#[test]
fn create_feature_duplicate_name_sets_error_no_external_calls() {
    let store = store_with_feature(ProjectStatus::Stopped);
    // "my-feat" already exists in the store
    let mut app = app_in_creating_feature_mode(store, "my-project", "my-feat", false);
    app.create_feature().unwrap();

    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("already exists"), "got: {msg}");
}

#[test]
fn create_feature_duplicate_normalized_name_sets_error_no_external_calls() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "My Feat", false);
    app.create_feature().unwrap();

    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("already exists"), "got: {msg}");
}

#[test]
fn create_feature_branch_enter_shows_inline_duplicate_error() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "My Feat", false);

    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Enter).unwrap();

    match &app.mode {
        AppMode::CreatingFeature(state) => {
            assert_eq!(state.step, CreateFeatureStep::Branch);
            assert!(
                state
                    .branch_error
                    .as_deref()
                    .unwrap_or_default()
                    .contains("already exists")
            );
        }
        _ => panic!("expected CreatingFeature mode"),
    }
}

#[test]
fn create_feature_branch_edit_after_error_can_continue() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "My Feat", false);

    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Enter).unwrap();
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.branch = "new-feat".to_string();
    }
    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Char('x')).unwrap();
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.branch.pop();
    }
    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Enter).unwrap();

    match &app.mode {
        AppMode::CreatingFeature(state) => {
            assert_eq!(state.step, CreateFeatureStep::Worktree);
            assert!(state.branch_error.is_none());
        }
        _ => panic!("expected CreatingFeature mode"),
    }
}

#[test]
fn create_feature_branch_enter_allows_same_name_in_different_project() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    let now = Utc::now();
    store.projects.push(Project {
        id: "proj-2".to_string(),
        name: "other-project".to_string(),
        repo: PathBuf::from("/tmp/other-repo"),
        collapsed: false,
        features: vec![],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: false,
    });
    let mut app = app_in_creating_feature_mode(store, "other-project", "my-feat", false);

    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Enter).unwrap();

    match &app.mode {
        AppMode::CreatingFeature(state) => {
            assert_eq!(state.step, CreateFeatureStep::Worktree);
            assert!(state.branch_error.is_none());
        }
        _ => panic!("expected CreatingFeature mode"),
    }
}

#[test]
fn feature_tmux_session_names_are_scoped_by_project() {
    assert_eq!(
        tmux_session_name("Project One", "main"),
        "amf-project-one-main"
    );
    assert_eq!(
        tmux_session_name("Project Two", "main"),
        "amf-project-two-main"
    );
}

#[test]
fn feature_worktree_names_are_scoped_by_project() {
    assert_eq!(worktree_name("Project One", "tt"), "project-one-tt");
    assert_eq!(worktree_name("Project Two", "tt"), "project-two-tt");
}

#[test]
fn create_feature_second_non_worktree_sets_error() {
    let store = store_with_feature(ProjectStatus::Stopped);
    // Existing feature is non-worktree; adding another must fail
    let mut app = app_in_creating_feature_mode(
        store,
        "my-project",
        "other-feat",
        false, // use_worktree = false
    );
    app.create_feature().unwrap();

    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("Only one non-worktree"), "got: {msg}");
}

#[test]
fn create_feature_disallowed_agent_sets_error() {
    let repo = TempDir::new().unwrap();
    let amf_dir = repo.path().join(".amf");
    std::fs::create_dir_all(&amf_dir).unwrap();
    std::fs::write(
        amf_dir.join("config.json"),
        serde_json::to_string(&ExtensionConfig {
            allowed_agents: Some(vec![AgentKind::Claude]),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();

    let store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "other-feat", false);
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.agent = AgentKind::Opencode;
        state.agent_index = 1;
    }

    app.create_feature().unwrap();

    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("not allowed"), "got: {msg}");
}

#[test]
fn create_feature_codex_vibeless_sets_error() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "other-feat", false);
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.agent = AgentKind::Codex;
        state.mode = VibeMode::Vibeless;
    }

    app.create_feature().unwrap();

    let msg = app.message.as_deref().unwrap_or("");
    assert!(
        msg.contains("Codex does not support Vibeless diff review"),
        "got: {msg}"
    );
}

#[test]
fn start_create_feature_defaults_to_first_allowed_agent() {
    let repo = TempDir::new().unwrap();
    let amf_dir = repo.path().join(".amf");
    std::fs::create_dir_all(&amf_dir).unwrap();
    std::fs::write(
        amf_dir.join("config.json"),
        serde_json::to_string(&ExtensionConfig {
            allowed_agents: Some(vec![AgentKind::Codex]),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();

    let now = Utc::now();
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: repo.path().to_path_buf(),
        collapsed: false,
        features: vec![],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: false,
    };
    let store = ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Project(0);

    app.start_create_feature();

    match &app.mode {
        AppMode::CreatingFeature(state) => {
            assert_eq!(state.agent, AgentKind::Codex);
            assert_eq!(state.agent_index, 0);
            assert!(!state.create_terminal);
            assert!(!state.steering_enabled);
        }
        _ => panic!("expected CreatingFeature mode"),
    }
}

fn project_only_store(repo: &std::path::Path) -> ProjectStore {
    let now = Utc::now();
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: repo.to_path_buf(),
        collapsed: false,
        features: vec![],
        created_at: now,
        preferred_agent: AgentKind::Claude,
        is_git: false,
    };
    ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    }
}

#[test]
fn create_feature_default_remote_control_matches_availability() {
    let repo = TempDir::new().unwrap();
    let store = project_only_store(repo.path());
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Project(0);
    app.config.remote_control_default = true;

    app.start_create_feature();

    match &app.mode {
        AppMode::CreatingFeature(state) => {
            // With the default on, the toggle follows availability exactly —
            // it is never forced on when Remote Control can't be used.
            assert_eq!(state.remote_control, state.remote_control_available);
        }
        _ => panic!("expected CreatingFeature mode"),
    }
}

#[test]
fn create_feature_default_remote_control_blocked_by_zai() {
    let repo = TempDir::new().unwrap();
    let store = project_only_store(repo.path());
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Project(0);
    app.config.remote_control_default = true;
    app.config.zai = Some(ZaiPlanConfig {
        plan: "coding".to_string(),
        ..Default::default()
    });

    app.start_create_feature();

    match &app.mode {
        AppMode::CreatingFeature(state) => {
            assert!(!state.remote_control_available);
            assert!(
                !state.remote_control,
                "default must not override z.ai guard"
            );
            assert_eq!(
                state.remote_control_block_reason.as_deref(),
                Some("Unavailable with z.ai provider")
            );
        }
        _ => panic!("expected CreatingFeature mode"),
    }
}

fn startup_prompt_overlay_test(agent: AgentKind, expected_window: &'static str) {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().join(".worktrees").join("coached");
    std::fs::create_dir_all(&workdir).unwrap();
    let expected_session = "amf-my-project-coached";
    let mode = if agent == AgentKind::Codex {
        VibeMode::Vibe
    } else {
        VibeMode::Vibeless
    };

    let now = Utc::now();
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: repo.path().to_path_buf(),
        collapsed: false,
        features: vec![],
        created_at: now,
        preferred_agent: agent.clone(),
        is_git: true,
    };
    let store = ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: std::collections::HashMap::new(),
    };

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists()
        .withf(move |session| session == expected_session)
        .times(2)
        .returning({
            let mut calls = 0;
            move |_| {
                calls += 1;
                calls > 1
            }
        });
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_create_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    match &agent {
        AgentKind::Claude => {
            tmux.expect_launch_claude()
                .times(1)
                .returning(|_, _, _, _, _| Ok(()));
        }
        AgentKind::Opencode => {
            tmux.expect_launch_opencode()
                .times(1)
                .returning(|_, _, _| Ok(()));
        }
        AgentKind::Codex => {
            tmux.expect_launch_codex()
                .times(1)
                .withf(|session, window, feature_session_id, resume, extra_args| {
                    session == "amf-my-project-coached"
                        && window == "codex"
                        && !feature_session_id.is_empty()
                        && resume.is_none()
                        && extra_args.iter().any(|arg| arg == "--add-dir")
                })
                .returning(|_, _, _, _, _| Ok(()));
        }
        AgentKind::Pi => {
            tmux.expect_launch_pi().times(1).returning(|_, _, _| Ok(()));
        }
    }
    tmux.expect_select_window()
        .times(1)
        .returning(|_, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::CreatingFeature(CreateFeatureState {
        project_name: "my-project".to_string(),
        project_repo: repo.path().to_path_buf(),
        todo_origin: None,
        branch: "coached".to_string(),
        branch_error: None,
        allowed_agents: AgentKind::ALL.to_vec(),
        feature_presets: Vec::new(),
        step: CreateFeatureStep::Mode,
        agent: agent.clone(),
        agent_index: 0,
        mode: mode.clone(),
        mode_index: 0,
        mode_focus: 0,
        review: false,
        plan_mode: false,
        quick_plan: false,
        create_terminal: false,
        session_name: "Claude 1".to_string(),
        source_index: 0,
        worktrees: vec![],
        worktree_index: 0,
        worktree_search_active: false,
        worktree_query: String::new(),
        use_worktree: true,
        enable_chrome: false,
        remote_control: false,
        remote_control_available: true,
        remote_control_block_reason: None,
        steering_enabled: true,
        preset_index: 0,
        task_prompt: String::new(),
        prompt_analysis: analyze_prompt(""),
        prepared_launch: None,
    });

    app.finish_feature_launch(PreparedFeatureLaunch {
        project_name: "my-project".to_string(),
        branch: "coached".to_string(),
        workdir: workdir.clone(),
        is_worktree: true,
        mode,
        review: false,
        plan_mode: false,
        quick_plan: false,
        agent: agent.clone(),
        create_terminal: true,
        session_name: "Claude 1".to_string(),
        enable_chrome: false,
        remote_control: false,
        steering_enabled: true,
        hook_succeeded: None,
        startup_prompt: None,
        todo_origin: None,
    })
    .unwrap();

    match &app.mode {
        AppMode::SteeringPrompt(state) => {
            assert_eq!(state.view.window, expected_window);
            assert_eq!(state.workdir, workdir);
        }
        _ => panic!("expected SteeringPrompt mode"),
    }
}

#[test]
fn finish_feature_launch_opens_startup_prompt_for_claude() {
    startup_prompt_overlay_test(AgentKind::Claude, "claude");
}

#[test]
fn finish_feature_launch_opens_startup_prompt_for_opencode() {
    startup_prompt_overlay_test(AgentKind::Opencode, "opencode");
}

#[test]
fn finish_feature_launch_opens_startup_prompt_for_codex() {
    startup_prompt_overlay_test(AgentKind::Codex, "codex");
}

#[test]
fn restore_claude_session_resizes_window_before_launch_when_viewport_known() {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().join(".worktrees").join("restore-me");
    std::fs::create_dir_all(&workdir).unwrap();

    let now = Utc::now();
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "restore-me".to_string(),
        branch: "restore-me".to_string(),
        workdir: workdir.clone(),
        is_worktree: true,
        tmux_session: "amf-restore-me".to_string(),
        sessions: vec![],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::Claude,
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Stopped,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        selected_plan_path: None,
        triage_source: None,
        review_source: None,
    };
    let store = ProjectStore {
        version: 4,
        projects: vec![Project {
            id: "proj-1".to_string(),
            name: "my-project".to_string(),
            repo: repo.path().to_path_buf(),
            collapsed: false,
            features: vec![feature],
            created_at: now,
            preferred_agent: AgentKind::Claude,
            is_git: true,
        }],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };

    let resized = Arc::new(AtomicBool::new(false));
    let resized_for_launch = resized.clone();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists()
        .withf(|session| session == "amf-restore-me")
        .times(3)
        .return_const(false);
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_create_window()
        .times(0)
        .returning(|_, _, _| Ok(()));
    tmux.expect_resize_pane()
        .times(1)
        .withf(|session, window, cols, rows| {
            session == "amf-restore-me" && window == "claude" && *cols == 120 && *rows == 40
        })
        .returning(move |_, window, _, _| {
            if window == "claude" {
                resized.store(true, Ordering::SeqCst);
            }
            Ok(())
        });
    tmux.expect_launch_claude()
        .times(1)
        .withf(
            move |session, window, feature_session_id, resume_id, extra_args| {
                resized_for_launch.load(Ordering::SeqCst)
                    && session == "amf-restore-me"
                    && window == "claude"
                    && !feature_session_id.is_empty()
                    && resume_id.as_deref() == Some("claude-session-123")
                    && extra_args.is_empty()
            },
        )
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window()
        .times(1)
        .withf(|session, window| session == "amf-restore-me" && window == "claude")
        .returning(|_, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.store_path = tmp.path().to_path_buf();
    app.selection = Selection::Feature(0, 0);
    app.viewport_cols = 120;
    app.viewport_rows = 40;
    app.viewport_total_rows = 41;
    app.mode = AppMode::ConfirmingClaudeSession {
        session_id: "claude-session-123".to_string(),
        workdir,
    };

    app.confirm_and_start_claude().unwrap();

    match &app.mode {
        AppMode::Viewing(view) => {
            assert_eq!(view.session_kind, SessionKind::Claude);
            assert!(view.startup_mask_active());
        }
        _ => panic!("expected Viewing mode"),
    }
    assert_eq!(app.message.as_deref(), Some("Restored claude session"));
    assert!(matches!(app.selection, Selection::Session(0, 0, 0)));
}

#[test]
fn enter_view_from_feature_selects_pi_harness_and_shows_startup_mask() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].agent = AgentKind::Pi;
    store.projects[0].preferred_agent = AgentKind::Pi;

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists()
        .withf(|session| session == "amf-my-feat")
        .times(1)
        .return_const(false);
    tmux.expect_create_session_with_window()
        .withf(|session, window, workdir| {
            session == "amf-my-feat"
                && window == "pi"
                && workdir == std::path::Path::new("/tmp/test-workdir")
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .withf(|session, key, value| {
            session == "amf-my-feat" && key == "AMF_SESSION" && value == "amf-my-feat"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_create_window()
        .times(0)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_pi()
        .withf(|session, window, feature_session_id| {
            session == "amf-my-feat" && window == "pi" && !feature_session_id.is_empty()
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_select_window()
        .withf(|session, window| session == "amf-my-feat" && window == "pi")
        .times(1)
        .returning(|_, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.store_path = tmp.path().to_path_buf();
    app.selection = Selection::Feature(0, 0);

    app.enter_view_without_auto_compose().unwrap();

    match &app.mode {
        AppMode::Viewing(view) => {
            assert_eq!(view.session_kind, SessionKind::Pi);
            assert_eq!(view.window, "pi");
            assert!(view.startup_mask_active());
        }
        _ => panic!("expected Viewing mode"),
    }
}

#[test]
fn finish_feature_launch_vibeless_injects_custom_diff_review_hook_on_worktree_creation() {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().join(".worktrees").join("diffy");
    std::fs::create_dir_all(&workdir).unwrap();

    let store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(1).return_const(false);
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_create_window()
        .times(0)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .times(1)
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window()
        .times(1)
        .returning(|_, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.store_path = tmp.path().to_path_buf();
    app.config.diff_review_viewer = DiffReviewViewer::Amf;

    app.finish_feature_launch(PreparedFeatureLaunch {
        project_name: "my-project".to_string(),
        branch: "diffy".to_string(),
        workdir: workdir.clone(),
        is_worktree: true,
        mode: VibeMode::Vibeless,
        review: false,
        plan_mode: false,
        quick_plan: false,
        agent: AgentKind::Claude,
        create_terminal: false,
        session_name: "Claude 1".to_string(),
        enable_chrome: false,
        remote_control: false,
        steering_enabled: false,
        hook_succeeded: None,
        startup_prompt: None,
        todo_origin: None,
    })
    .unwrap();

    let settings =
        std::fs::read_to_string(workdir.join(".claude").join("settings.local.json")).unwrap();
    assert!(
        settings.contains("custom-diff-review.sh"),
        "expected vibeless worktree creation to inject custom diff review hook, got: {settings}"
    );
    assert!(
        settings.contains("notify.sh"),
        "expected Claude notification hook to be installed, got: {settings}"
    );
    assert!(
        settings.contains("\"matcher\": \"Edit|Write\""),
        "expected vibeless PreToolUse matcher to be limited to Edit|Write, got: {settings}"
    );
}

#[test]
fn finish_feature_launch_vibeless_copies_opencode_change_tracker_plugin() {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().join(".worktrees").join("diffy-opencode");
    std::fs::create_dir_all(&workdir).unwrap();

    let store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(1).return_const(false);
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_create_window()
        .times(0)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_opencode()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_select_window()
        .times(1)
        .returning(|_, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.store_path = tmp.path().to_path_buf();

    app.finish_feature_launch(PreparedFeatureLaunch {
        project_name: "my-project".to_string(),
        branch: "diffy-opencode".to_string(),
        workdir: workdir.clone(),
        is_worktree: true,
        mode: VibeMode::Vibeless,
        review: false,
        plan_mode: false,
        quick_plan: false,
        agent: AgentKind::Opencode,
        create_terminal: false,
        session_name: "Opencode 1".to_string(),
        enable_chrome: false,
        remote_control: false,
        steering_enabled: false,
        hook_succeeded: None,
        startup_prompt: None,
        todo_origin: None,
    })
    .unwrap();

    let plugin_dir = workdir.join(".opencode").join("plugins");
    let change_tracker = plugin_dir.join("change-tracker.js");
    assert!(
        change_tracker.exists(),
        "expected vibeless Opencode launch to install change-tracker.js in {}, available files: {:?}",
        plugin_dir.display(),
        std::fs::read_dir(&plugin_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    );
    let installed = std::fs::read_to_string(change_tracker).unwrap();
    assert!(
        installed.contains("original_file")
            && installed.contains("proposed_file")
            && installed.contains("buildReviewFiles"),
        "expected installed change-tracker.js to be the structured diff-review version, got: {installed}"
    );
    assert!(
        installed.contains("amf_feature_session_id")
            && installed.contains("provider_session_id")
            && installed.contains("AMF_FEATURE_SESSION_ID")
            && installed.contains("AMF_ACTIVE"),
        "expected installed change-tracker.js to include session identity metadata, got: {installed}"
    );
}

#[test]
fn refresh_opencode_plugins_overwrites_stale_change_tracker_plugin() {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().join(".worktrees").join("diffy-opencode");
    let plugin_dir = workdir.join(".opencode").join("plugins");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("change-tracker.js"), "stale plugin").unwrap();

    // Remove the version stamp so refresh_opencode_plugins_for_store
    // doesn't short-circuit even if another test already marked hooks current.
    let stamp = crate::project::amf_config_dir().join("last_hook_refresh_version");
    let _ = std::fs::remove_file(&stamp);

    crate::app::setup::ensure_notify_scripts();

    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    let feature = &mut store.projects[0].features[0];
    feature.workdir = workdir.clone();
    feature.is_worktree = true;
    feature.agent = AgentKind::Opencode;
    feature.mode = VibeMode::Vibeless;

    let refreshed = crate::app::setup::refresh_opencode_plugins_for_store(&store);
    assert_eq!(refreshed, 1);

    let installed = std::fs::read_to_string(plugin_dir.join("change-tracker.js")).unwrap();
    assert!(
        installed.contains("original_file")
            && installed.contains("proposed_file")
            && installed.contains("buildReviewFiles"),
        "expected stale change-tracker.js to be replaced with the structured diff-review version, got: {installed}"
    );

    let sidebar_plugin = std::fs::read_to_string(plugin_dir.join("sidebar-state.js")).unwrap();
    assert!(
        sidebar_plugin.contains("SidebarStatePlugin")
            && sidebar_plugin.contains("opencode-sidebar")
            && sidebar_plugin.contains("state.liveSummary = null")
            && sidebar_plugin.contains("role !== \"assistant\"")
            && sidebar_plugin.contains("pruneSidebarFiles")
            && sidebar_plugin.contains("SIDEBAR_MAX_FILES")
            && sidebar_plugin.contains("normalizePrompt(payload?.message?.summary?.content)")
            && sidebar_plugin.contains("function eventPayload(event)")
            && sidebar_plugin.contains("amf_feature_session_id")
            && sidebar_plugin.contains("provider_session_id")
            && sidebar_plugin.contains("event: async ({ event })")
            && sidebar_plugin.contains("switch (event?.type)")
            && sidebar_plugin.contains("case \"todo.updated\"")
            && sidebar_plugin.contains("case \"message.updated\""),
        "expected sidebar-state.js to be installed, got: {sidebar_plugin}"
    );
}

#[test]
fn create_project_persists_selected_preferred_agent() {
    let repo = TempDir::new().unwrap();
    let tmp = NamedTempFile::new().unwrap();
    let store = ProjectStore {
        version: 4,
        projects: vec![],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };
    let repo_path = repo.path().to_path_buf();
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .times(2)
        .returning(move |_| Ok(repo_path.clone()));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::CreatingProject(CreateProjectState {
        step: CreateProjectStep::Agent,
        name: "my-project".to_string(),
        path: repo.path().to_string_lossy().into_owned(),
        agent: AgentKind::Codex,
        agent_index: 0,
    });

    app.create_project().unwrap();

    assert_eq!(app.store.projects.len(), 1);
    assert_eq!(app.store.projects[0].preferred_agent, AgentKind::Codex);
}

#[test]
fn start_create_feature_uses_project_preferred_agent_when_allowed() {
    let now = Utc::now();
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: PathBuf::from("/tmp/test-repo"),
        collapsed: false,
        features: vec![],
        created_at: now,
        preferred_agent: AgentKind::Codex,
        is_git: false,
    };
    let store = ProjectStore {
        version: 4,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Project(0);

    app.start_create_feature();

    match &app.mode {
        AppMode::CreatingFeature(state) => {
            assert_eq!(state.agent, AgentKind::Codex);
            assert_eq!(state.agent_index, 2);
            assert!(!state.steering_enabled);
        }
        _ => panic!("expected CreatingFeature mode"),
    }
}

#[test]
fn create_feature_mode_allows_toggling_steering_for_claude() {
    use crossterm::event::KeyCode;

    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "other-feat", true);
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.step = CreateFeatureStep::Mode;
        state.agent = AgentKind::Claude;
        // Steering coach is focus 6 for Claude (focus 5 is Remote Control).
        state.mode_focus = 6;
        state.steering_enabled = false;
    }

    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Char('j')).unwrap();

    match &app.mode {
        AppMode::CreatingFeature(state) => assert!(state.steering_enabled),
        _ => panic!("expected CreatingFeature mode"),
    }
}

#[test]
fn create_feature_mode_allows_toggling_remote_control_for_claude() {
    use crossterm::event::KeyCode;

    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "other-feat", true);
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.step = CreateFeatureStep::Mode;
        state.agent = AgentKind::Claude;
        // Remote Control is focus 5 for Claude.
        state.mode_focus = 5;
        state.remote_control = false;
        state.steering_enabled = false;
    }

    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Char('j')).unwrap();

    match &app.mode {
        AppMode::CreatingFeature(state) => {
            assert!(state.remote_control);
            // Toggling Remote Control must not affect steering.
            assert!(!state.steering_enabled);
        }
        _ => panic!("expected CreatingFeature mode"),
    }
}

#[test]
fn create_feature_mode_review_focus_describes_review_notes() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "other-feat", true);
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.step = CreateFeatureStep::Mode;
        state.mode_focus = 2;
        assert_eq!(
            state.focused_mode_description(),
            Some(
                "High token usage: writes developer notes with every code change for a detailed code review."
            )
        );
    } else {
        panic!("expected CreatingFeature mode");
    }
}

#[test]
fn create_feature_mode_harness_and_vibemode_focus_have_no_description() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "other-feat", true);
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.step = CreateFeatureStep::Mode;
        state.mode_focus = 0;
        assert_eq!(state.focused_mode_description(), None);

        state.mode_focus = 1;
        assert_eq!(state.focused_mode_description(), None);
    } else {
        panic!("expected CreatingFeature mode");
    }
}

#[test]
fn create_feature_mouse_hover_over_review_updates_mode_focus() {
    use crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};

    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "other-feat", true);
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.step = CreateFeatureStep::Mode;
        state.mode_focus = 0;
    }

    crate::handlers::handle_mouse(
        &mut app,
        MouseEvent {
            kind: MouseEventKind::Moved,
            column: 10,
            row: 25,
            modifiers: KeyModifiers::NONE,
        },
        97,
    )
    .unwrap();

    match &app.mode {
        AppMode::CreatingFeature(state) => assert_eq!(state.mode_focus, 2),
        _ => panic!("expected CreatingFeature mode"),
    }
}

#[test]
fn create_feature_mode_remote_control_toggle_inert_when_unavailable() {
    use crossterm::event::KeyCode;

    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "other-feat", true);
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.step = CreateFeatureStep::Mode;
        state.agent = AgentKind::Claude;
        state.mode_focus = 5;
        state.remote_control = false;
        // Simulate a z.ai / third-party provider session.
        state.remote_control_available = false;
    }

    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Char('j')).unwrap();

    match &app.mode {
        AppMode::CreatingFeature(state) => assert!(!state.remote_control),
        _ => panic!("expected CreatingFeature mode"),
    }
}

#[test]
fn open_session_picker_selects_project_preferred_agent_by_default() {
    let repo = TempDir::new().unwrap();
    let amf_dir = repo.path().join(".amf");
    std::fs::create_dir_all(&amf_dir).unwrap();
    std::fs::write(
        amf_dir.join("config.json"),
        serde_json::to_string(&ExtensionConfig {
            allowed_agents: Some(vec![AgentKind::Claude, AgentKind::Codex]),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();

    let now = Utc::now();
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: repo.path().to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::Claude,
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Stopped,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        selected_plan_path: None,
        triage_source: None,
        review_source: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: repo.path().to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::Codex,
        is_git: true,
    };
    let store = ProjectStore {
        version: 4,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);

    app.open_session_picker().unwrap();

    match &app.mode {
        AppMode::SessionPicker(state) => {
            assert_eq!(state.selected, 1);
            assert_eq!(state.builtin_sessions[0].kind, SessionKind::Claude);
            assert_eq!(state.builtin_sessions[1].kind, SessionKind::Codex);
        }
        _ => panic!("expected SessionPicker mode"),
    }
}

#[test]
fn create_feature_final_enter_opens_session_name_step() {
    use crossterm::event::KeyCode;

    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "other-feat", true);
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.step = CreateFeatureStep::Mode;
        state.agent = AgentKind::Codex;
        state.mode_focus = 4;
    }

    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Enter).unwrap();

    match &app.mode {
        AppMode::CreatingFeature(state) => {
            assert_eq!(state.step, CreateFeatureStep::SessionName);
            assert_eq!(state.session_name, "Codex 1");
        }
        _ => panic!("expected CreatingFeature mode"),
    }
}

#[test]
fn create_feature_session_name_enter_creates_and_starts_feature() {
    use crossterm::event::KeyCode;

    let repo = TempDir::new().unwrap();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists()
        .withf(|session| session == "amf-automation-project-feature-1")
        .times(1)
        .return_const(false);
    let expected_repo = repo.path().to_path_buf();
    tmux.expect_create_session_with_window()
        .withf(move |session, window, workdir| {
            session == "amf-automation-project-feature-1"
                && window == "claude"
                && workdir == expected_repo.as_path()
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_create_window()
        .times(0)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .withf(
            |session, window, feature_session_id, resume_id, extra_args| {
                session == "amf-automation-project-feature-1"
                    && window == "claude"
                    && !feature_session_id.is_empty()
                    && resume_id.is_none()
                    && extra_args.is_empty()
            },
        )
        .times(1)
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window()
        .withf(|session, window| {
            session == "amf-automation-project-feature-1" && window == "claude"
        })
        .times(1)
        .returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        store_with_empty_project(repo.path().to_path_buf(), true),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.selection = Selection::Project(0);
    app.mode = AppMode::CreatingFeature(CreateFeatureState {
        project_name: "automation-project".to_string(),
        project_repo: repo.path().to_path_buf(),
        todo_origin: None,
        branch: "feature-1".to_string(),
        branch_error: None,
        allowed_agents: AgentKind::ALL.to_vec(),
        feature_presets: Vec::new(),
        step: CreateFeatureStep::SessionName,
        agent: AgentKind::Claude,
        agent_index: 0,
        mode: VibeMode::Vibeless,
        mode_index: 0,
        mode_focus: 5,
        review: false,
        plan_mode: false,
        quick_plan: false,
        create_terminal: false,
        session_name: "Pairing Claude".to_string(),
        source_index: 0,
        worktrees: vec![],
        worktree_index: 0,
        worktree_search_active: false,
        worktree_query: String::new(),
        use_worktree: false,
        enable_chrome: false,
        remote_control: false,
        remote_control_available: true,
        remote_control_block_reason: None,
        steering_enabled: false,
        preset_index: 0,
        task_prompt: String::new(),
        prompt_analysis: analyze_prompt(""),
        prepared_launch: None,
    });

    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Enter).unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    let feature = &app.store.projects[0].features[0];
    assert_eq!(feature.name, "feature-1");
    assert_eq!(feature.sessions[0].label, "Pairing Claude");
    assert_eq!(feature.sessions[0].kind, SessionKind::Claude);
}

#[test]
fn create_feature_session_name_enter_surfaces_validation_error() {
    use crossterm::event::KeyCode;

    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = app_in_creating_feature_mode(store, "my-project", "my-feat", false);
    if let AppMode::CreatingFeature(state) = &mut app.mode {
        state.step = CreateFeatureStep::SessionName;
        state.session_name = "Claude 1".to_string();
    }

    crate::handlers::handle_create_feature_key(&mut app, KeyCode::Enter).unwrap();

    match &app.mode {
        AppMode::CreatingFeature(state) => {
            assert_eq!(state.step, CreateFeatureStep::Branch);
            assert!(
                state
                    .branch_error
                    .as_deref()
                    .unwrap_or_default()
                    .contains("already exists")
            );
        }
        _ => panic!("expected CreatingFeature mode"),
    }
}

#[test]
fn session_picker_enter_opens_name_step_with_default_label() {
    let repo = TempDir::new().unwrap();
    let amf_dir = repo.path().join(".amf");
    std::fs::create_dir_all(&amf_dir).unwrap();
    std::fs::write(
        amf_dir.join("config.json"),
        serde_json::to_string(&ExtensionConfig {
            allowed_agents: Some(vec![AgentKind::Claude, AgentKind::Codex]),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();

    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active);
    store.projects[0].preferred_agent = AgentKind::Codex;
    store.projects[0].is_git = true;
    store.projects[0].features[0].sessions.push(FeatureSession {
        id: "session-1".to_string(),
        kind: SessionKind::Codex,
        label: "Codex 1".to_string(),
        tmux_window: "codex".to_string(),
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
    });

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.open_session_picker().unwrap();

    crate::handlers::handle_session_picker_key(&mut app, KeyCode::Enter).unwrap();

    match &app.mode {
        AppMode::NamingNewSession(state) => {
            assert_eq!(state.input, "Codex 2");
            assert_eq!(state.project_idx, 0);
            assert_eq!(state.feature_idx, 0);
        }
        _ => panic!("expected NamingNewSession mode"),
    }
}

#[test]
fn session_picker_offers_terminal() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);

    app.open_session_picker().unwrap();

    match &app.mode {
        AppMode::SessionPicker(state) => {
            assert!(
                state
                    .builtin_sessions
                    .iter()
                    .any(|session| session.kind == SessionKind::Terminal)
            );
        }
        _ => panic!("expected SessionPicker mode"),
    }
}

#[test]
fn new_session_name_escape_returns_to_picker() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.open_session_picker().unwrap();

    crate::handlers::handle_session_picker_key(&mut app, KeyCode::Enter).unwrap();
    crate::handlers::handle_new_session_name_key(&mut app, KeyCode::Esc).unwrap();

    match &app.mode {
        AppMode::SessionPicker(state) => assert_eq!(state.selected, 0),
        _ => panic!("expected SessionPicker mode"),
    }
}

#[test]
fn new_session_name_rejects_empty_input() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.open_session_picker().unwrap();

    crate::handlers::handle_session_picker_key(&mut app, KeyCode::Enter).unwrap();
    if let AppMode::NamingNewSession(state) = &mut app.mode {
        state.input.clear();
    }
    crate::handlers::handle_new_session_name_key(&mut app, KeyCode::Enter).unwrap();

    assert!(matches!(app.mode, AppMode::NamingNewSession(_)));
    assert_eq!(app.message.as_deref(), Some("Name cannot be empty"));
    assert!(app.store.projects[0].features[0].sessions.is_empty());
}

#[test]
fn adding_session_starts_stopped_feature() {
    let mut tmux = MockTmuxOps::new();
    let mut sequence = mockall::Sequence::new();
    // The gate asks first whether this add will start the feature.
    tmux.expect_session_exists()
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(false);
    tmux.expect_session_exists()
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(false);
    tmux.expect_session_exists()
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(false);
    tmux.expect_session_exists()
        .times(1)
        .in_sequence(&mut sequence)
        .return_const(true);
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .times(1)
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window()
        .times(1)
        .returning(|_, _| Ok(()));
    tmux.expect_create_window()
        .withf(|session, window, _| session == "amf-my-feat" && window == "terminal")
        .times(1)
        .returning(|_, _, _| Ok(()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Stopped),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );

    app.add_builtin_session_with_label(0, 0, SessionKind::Terminal, "Shell".into())
        .unwrap();

    let feature = &app.store.projects[0].features[0];
    assert_eq!(feature.status, ProjectStatus::Idle);
    assert_eq!(feature.sessions.len(), 2);
    assert_eq!(feature.sessions[1].label, "Shell");
}

#[test]
fn starting_stopped_feature_limits_saved_agent_autostart() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    {
        let feature = &mut store.projects[0].features[0];
        feature.add_session_named(SessionKind::Claude, "Primary Claude".into());
        feature.add_session_named(SessionKind::Claude, "Extra Claude".into());
        feature.add_session_named(SessionKind::Codex, "Extra Codex".into());
    }

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(1).return_const(false);
    tmux.expect_create_session_with_window()
        .withf(|session, window, _| session == "amf-my-feat" && window == "claude")
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_create_window()
        .withf(|session, window, _| {
            session == "amf-my-feat" && matches!(window, "claude-2" | "codex")
        })
        .times(2)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .withf(|session, window, _, _, _| session == "amf-my-feat" && window == "claude")
        .times(1)
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_launch_codex().times(0);
    tmux.expect_select_window()
        .withf(|session, window| session == "amf-my-feat" && window == "claude")
        .times(1)
        .returning(|_, _| Ok(()));

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));

    app.do_start_feature(0, 0).unwrap();

    let feature = &app.store.projects[0].features[0];
    assert_eq!(feature.status, ProjectStatus::Idle);
    assert_eq!(feature.sessions.len(), 3);
}

#[test]
fn adding_session_start_failure_shows_error_toast() {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(3).return_const(false);
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| anyhow::bail!("tmux failed"));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Stopped),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.open_session_picker().unwrap();
    if let AppMode::SessionPicker(state) = &mut app.mode {
        state.selected = state
            .builtin_sessions
            .iter()
            .position(|session| session.kind == SessionKind::Terminal)
            .unwrap();
    }

    crate::handlers::handle_session_picker_key(&mut app, KeyCode::Enter).unwrap();
    crate::handlers::handle_new_session_name_key(&mut app, KeyCode::Enter).unwrap();

    assert_eq!(
        app.toasts.last().map(|toast| toast.message.as_str()),
        Some("Error: tmux failed")
    );
    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
}

#[test]
fn feature_add_session_named_uses_custom_label_and_default_window() {
    let mut feature = Feature::new(
        "my-feat".to_string(),
        "my-feat".to_string(),
        PathBuf::from("/tmp/test-workdir"),
        false,
        VibeMode::default(),
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );

    let session = feature.add_session_named(SessionKind::Claude, "Review Claude".to_string());

    assert_eq!(session.label, "Review Claude");
    assert_eq!(session.tmux_window, "claude");
}

#[test]
fn reload_extension_config_uses_project_repo_for_worktree_feature() {
    let repo = TempDir::new().unwrap();
    let amf_dir = repo.path().join(".amf");
    std::fs::create_dir_all(&amf_dir).unwrap();
    std::fs::write(
        amf_dir.join("config.json"),
        serde_json::to_string(&ExtensionConfig {
            allowed_agents: Some(vec![AgentKind::Claude]),
            ..Default::default()
        })
        .unwrap(),
    )
    .unwrap();

    let workdir = repo.path().join(".worktrees").join("feature-a");
    std::fs::create_dir_all(&workdir).unwrap();

    let now = Utc::now();
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir,
        is_worktree: true,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::default(),
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Stopped,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        selected_plan_path: None,
        triage_source: None,
        review_source: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: repo.path().to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: true,
    };
    let store = ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);

    app.reload_extension_config();

    assert_eq!(
        app.active_extension.allowed_agents(),
        vec![AgentKind::Claude]
    );
}

// ── stop_feature ──────────────────────────────────────────────

#[test]
fn stop_feature_transitions_idle_to_stopped() {
    let tmp = NamedTempFile::new().unwrap();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_kill_session()
        .withf(|s| s == "amf-my-feat")
        .times(1)
        .returning(|_| Ok(()));

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.store_path = tmp.path().to_path_buf();
    app.selection = Selection::Feature(0, 0);
    app.pending_sidebar_loads.insert("amf-my-feat".to_string());
    app.latest_prompt_cache
        .insert("amf-my-feat".to_string(), prompt_entry("cached prompt"));
    app.opencode_sidebar_cache.insert(
        "amf-my-feat".to_string(),
        crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "ses-1".to_string(),
            title: Some("cached".to_string()),
            latest_prompt: None,
            status: None,
            last_tool: None,
            todo_count: None,
            todo_preview: Vec::new(),
            pending_permission: None,
            last_error: None,
            lsp_summary: None,
            live_summary: None,
            model: None,
            provider: None,
            reasoning_tokens: None,
            additions: None,
            deletions: None,
            files: None,
        },
    );

    app.stop_feature().unwrap();

    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Stopped"),
        "got: {:?}",
        app.message
    );
    assert!(app.latest_prompt_for_session("amf-my-feat").is_none());
    assert!(!app.opencode_sidebar_cache.contains_key("amf-my-feat"));
    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
}

#[test]
fn complete_deleting_feature_clears_sidebar_caches() {
    let repo = TempDir::new().unwrap();
    let tmp = NamedTempFile::new().unwrap();

    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.latest_prompt_cache
        .insert("amf-my-feat".to_string(), prompt_entry("cached prompt"));
    app.opencode_sidebar_cache.insert(
        "amf-my-feat".to_string(),
        crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "ses-1".to_string(),
            title: Some("cached".to_string()),
            latest_prompt: None,
            status: None,
            last_tool: None,
            todo_count: None,
            todo_preview: Vec::new(),
            pending_permission: None,
            last_error: None,
            lsp_summary: None,
            live_summary: None,
            model: None,
            provider: None,
            reasoning_tokens: None,
            additions: None,
            deletions: None,
            files: None,
        },
    );
    app.pending_sidebar_loads.insert("amf-my-feat".to_string());
    app.mode = AppMode::DeletingFeatureInProgress(DeletingFeatureState {
        project_name: "my-project".to_string(),
        feature_name: "my-feat".to_string(),
        tmux_session: "amf-my-feat".to_string(),
        is_worktree: false,
        repo: repo.path().to_path_buf(),
        workdir: repo.path().to_path_buf(),
        stage: DeleteStage::Completed,
        child: None,
        output: String::new(),
        output_rx: None,
        error: None,
    });

    app.complete_deleting_feature().unwrap();

    assert!(app.latest_prompt_for_session("amf-my-feat").is_none());
    assert!(!app.opencode_sidebar_cache.contains_key("amf-my-feat"));
    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
}

#[test]
fn complete_deleting_feature_clears_terminal_pr_association() {
    let repo = TempDir::new().unwrap();
    let store_file = NamedTempFile::new().unwrap();
    let db_file = NamedTempFile::new().unwrap();
    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    store.projects[0].features[0].is_worktree = true;
    let feature = &store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    let terminal_pr = crate::github::TerminalPr {
        number: 550,
        state: crate::github::TerminalPrState::Merged,
        at: "2026-08-21T13:32:59Z".to_string(),
    };

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = store_file.path().to_path_buf();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    app.db
        .as_ref()
        .unwrap()
        .save_pr_terminal_state(&repo.path().to_string_lossy(), &branch, &terminal_pr)
        .unwrap();
    app.terminal_prs
        .insert(feature_id.clone(), terminal_pr.clone());
    app.confirmed_no_terminal_pr.insert(feature_id.clone());
    app.active_prs.insert(
        feature_id.clone(),
        ActivePrStatus {
            branch: branch.clone(),
            head_sha: "new-head".to_string(),
            number: 561,
            unresolved_threads: Some(0),
        },
    );
    app.mode = AppMode::DeletingFeatureInProgress(DeletingFeatureState {
        project_name: "my-project".to_string(),
        feature_name: "my-feat".to_string(),
        tmux_session: "amf-my-feat".to_string(),
        is_worktree: true,
        repo: repo.path().to_path_buf(),
        workdir: repo.path().join(".worktrees/my-feat"),
        stage: DeleteStage::Completed,
        child: None,
        output: String::new(),
        output_rx: None,
        error: None,
    });

    app.complete_deleting_feature().unwrap();

    assert!(!app.active_prs.contains_key(&feature_id));
    assert!(!app.terminal_prs.contains_key(&feature_id));
    assert!(!app.confirmed_no_terminal_pr.contains(&feature_id));
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_all_pr_terminal_state()
        .unwrap();
    assert!(!cached.contains_key(&(repo.path().to_string_lossy().to_string(), branch)));
}

#[test]
fn completed_background_deletion_clears_terminal_pr_association() {
    let repo = TempDir::new().unwrap();
    let store_file = NamedTempFile::new().unwrap();
    let db_file = NamedTempFile::new().unwrap();
    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    store.projects[0].features[0].is_worktree = true;
    let feature = &store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    let terminal_pr = crate::github::TerminalPr {
        number: 550,
        state: crate::github::TerminalPrState::Merged,
        at: "2026-08-21T13:32:59Z".to_string(),
    };

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = store_file.path().to_path_buf();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    app.db
        .as_ref()
        .unwrap()
        .save_pr_terminal_state(&repo.path().to_string_lossy(), &branch, &terminal_pr)
        .unwrap();
    app.terminal_prs.insert(feature_id.clone(), terminal_pr);
    app.background_deletions.insert(
        "my-project/my-feat".to_string(),
        BackgroundDeletion {
            project_name: "my-project".to_string(),
            feature_name: "my-feat".to_string(),
            tmux_session: "amf-my-feat".to_string(),
            is_worktree: true,
            repo: repo.path().to_path_buf(),
            workdir: repo.path().join(".worktrees/my-feat"),
            stage: DeleteStage::Completed,
            child: None,
            output: String::new(),
            output_rx: None,
            error: None,
        },
    );

    app.poll_background_deletions().unwrap();

    assert!(!app.terminal_prs.contains_key(&feature_id));
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_all_pr_terminal_state()
        .unwrap();
    assert!(!cached.contains_key(&(repo.path().to_string_lossy().to_string(), branch)));
}

#[test]
fn delete_project_clears_terminal_pr_associations_for_all_features() {
    let repo = TempDir::new().unwrap();
    let store_file = NamedTempFile::new().unwrap();
    let db_file = NamedTempFile::new().unwrap();
    let store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    let feature = &store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    let terminal_pr = crate::github::TerminalPr {
        number: 550,
        state: crate::github::TerminalPrState::Merged,
        at: "2026-08-21T13:32:59Z".to_string(),
    };

    let mut tmux = MockTmuxOps::new();
    tmux.expect_kill_session().returning(|_| Ok(()));
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.store_path = store_file.path().to_path_buf();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    app.db
        .as_ref()
        .unwrap()
        .save_pr_terminal_state(&repo.path().to_string_lossy(), &branch, &terminal_pr)
        .unwrap();
    app.active_prs.insert(
        feature_id.clone(),
        ActivePrStatus {
            branch: branch.clone(),
            head_sha: "new-head".to_string(),
            number: 561,
            unresolved_threads: Some(0),
        },
    );
    app.terminal_prs.insert(feature_id.clone(), terminal_pr);
    app.confirmed_no_terminal_pr.insert(feature_id.clone());
    app.mode = AppMode::DeletingProject("my-project".to_string());

    app.delete_project().unwrap();

    assert!(!app.active_prs.contains_key(&feature_id));
    assert!(!app.terminal_prs.contains_key(&feature_id));
    assert!(!app.confirmed_no_terminal_pr.contains(&feature_id));
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_all_pr_terminal_state()
        .unwrap();
    assert!(!cached.contains_key(&(repo.path().to_string_lossy().to_string(), branch)));
}

fn store_with_single_claude_session() -> ProjectStore {
    let now = Utc::now();
    let session = FeatureSession {
        id: "sess-1".to_string(),
        kind: SessionKind::Claude,
        label: "Claude 1".to_string(),
        tmux_window: "claude".to_string(),
        claude_session_id: None,
        todo_reference: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at: now,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
        token_usage: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: PathBuf::from("/tmp/test-workdir"),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::default(),
        enable_chrome: false,
        remote_control: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        selected_plan_path: None,
        triage_source: None,
        review_source: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: PathBuf::from("/tmp/test-repo"),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: false,
    };
    ProjectStore {
        version: 4,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    }
}

#[test]
fn bookmark_add_and_remove_current_session() {
    let store = store_with_single_claude_session();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Session(0, 0, 0);
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();

    app.bookmark_current_session().unwrap();
    assert_eq!(app.store.session_bookmarks.len(), 1);
    assert_eq!(app.store.session_bookmarks[0].session_id, "sess-1");

    app.unbookmark_current_session().unwrap();
    assert!(app.store.session_bookmarks.is_empty());
}

#[test]
fn jump_to_bookmark_opens_composer_for_agent_session() {
    let store = store_with_single_claude_session();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(1).returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.bookmark_current_session().unwrap();
    app.mode = AppMode::Normal;

    app.jump_to_bookmark(1).unwrap();

    assert!(matches!(app.selection, Selection::Session(0, 0, 0)));
    match &app.mode {
        AppMode::Compose(state) => {
            assert_eq!(state.view.session, "amf-my-feat");
            assert_eq!(state.view.window, "claude");
            assert!(state.editor.text().is_empty());
        }
        _ => panic!("expected Compose mode"),
    }
}

#[test]
fn jump_to_bookmark_keeps_direct_agent_session_in_view() {
    let store = store_with_single_claude_session();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(1).returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.selection = Selection::Session(0, 0, 0);
    app.compose_direct_targets
        .insert("amf-my-feat:claude".to_string());
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.bookmark_current_session().unwrap();
    app.mode = AppMode::Normal;

    app.jump_to_bookmark(1).unwrap();

    assert!(matches!(app.selection, Selection::Session(0, 0, 0)));
    assert!(matches!(app.mode, AppMode::Viewing(_)));
}

#[test]
fn create_project_automation_dry_run_returns_plan_without_mutating_store() {
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(2)
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        ProjectStore {
            version: 4,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );

    let request = CreateProjectRequest {
        path: repo.clone(),
        project_name: "automation-project".to_string(),
        preferred_agent: None,
        dry_run: true,
    };

    let response = app.create_project_from_request(&request).unwrap();

    assert!(response.ok);
    assert!(response.dry_run);
    assert_eq!(response.project_name, "automation-project");
    assert_eq!(response.project_path, repo);
    assert!(response.is_git);
    assert!(app.store.projects.is_empty());
}

#[test]
fn usability_project_form_can_move_back_without_losing_input() {
    let mut app = App::new_for_test(
        ProjectStore::empty(),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::CreatingProject(CreateProjectState {
        step: CreateProjectStep::Agent,
        name: "my-project".into(),
        path: "/tmp/my-project".into(),
        agent: AgentKind::Claude,
        agent_index: 0,
    });
    crate::handlers::handle_create_project_key(
        &mut app,
        KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT),
    )
    .unwrap();
    assert!(
        matches!(&app.mode, AppMode::CreatingProject(s) if matches!(s.step, CreateProjectStep::Path) && s.name == "my-project" && s.path == "/tmp/my-project")
    );
}

#[test]
fn usability_project_rejects_file_path_and_focuses_preserved_input() {
    let file = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore::empty(),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::CreatingProject(CreateProjectState {
        step: CreateProjectStep::Agent,
        name: "example".into(),
        path: file.path().to_string_lossy().into_owned(),
        agent: AgentKind::Claude,
        agent_index: 0,
    });
    app.create_project().unwrap();
    assert!(app.store.projects.is_empty());
    assert!(app.message.as_deref().unwrap().contains("not a directory"));
    assert!(
        matches!(&app.mode, AppMode::CreatingProject(s) if matches!(s.step, CreateProjectStep::Path) && s.name == "example")
    );
}

#[test]
fn usability_failed_session_removal_preserves_records() {
    for session_count in [1, 2] {
        let mut store = store_with_feature(ProjectStatus::Active);
        store.projects[0].features[0].sessions = (0..session_count)
            .map(|i| make_session(&format!("agent-{i}"), None))
            .collect();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().returning(|_| true);
        if session_count == 1 {
            tmux.expect_kill_session()
                .times(1)
                .returning(|_| Err(anyhow::anyhow!("tmux unavailable")));
        } else {
            tmux.expect_window_exists().returning(|_, _| true);
            tmux.expect_kill_window()
                .times(1)
                .returning(|_, _| Err(anyhow::anyhow!("tmux unavailable")));
        }
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        app.selection = Selection::Session(0, 0, 0);
        let error = app.remove_session().unwrap_err();
        assert!(error.to_string().contains("Session retained"));
        assert_eq!(
            app.store.projects[0].features[0].sessions.len(),
            session_count
        );
        assert_eq!(
            app.store.projects[0].features[0].status,
            ProjectStatus::Active
        );
    }
}

#[test]
fn usability_failed_project_cleanup_preserves_records_for_retry() {
    for fail_tmux in [true, false] {
        let workdir = TempDir::new().unwrap();
        let mut store = store_with_feature(ProjectStatus::Active);
        store.projects[0].features[0].is_worktree = true;
        store.projects[0].features[0].workdir = workdir.path().to_path_buf();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_kill_session().times(1).returning(move |_| {
            if fail_tmux {
                Err(anyhow::anyhow!("tmux unavailable"))
            } else {
                Ok(())
            }
        });
        let mut worktree = MockWorktreeOps::new();
        if !fail_tmux {
            worktree
                .expect_remove()
                .times(1)
                .returning(|_, _| Err(anyhow::anyhow!("permission denied")));
        }
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(worktree));
        app.mode = AppMode::DeletingProject("my-project".into());
        let error = app.delete_project().unwrap_err();
        assert!(error.to_string().contains("Project retained"));
        assert_eq!(app.store.projects.len(), 1);
        assert_eq!(app.store.projects[0].features.len(), 1);
    }
}
