use super::support::*;
use crate::app::*;
use crate::automation::{CreateBatchFeaturesRequest, CreateFeatureRequest, CreateProjectRequest};
use crate::extension::{ExtensionConfig, HookConfig, HookPrompt, LifecycleHooks};
use crate::project::AgentKind;
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use std::collections::HashMap;
use tempfile::NamedTempFile;
use tempfile::TempDir;

#[test]
fn create_project_automation_creates_project() {
    let workspace = TempDir::new().unwrap();
    let path = workspace.path().join("repo");
    std::fs::create_dir_all(&path).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = path.clone();
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
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();

    let request = CreateProjectRequest {
        path: path.clone(),
        project_name: "automation-project".to_string(),
        preferred_agent: None,
        dry_run: false,
    };

    let response = app.create_project_from_request(&request).unwrap();

    assert!(response.ok);
    assert_eq!(app.store.projects.len(), 1);
    assert_eq!(app.store.projects[0].name, "automation-project");
    assert_eq!(app.store.projects[0].repo, path);
    assert!(app.store.projects[0].is_git);
}

#[test]
fn create_feature_automation_dry_run_returns_plan_without_mutating_store() {
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let mut app = App::new_for_test(
        store_with_empty_project(repo.clone(), true),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    let request = CreateFeatureRequest {
        project_name: "automation-project".to_string(),
        branch: "feature-1".to_string(),
        agent: AgentKind::Codex,
        mode: VibeMode::Vibe,
        review: false,
        plan_mode: false,
        create_terminal: false,
        use_worktree: Some(true),
        enable_chrome: false,
        hook_choice: None,
        dry_run: true,
    };

    let response = app.create_feature_from_request(&request).unwrap();

    assert!(response.ok);
    assert!(response.dry_run);
    assert_eq!(response.project_name, "automation-project");
    assert_eq!(response.branch, "feature-1");
    assert_eq!(
        response.workdir,
        repo.join(".worktrees").join("automation-project-feature-1")
    );
    assert_eq!(response.tmux_session, "amf-automation-project-feature-1");
    assert!(response.is_worktree);
    assert!(!response.started);
    assert!(app.store.projects[0].features.is_empty());
}

#[test]
fn create_feature_automation_dry_run_surfaces_hook_prompt_options() {
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let mut app = App::new_for_test(
        store_with_empty_project(repo.clone(), true),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.extension = ExtensionConfig {
        lifecycle_hooks: LifecycleHooks {
            on_worktree_created: Some(HookConfig::WithPrompt {
                script: "setup.sh".to_string(),
                prompt: HookPrompt {
                    title: "Choose stack".to_string(),
                    options: vec!["node".to_string(), "rust".to_string()],
                },
            }),
            ..Default::default()
        },
        ..Default::default()
    };

    let request = CreateFeatureRequest {
        project_name: "automation-project".to_string(),
        branch: "feature-1".to_string(),
        agent: AgentKind::Codex,
        mode: VibeMode::Vibe,
        review: false,
        plan_mode: false,
        create_terminal: false,
        use_worktree: Some(true),
        enable_chrome: false,
        hook_choice: None,
        dry_run: true,
    };

    let response = app.create_feature_from_request(&request).unwrap();

    let prompt = response.worktree_hook_prompt.expect("missing hook prompt");
    assert_eq!(prompt.title, "Choose stack");
    assert_eq!(prompt.options, vec!["node", "rust"]);
}

#[test]
fn create_feature_automation_rejects_codex_vibeless_mode() {
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let mut app = App::new_for_test(
        store_with_empty_project(repo, true),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    let request = CreateFeatureRequest {
        project_name: "automation-project".to_string(),
        branch: "feature-1".to_string(),
        agent: AgentKind::Codex,
        mode: VibeMode::Vibeless,
        review: true,
        plan_mode: false,
        create_terminal: false,
        use_worktree: Some(true),
        enable_chrome: false,
        hook_choice: None,
        dry_run: true,
    };

    let err = app.create_feature_from_request(&request).unwrap_err();
    assert!(
        err.to_string()
            .contains("Codex does not support Vibeless diff review"),
        "got: {err}"
    );
}

#[test]
fn create_feature_automation_creates_and_starts_feature() {
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(repo.join(".claude")).unwrap();
    let worktree_path = repo.join(".worktrees").join("automation-project-feature-1");

    let mut worktree = MockWorktreeOps::new();
    let repo_for_create = repo.clone();
    let worktree_clone = worktree_path.clone();
    worktree
        .expect_create()
        .times(1)
        .withf(move |repo_path, name, branch| {
            repo_path == repo_for_create.as_path()
                && name == "automation-project-feature-1"
                && branch == "feature-1"
        })
        .returning(move |_, _, _| Ok(worktree_clone.clone()));

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(1).returning(|_| false);
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_create_window()
        .times(0)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_codex()
        .times(1)
        .withf(|_, _, feature_session_id, _, extra_args| {
            !feature_session_id.is_empty() && extra_args.iter().any(|arg| arg == "--add-dir")
        })
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window()
        .times(1)
        .returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        store_with_empty_project(repo.clone(), true),
        Box::new(tmux),
        Box::new(worktree),
    );
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();

    let request = CreateFeatureRequest {
        project_name: "automation-project".to_string(),
        branch: "feature-1".to_string(),
        agent: AgentKind::Codex,
        mode: VibeMode::Vibe,
        review: true,
        plan_mode: false,
        create_terminal: false,
        use_worktree: Some(true),
        enable_chrome: false,
        hook_choice: None,
        dry_run: false,
    };

    let response = app.create_feature_from_request(&request).unwrap();

    assert!(response.ok);
    assert_eq!(response.workdir, worktree_path);
    assert!(response.started);
    assert_eq!(app.store.projects[0].features.len(), 1);
    assert_eq!(app.store.projects[0].features[0].branch, "feature-1");
    assert_eq!(
        app.store.projects[0].features[0].tmux_session,
        "amf-automation-project-feature-1"
    );
    assert!(app.store.projects[0].features[0].is_worktree);
    assert!(app.store.projects[0].features[0].review);
    assert_eq!(app.store.projects[0].features[0].sessions.len(), 1);
}

#[test]
fn batch_feature_automation_dry_run_returns_plan_without_mutating_store() {
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(1)
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

    let request = CreateBatchFeaturesRequest {
        workspace_path: repo.clone(),
        project_name: "plan-batch".to_string(),
        feature_count: 3,
        feature_prefix: "plan-".to_string(),
        agent: AgentKind::Codex,
        mode: VibeMode::Vibe,
        review: false,
        enable_chrome: false,
        dry_run: true,
    };

    let response = app.create_batch_features_from_request(&request).unwrap();

    assert!(response.ok);
    assert!(response.dry_run);
    assert_eq!(response.features.len(), 3);
    assert_eq!(response.features[0].branch, "plan-1");
    assert_eq!(
        response.features[0].workdir,
        repo.join(".worktrees").join("plan-batch-plan-1")
    );
    assert!(app.store.projects.is_empty());
}

#[test]
fn batch_feature_automation_rejects_review_as_a_mode() {
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(1)
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

    let request = CreateBatchFeaturesRequest {
        workspace_path: repo,
        project_name: "plan-batch".to_string(),
        feature_count: 2,
        feature_prefix: "plan-".to_string(),
        agent: AgentKind::Codex,
        mode: VibeMode::Vibeless,
        review: true,
        enable_chrome: false,
        dry_run: true,
    };

    let err = app
        .create_batch_features_from_request(&request)
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("Codex does not support Vibeless diff review"),
        "got: {err}"
    );
}

#[test]
fn batch_feature_automation_creates_project_and_starts_features() {
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let worktree_one = repo.join(".worktrees").join("plan-batch-plan-1");
    let worktree_two = repo.join(".worktrees").join("plan-batch-plan-2");
    std::fs::create_dir_all(repo.join(".claude")).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(1)
        .returning(move |_| Ok(repo_clone.clone()));
    let repo_for_first = repo.clone();
    let worktree_one_clone = worktree_one.clone();
    worktree
        .expect_create()
        .times(1)
        .withf(move |repo_path, name, branch| {
            repo_path == repo_for_first.as_path()
                && name == "plan-batch-plan-1"
                && branch == "plan-1"
        })
        .returning(move |_, _, _| Ok(worktree_one_clone.clone()));
    let repo_for_second = repo.clone();
    let worktree_two_clone = worktree_two.clone();
    worktree
        .expect_create()
        .times(1)
        .withf(move |repo_path, name, branch| {
            repo_path == repo_for_second.as_path()
                && name == "plan-batch-plan-2"
                && branch == "plan-2"
        })
        .returning(move |_, _, _| Ok(worktree_two_clone.clone()));

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(2).returning(|_| false);
    tmux.expect_create_session_with_window()
        .times(2)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(2)
        .returning(|_, _, _| Ok(()));
    tmux.expect_create_window()
        .times(0)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_codex()
        .times(2)
        .withf(|_, _, feature_session_id, _, extra_args| {
            !feature_session_id.is_empty() && extra_args.iter().any(|arg| arg == "--add-dir")
        })
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window()
        .times(2)
        .returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        ProjectStore {
            version: 4,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(tmux),
        Box::new(worktree),
    );
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();

    let request = CreateBatchFeaturesRequest {
        workspace_path: repo.clone(),
        project_name: "plan-batch".to_string(),
        feature_count: 2,
        feature_prefix: "plan-".to_string(),
        agent: AgentKind::Codex,
        mode: VibeMode::Vibe,
        review: false,
        enable_chrome: false,
        dry_run: false,
    };

    let response = app.create_batch_features_from_request(&request).unwrap();

    assert!(response.ok);
    assert_eq!(response.features.len(), 2);
    assert_eq!(app.store.projects.len(), 1);
    assert_eq!(app.store.projects[0].name, "plan-batch");
    assert_eq!(app.store.projects[0].features.len(), 2);
    assert_eq!(app.store.projects[0].features[0].branch, "plan-1");
    assert_eq!(app.store.projects[0].features[1].branch, "plan-2");
    assert!(
        app.store.projects[0]
            .features
            .iter()
            .all(|feature| feature.sessions.len() == 1)
    );
}
