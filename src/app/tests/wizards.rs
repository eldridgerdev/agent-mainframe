//! The creation wizards' shared keys: New Project, New Feature, batch
//! creation and the harness manager agree on Tab, Space, Ctrl+U and Esc.

use super::support::*;
use crate::app::*;
use crate::project::{AgentKind, VibeMode};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::path::PathBuf;

fn press(app: &mut App, code: KeyCode) {
    crate::handlers::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 40).unwrap();
}

fn ctrl(app: &mut App, c: char) {
    crate::handlers::handle_key(
        app,
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL),
        40,
    )
    .unwrap();
}

fn app_with(store: ProjectStore) -> App {
    App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    )
}

/// A New Feature form on `my-project` (Claude, Vibeless), focused on Name.
fn feature_form(branch: &str) -> App {
    let mut app = app_with(store_with_feature(ProjectStatus::Stopped));
    let mut state = CreateFeatureState::new(
        "my-project".into(),
        PathBuf::from("/tmp/test-repo"),
        Vec::new(),
        false,
    );
    state.branch = branch.into();
    state.session_name = default_session_name(&state.agent);
    app.mode = AppMode::CreatingFeature(state);
    app
}

fn feature_state(app: &App) -> &CreateFeatureState {
    match &app.mode {
        AppMode::CreatingFeature(state) => state,
        _ => panic!("expected the New Feature form"),
    }
}

fn feature_state_mut(app: &mut App) -> &mut CreateFeatureState {
    match &mut app.mode {
        AppMode::CreatingFeature(state) => state,
        _ => panic!("expected the New Feature form"),
    }
}

#[test]
fn new_project_starts_blank_and_ctrl_u_clears_instead_of_typing_u() {
    // Typing a path probes it for git; none of these folders is a repo.
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|path| Err(anyhow::anyhow!("{} is not a repo", path.display())));
    let mut app = App::new_for_test(
        ProjectStore::empty(),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.start_create_project();
    match &app.mode {
        AppMode::CreatingProject(state) => {
            assert!(state.path.is_empty(), "never prefilled with AMF's cwd");
            assert_eq!(state.path_is_git, None, "no git warning before a path");
        }
        _ => panic!("expected New Project"),
    }

    for c in "demo".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    press(&mut app, KeyCode::Tab);
    for c in "/tmp/x".chars() {
        press(&mut app, KeyCode::Char(c));
    }
    ctrl(&mut app, 'u');
    ctrl(&mut app, 'x');
    match &app.mode {
        AppMode::CreatingProject(state) => {
            assert_eq!(state.step, CreateProjectStep::Path);
            assert_eq!(state.path, "", "Ctrl+U clears; Ctrl+X types nothing");
            assert_eq!(state.name, "demo", "only the focused field is cleared");
        }
        _ => panic!("expected New Project"),
    }
}

#[test]
fn feature_form_tab_visits_every_row_and_wraps_like_new_project() {
    let mut app = feature_form("login");
    let mut visited = vec![(feature_state(&app).step.clone(), 0)];
    // Name, Worktree, 7 Claude option rows, Session = 10 rows.
    for _ in 0..10 {
        press(&mut app, KeyCode::Tab);
        let state = feature_state(&app);
        visited.push((state.step.clone(), state.mode_focus));
    }
    assert_eq!(visited[1].0, CreateFeatureStep::Worktree);
    assert_eq!(visited[2], (CreateFeatureStep::Mode, 0));
    assert_eq!(visited[8], (CreateFeatureStep::Mode, 6));
    assert_eq!(visited[9].0, CreateFeatureStep::SessionName);
    assert_eq!(visited[10].0, CreateFeatureStep::Branch, "Tab wraps");

    press(&mut app, KeyCode::BackTab);
    assert_eq!(feature_state(&app).step, CreateFeatureStep::SessionName);
    press(&mut app, KeyCode::BackTab);
    assert_eq!(
        (
            feature_state(&app).step.clone(),
            feature_state(&app).mode_focus
        ),
        (CreateFeatureStep::Mode, 6)
    );
}

#[test]
fn space_toggles_feature_checkboxes_and_the_worktree_choice() {
    let mut app = feature_form("login");
    {
        let state = feature_state_mut(&mut app);
        state.step = CreateFeatureStep::Mode;
        state.mode_focus = 2;
    }
    press(&mut app, KeyCode::Char(' '));
    assert!(feature_state(&app).review, "Space ticks Review");
    press(&mut app, KeyCode::Char(' '));
    assert!(!feature_state(&app).review, "and unticks it");

    feature_state_mut(&mut app).step = CreateFeatureStep::Worktree;
    let before = feature_state(&app).use_worktree;
    press(&mut app, KeyCode::Char(' '));
    assert_ne!(feature_state(&app).use_worktree, before);
}

#[test]
fn feature_name_ctrl_u_clears_and_other_chords_type_nothing() {
    let mut app = feature_form("login");
    ctrl(&mut app, 'x');
    assert_eq!(feature_state(&app).branch, "login");
    ctrl(&mut app, 'u');
    assert_eq!(feature_state(&app).branch, "");

    feature_state_mut(&mut app).step = CreateFeatureStep::SessionName;
    ctrl(&mut app, 'u');
    assert_eq!(feature_state(&app).session_name, "");
}

#[test]
fn ctrl_s_creates_from_any_row_and_still_checks_the_name() {
    let mut app = feature_form("");
    {
        let state = feature_state_mut(&mut app);
        state.step = CreateFeatureStep::Mode;
        state.mode_focus = 3;
    }
    ctrl(&mut app, 's');
    let state = feature_state(&app);
    assert_eq!(
        state.step,
        CreateFeatureStep::Branch,
        "jumps to the bad name"
    );
    assert!(
        state
            .branch_error
            .as_deref()
            .is_some_and(|e| e.contains("cannot be empty"))
    );
}

#[test]
fn supervibe_is_confirmed_at_create_time_and_n_returns_to_the_form() {
    let mut app = feature_form("fresh-feature");
    {
        let state = feature_state_mut(&mut app);
        state.step = CreateFeatureStep::Mode;
        state.mode_focus = 1;
        state.mode_index = 2;
        state.mode = VibeMode::SuperVibe;
    }
    // Enter on an option row is "next field", never create.
    press(&mut app, KeyCode::Enter);
    assert_eq!(feature_state(&app).mode_focus, 2);

    ctrl(&mut app, 's');
    assert_eq!(
        feature_state(&app).step,
        CreateFeatureStep::ConfirmSuperVibe
    );
    press(&mut app, KeyCode::Char('n'));
    assert_eq!(feature_state(&app).step, CreateFeatureStep::Mode);
    assert!(app.store.projects[0].features.len() == 1, "nothing created");
}

#[test]
fn a_typed_session_name_survives_a_harness_change() {
    let mut app = feature_form("login");
    let codex = AgentKind::ALL
        .iter()
        .position(|agent| *agent == AgentKind::Codex)
        .unwrap();
    {
        let state = feature_state_mut(&mut app);
        state.select_agent(codex);
        assert_eq!(state.session_name, "Codex 1", "default follows harness");
        state.session_name = "Pairing".into();
        state.select_agent(0);
        assert_eq!(state.session_name, "Pairing", "typed name is kept");
    }
}

#[test]
fn batch_on_a_selected_project_skips_path_and_name_and_esc_cancels() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].is_git = true;
    let mut app = app_with(store);
    app.selection = Selection::Project(0);

    app.start_create_batch_features();
    match &app.mode {
        AppMode::CreatingBatchFeatures(state) => {
            assert!(state.existing_project);
            assert_eq!(state.step, CreateBatchFeaturesStep::FeatureCount);
            assert_eq!(state.project_name, "my-project");
            assert_eq!(state.workspace_path, "/tmp/test-repo");
        }
        _ => panic!("expected the batch dialog"),
    }
    ctrl(&mut app, 'u');
    match &app.mode {
        AppMode::CreatingBatchFeatures(state) => assert_eq!(state.feature_count, 1),
        _ => panic!("expected the batch dialog"),
    }
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn batch_on_a_non_git_project_says_why_instead_of_opening() {
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Err(anyhow::anyhow!("not a git repository")));
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Stopped),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.selection = Selection::Project(0);

    app.start_create_batch_features();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.toasts
            .iter()
            .any(|toast| toast.message.contains("not a git repository"))
    );
}

#[test]
fn harness_manager_q_is_not_a_second_esc() {
    let mut app = app_with(ProjectStore::empty());
    app.open_harness_setup(false);
    press(&mut app, KeyCode::Char('q'));
    assert!(matches!(app.mode, AppMode::HarnessSetup(_)), "q is unbound");
    assert!(!app.should_quit);
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, AppMode::Normal), "Esc cancels");
}

#[test]
fn rename_prompts_clear_with_ctrl_u_like_the_wizards() {
    let mut app = app_with(store_with_feature(ProjectStatus::Stopped));
    app.mode = AppMode::RenamingFeature(RenameFeatureState {
        project_idx: 0,
        feature_idx: 0,
        input: "old".into(),
    });
    ctrl(&mut app, 'z');
    ctrl(&mut app, 'u');
    match &app.mode {
        AppMode::RenamingFeature(state) => assert_eq!(state.input, ""),
        _ => panic!("expected the rename prompt"),
    }
}

#[test]
fn only_a_projects_first_feature_gets_the_repo_dir_defaults() {
    let first =
        CreateFeatureState::new("p".into(), PathBuf::from("/nonexistent"), Vec::new(), true);
    assert!(first.first_feature);
    assert!(!first.use_worktree);
    let later =
        CreateFeatureState::new("p".into(), PathBuf::from("/nonexistent"), Vec::new(), false);
    assert!(!later.first_feature);
    assert!(later.use_worktree);
    assert!(later.branch.is_empty());
}
