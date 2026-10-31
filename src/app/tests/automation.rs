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
        no_start: false,
        initial_prompt: None,
        submit_prompt: false,
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
        existing_worktree: None,
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
        no_start: false,
        initial_prompt: None,
        submit_prompt: false,
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
        existing_worktree: None,
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
        no_start: false,
        initial_prompt: None,
        submit_prompt: false,
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
        existing_worktree: None,
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
fn create_feature_automation_validates_hook_choices_before_worktree_creation() {
    for choice in [None, Some("removed-option")] {
        let workspace = TempDir::new().unwrap();
        let repo = workspace.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        // Mock worktree creation has no expectation: either request must be
        // rejected before creating a checkout or persisting a feature.
        let mut app = App::new_for_test(
            store_with_empty_project(repo, true),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.config.extension = ExtensionConfig {
            lifecycle_hooks: LifecycleHooks {
                on_worktree_created: Some(HookConfig::WithPrompt {
                    script: "exit 0".into(),
                    prompt: HookPrompt {
                        title: "Choose stack".into(),
                        options: vec!["rust".into()],
                    },
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let request = CreateFeatureRequest {
            project_name: "automation-project".into(),
            branch: "feature-1".into(),
            use_worktree: Some(true),
            hook_choice: choice.map(str::to_string),
            ..Default::default()
        };
        assert!(app.create_feature_from_request(&request).is_err());
        assert!(app.store.projects[0].features.is_empty());
    }
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
        .returning(move |_, _, _| {
            std::fs::create_dir_all(&worktree_clone).unwrap();
            Ok(worktree_clone.clone())
        });

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
        no_start: false,
        initial_prompt: Some("Review this feature".into()),
        submit_prompt: false,
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
        existing_worktree: None,
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
    assert_eq!(app.pending_automation_prompts.len(), 1);
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
        initial_prompt: None,
        submit_prompt: false,
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
        initial_prompt: None,
        submit_prompt: false,
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
        .returning(move |_, _, _| {
            std::fs::create_dir_all(&worktree_one_clone).unwrap();
            Ok(worktree_one_clone.clone())
        });
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
        .returning(move |_, _, _| {
            std::fs::create_dir_all(&worktree_two_clone).unwrap();
            Ok(worktree_two_clone.clone())
        });

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
        initial_prompt: None,
        submit_prompt: false,
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

fn session_request() -> crate::automation::CreateSessionRequest {
    crate::automation::CreateSessionRequest {
        project_name: "my-project".into(),
        feature: "my-feat".into(),
        kind: "terminal".into(),
        label: Some("Docs from note".into()),
        ..Default::default()
    }
}

#[test]
fn create_session_dry_run_validates_non_git_feature_without_side_effects() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].is_git = false;
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let mut request = session_request();
    request.dry_run = true;
    let response = app.create_session_from_request(&request).unwrap();
    assert!(response.dry_run);
    assert!(response.session_id.is_none());
    assert!(app.store.projects[0].features[0].sessions.is_empty());
    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
    request.kind = "missing".into();
    assert!(
        app.create_session_from_request(&request)
            .unwrap_err()
            .to_string()
            .contains("Unknown session kind")
    );
    request.kind = "terminal".into();
    request.initial_prompt = Some("A task".into());
    assert!(app.create_session_from_request(&request).is_err());
    request.kind = "codex".into();
    // Codex can be added beside a Vibeless primary harness, like the picker.
    assert!(app.create_session_from_request(&request).is_ok());
    request.feature = "missing".into();
    assert!(app.create_session_from_request(&request).is_err());
}

#[test]
fn create_session_adds_tracked_terminal_to_non_git_feature() {
    let dir = TempDir::new().unwrap();
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].repo = dir.path().into();
    store.projects[0].is_git = false;
    store.projects[0].features[0].workdir = dir.path().into();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    tmux.expect_create_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    let db = crate::db::AmfDb::open(&dir.path().join("test.db")).unwrap();
    app.store_version = Some(db.current_store_version().unwrap());
    app.db = Some(db);
    let response = app.create_session_from_request(&session_request()).unwrap();
    let feature = &app.store.projects[0].features[0];
    assert_eq!(feature.sessions.len(), 1);
    let session = &feature.sessions[0];
    assert_eq!(response.session_id.as_deref(), Some(session.id.as_str()));
    assert_eq!(
        response.tmux_window.as_deref(),
        Some(session.tmux_window.as_str())
    );
    assert_eq!(session.label, "Docs from note");
    let saved = app.db.as_ref().unwrap().load_store().unwrap();
    assert_eq!(saved.projects[0].features[0].sessions[0].id, session.id);
}

#[test]
fn automation_prompt_waits_for_input_and_only_submits_when_requested() {
    for submit in [false, true] {
        let mut store = store_with_feature(ProjectStatus::Active);
        let id = store.projects[0].features[0]
            .add_session_named(SessionKind::Codex, "Task".into())
            .id
            .clone();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_window_exists().return_const(true);
        let mut sequence = mockall::Sequence::new();
        tmux.expect_capture_pane()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok("Starting Codex…".into()));
        tmux.expect_capture_pane()
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _| Ok("› Describe your task\n? for shortcuts".into()));
        tmux.expect_paste_text()
            .withf(|_, _, text| text == "First line\nSecond line")
            .times(1)
            .in_sequence(&mut sequence)
            .returning(|_, _, _| Ok(()));
        tmux.expect_send_key_name()
            .withf(|_, _, key| key == "Enter")
            .times(usize::from(submit))
            .returning(|_, _, _| Ok(()));
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        app.queue_automation_prompt(&id, Some("First line\nSecond line"), submit);
        app.poll_automation_prompts();
        assert_eq!(app.pending_automation_prompts.len(), 1);
        app.poll_automation_prompts();
        assert!(app.pending_automation_prompts.is_empty());
        app.poll_automation_prompts();
    }
}

#[test]
fn automation_prompt_is_cancelled_when_session_is_stopped_or_replaced() {
    for stopped in [false, true] {
        let mut store = store_with_feature(ProjectStatus::Active);
        let id = store.projects[0].features[0]
            .add_session_named(SessionKind::Claude, "Task".into())
            .id
            .clone();
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.queue_automation_prompt(&id, Some("task"), true);
        let session = &mut app.store.projects[0].features[0].sessions[0];
        if stopped {
            session.stopped = true;
        } else {
            session.id = "replacement".into();
        }
        app.poll_automation_prompts();
        assert!(app.pending_automation_prompts.is_empty());
    }
}

#[test]
fn create_session_launches_agent_with_feature_permissions_and_queues_prompt() {
    let dir = TempDir::new().unwrap();
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].repo = dir.path().into();
    store.projects[0].is_git = false;
    store.projects[0].features[0].workdir = dir.path().into();
    store.projects[0].features[0].mode = VibeMode::SuperVibe;
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    tmux.expect_create_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .times(1)
        .withf(|_, _, id, resume, args| {
            !id.is_empty()
                && resume.is_none()
                && args
                    .iter()
                    .any(|arg| arg == "--dangerously-skip-permissions")
        })
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_check_harness_available()
        .times(1)
        .returning(|_| Ok(()));
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    let file = NamedTempFile::new().unwrap();
    app.store_path = file.path().into();
    let request = crate::automation::CreateSessionRequest {
        kind: "claude".into(),
        initial_prompt: Some("Write the docs".into()),
        ..session_request()
    };
    let response = app.create_session_from_request(&request).unwrap();
    assert!(response.session_id.is_some());
    assert_eq!(
        app.store.projects[0].features[0].sessions[0].kind,
        SessionKind::Claude
    );
    assert_eq!(app.pending_automation_prompts.len(), 1);
}

#[test]
fn create_session_failed_launch_removes_new_session_record() {
    let dir = TempDir::new().unwrap();
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].repo = dir.path().into();
    store.projects[0].features[0].workdir = dir.path().into();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    tmux.expect_create_window()
        .times(1)
        .returning(|_, _, _| anyhow::bail!("launch failed"));
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    assert!(app.create_session_from_request(&session_request()).is_err());
    assert!(app.store.projects[0].features[0].sessions.is_empty());
}

#[test]
fn batch_creation_queues_the_task_for_every_started_primary_agent() {
    let dir = TempDir::new().unwrap();
    let repo = dir.path().to_path_buf();
    let mut worktree = MockWorktreeOps::new();
    let root = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(root.clone()));
    worktree
        .expect_create()
        .times(2)
        .returning(|repo, name, _| {
            let path = repo.join(".worktrees").join(name);
            std::fs::create_dir_all(&path)?;
            Ok(path)
        });
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(false);
    tmux.expect_create_session_with_window()
        .times(2)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(2)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_codex()
        .times(2)
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window()
        .times(2)
        .returning(|_, _| Ok(()));
    let mut store = store_with_empty_project(repo.clone(), true);
    store.projects.clear();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(worktree));
    let file = NamedTempFile::new().unwrap();
    app.store_path = file.path().into();
    let response = app
        .create_batch_features_from_request(&CreateBatchFeaturesRequest {
            workspace_path: repo,
            project_name: "batch".into(),
            feature_count: 2,
            agent: AgentKind::Codex,
            mode: VibeMode::Vibe,
            initial_prompt: Some("Review the code".into()),
            ..Default::default()
        })
        .unwrap();
    assert!(response.features.iter().all(|feature| feature.started));
    assert_eq!(app.pending_automation_prompts.len(), 2);
}

#[test]
fn custom_session_dry_run_skips_precheck_and_real_failure_creates_nothing() {
    let dir = TempDir::new().unwrap();
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].repo = dir.path().into();
    store.projects[0].features[0].workdir = dir.path().into();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config
        .extension
        .custom_sessions
        .push(crate::extension::CustomSessionConfig {
            name: "docs".into(),
            pre_check: Some("exit 23".into()),
            ..Default::default()
        });
    let mut request = crate::automation::CreateSessionRequest {
        kind: "docs".into(),
        dry_run: true,
        ..session_request()
    };
    assert!(app.create_session_from_request(&request).is_ok());
    request.dry_run = false;
    // The running feature's tmux existence is checked before the pre-check.
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    app.tmux = Box::new(tmux);
    assert!(
        app.create_session_from_request(&request)
            .unwrap_err()
            .to_string()
            .contains("23")
    );
    assert!(app.store.projects[0].features[0].sessions.is_empty());
}

#[test]
fn create_feature_automation_no_start_creates_stopped_worktree_without_tmux() {
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let workdir = workspace.path().join("feature");
    let expected = workdir.clone();
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_create()
        .once()
        .returning(move |_, _, _| Ok(expected.clone()));
    let mut app = App::new_for_test(
        store_with_empty_project(repo, true),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();
    let request = CreateFeatureRequest {
        project_name: "automation-project".into(),
        branch: "later".into(),
        use_worktree: Some(true),
        no_start: true,
        ..Default::default()
    };
    let response = app.create_feature_from_request(&request).unwrap();
    assert!(!response.started);
    assert_eq!(response.workdir, workdir);
    assert!(response.message.contains("without starting"));
    let feature = &app.store.projects[0].features[0];
    assert_eq!(feature.status, ProjectStatus::Stopped);
    assert_eq!(feature.sessions.len(), 1);
    assert!(app.pending_automation_prompts.is_empty());
    assert!(response.feature_id.is_some());
}

#[test]
fn create_feature_automation_no_start_rejects_prompt_before_side_effects() {
    let mut app = App::new_for_test(
        store_with_empty_project(std::path::PathBuf::from("/unused"), true),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let request = CreateFeatureRequest {
        no_start: true,
        initial_prompt: Some("Do work".into()),
        ..Default::default()
    };
    assert!(
        app.create_feature_from_request(&request)
            .unwrap_err()
            .to_string()
            .contains("require starting")
    );
    assert!(app.store.projects[0].features.is_empty());
}

/// `B` on a selected project adds the batch to it. The automation API keeps
/// treating an existing project name as an error.
#[test]
fn a_batch_can_join_an_existing_project_but_not_reuse_its_feature_names() {
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));
    let mut store = store_with_empty_project(repo.clone(), true);
    let existing = crate::project::Feature::new_for_project(
        "automation-project",
        "feature-1".into(),
        "feature-1".into(),
        repo.join(".worktrees").join("elsewhere"),
        true,
        VibeMode::default(),
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );
    store.projects[0].features.push(existing);
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));

    let mut request = CreateBatchFeaturesRequest {
        workspace_path: repo.clone(),
        project_name: "automation-project".into(),
        feature_count: 2,
        feature_prefix: "feature-".into(),
        agent: AgentKind::Claude,
        dry_run: true,
        ..Default::default()
    };

    let err = app
        .create_batch_features_from_request(&request)
        .unwrap_err();
    assert!(err.to_string().contains("already exists"), "{err}");

    let err = app.add_batch_features_to_project(&request).unwrap_err();
    assert!(
        err.to_string()
            .contains("Feature 'feature-1' already exists in 'automation-project'"),
        "{err}"
    );

    request.feature_prefix = "batch-".into();
    let response = app.add_batch_features_to_project(&request).unwrap();
    assert_eq!(response.features[0].name, "batch-1");
    assert_eq!(app.store.projects.len(), 1, "no second project");

    request.project_name = "missing".into();
    let err = app.add_batch_features_to_project(&request).unwrap_err();
    assert!(err.to_string().contains("not found"), "{err}");
}

#[test]
fn create_feature_automation_hints_existing_worktree_when_path_exists() {
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    let existing = repo.join(".worktrees").join("automation-project-feature-1");
    std::fs::create_dir_all(&existing).unwrap();
    let mut app = App::new_for_test(
        store_with_empty_project(repo, true),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let request = CreateFeatureRequest {
        project_name: "automation-project".into(),
        branch: "feature-1".into(),
        use_worktree: Some(true),
        ..Default::default()
    };
    let err = app
        .create_feature_from_request(&request)
        .unwrap_err()
        .to_string();
    assert!(err.contains("Worktree path already exists"), "got: {err}");
    assert!(
        err.contains("not a registered git worktree") && !err.contains("\"existing_worktree\""),
        "got: {err}"
    );
}

#[test]
fn create_feature_automation_attaches_existing_worktree() {
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}: {out:?}");
    };
    let workspace = TempDir::new().unwrap();
    let repo = workspace.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "init",
        ],
    );
    let wt = workspace.path().join("elsewhere");
    git(
        &repo,
        &["worktree", "add", "-q", "-b", "side", &wt.to_string_lossy()],
    );

    let mut app = App::new_for_test(
        store_with_empty_project(repo.clone(), true),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();

    let mut request = CreateFeatureRequest {
        project_name: "automation-project".into(),
        branch: "feature-on-side".into(),
        no_start: true,
        existing_worktree: Some(workspace.path().join("nope")),
        ..Default::default()
    };
    let err = app.create_feature_from_request(&request).unwrap_err();
    assert!(err.to_string().contains("not an existing git worktree"));

    request.existing_worktree = Some(repo.clone());
    assert!(app.create_feature_from_request(&request).is_err());

    // A registered worktree at the planned path gets the existing_worktree hint.
    let planned = repo.join(".worktrees").join("automation-project-hinted");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "hinted",
            &planned.to_string_lossy(),
        ],
    );
    let hinted = CreateFeatureRequest {
        project_name: "automation-project".into(),
        branch: "hinted".into(),
        use_worktree: Some(true),
        ..Default::default()
    };
    let err = app
        .create_feature_from_request(&hinted)
        .unwrap_err()
        .to_string();
    assert!(err.contains("\"existing_worktree\""), "got: {err}");

    // A detached worktree is rejected rather than recording a guessed branch.
    let detached = workspace.path().join("detached");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            &detached.to_string_lossy(),
        ],
    );
    request.existing_worktree = Some(detached);
    let err = app.create_feature_from_request(&request).unwrap_err();
    assert!(err.to_string().contains("detached HEAD"), "{err}");

    request.existing_worktree = Some(wt.clone());
    let response = app.create_feature_from_request(&request).unwrap();
    assert!(response.ok);
    let feature = &app.store.projects[0].features[0];
    assert_eq!(feature.workdir, wt);
    assert!(feature.is_worktree);
    assert_eq!(feature.name, "feature-on-side");
    assert_eq!(feature.branch, "side");

    request.branch = "side-2".into();
    let err = app.create_feature_from_request(&request).unwrap_err();
    assert!(err.to_string().contains("already used by feature"), "{err}");
}
