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
        containerized: false,
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
    assert!(app.pending_automation_prompts.is_empty());
    assert_eq!(app.compose_drafts.len(), 1);
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
        let dir = TempDir::new().unwrap();
        let mut store = store_with_repo(dir.path().into(), ProjectStatus::Active);
        let session = store.projects[0].features[0]
            .add_session_named(SessionKind::Codex, "Task".into())
            .clone();
        let mut tmux = MockTmuxOps::new();
        if submit {
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
                .times(1)
                .returning(|_, _, _| Ok(()));
        }
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        app.queue_automation_prompt(&session.id, Some("First line\nSecond line"), submit);
        app.poll_automation_prompts();
        assert_eq!(app.pending_automation_prompts.len(), usize::from(submit));
        app.poll_automation_prompts();
        assert!(app.pending_automation_prompts.is_empty());
        app.poll_automation_prompts();
        if submit {
            assert!(app.compose_drafts.is_empty());
        } else {
            // Opening the target restores the text in AMF's editable composer.
            // With no tmux expectations this also proves that unsent prompts
            // need no harness readiness check, paste, or Enter.
            let project = &app.store.projects[0];
            let feature = &project.features[0];
            app.mode = AppMode::Viewing(ViewState::new(
                project.name.clone(),
                feature.name.clone(),
                feature.tmux_session.clone(),
                session.tmux_window,
                session.label,
                session.kind,
                feature.mode.clone(),
                false,
            ));
            app.open_compose_from_view(None).unwrap();
            let AppMode::Compose(state) = &mut app.mode else {
                panic!("expected composer")
            };
            assert_eq!(state.editor.text(), "First line\nSecond line");
            state.editor.insert_str(" edited");
            assert_eq!(state.editor.text(), "First line\nSecond line edited");
        }
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
fn create_session_launches_agent_with_feature_permissions_and_seeds_composer() {
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
    assert!(app.pending_automation_prompts.is_empty());
    assert_eq!(app.compose_drafts.len(), 1);
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
fn custom_agent_automation_seeds_configured_prompt_and_allows_request_overrides() {
    for (prompt, submit) in [
        (None, false),
        (Some("Override"), false),
        (Some("Override"), true),
    ] {
        let dir = TempDir::new().unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_check_harness_available()
            .times(1)
            .returning(|_| Ok(()));
        tmux.expect_create_window()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_run_shell_command()
            .times(1)
            .returning(|_, _, _| Ok(()));
        let mut app = App::new_for_test(
            store_with_repo(dir.path().into(), ProjectStatus::Active),
            Box::new(tmux),
            Box::new(MockWorktreeOps::new()),
        );
        let file = NamedTempFile::new().unwrap();
        app.store_path = file.path().into();
        app.config
            .extension
            .custom_sessions
            .push(crate::extension::CustomSessionConfig {
                name: "wrapper".into(),
                agent: Some(AgentKind::Claude),
                command: Some("claude".into()),
                initial_prompt: Some("Configured".into()),
                ..Default::default()
            });
        let mut request = crate::automation::CreateSessionRequest {
            kind: "wrapper".into(),
            initial_prompt: prompt.map(str::to_string),
            submit_prompt: submit,
            dry_run: true,
            ..session_request()
        };
        assert!(app.create_session_from_request(&request).unwrap().dry_run);
        assert!(app.compose_drafts.is_empty());
        assert!(app.pending_automation_prompts.is_empty());
        request.dry_run = false;
        let response = app.create_session_from_request(&request).unwrap();
        assert!(response.session_id.is_some());
        assert_eq!(app.pending_automation_prompts.len(), usize::from(submit));
        if submit {
            assert!(
                app.compose_drafts.is_empty(),
                "no configured draft left behind on submission"
            );
            assert!(response.message.contains("queued for submission"));
        } else {
            assert_eq!(app.compose_drafts.len(), 1);
            assert_eq!(
                app.compose_drafts.values().next().unwrap().text,
                prompt.unwrap_or("Configured")
            );
            assert!(response.message.contains("AMF composer, unsent"));
        }
    }
}

#[test]
fn generic_custom_session_automation_rejects_initial_prompt_even_in_dry_run() {
    let dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(dir.path().into(), ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config
        .extension
        .custom_sessions
        .push(crate::extension::CustomSessionConfig {
            name: "server".into(),
            command: Some("npm run dev".into()),
            ..Default::default()
        });
    let request = crate::automation::CreateSessionRequest {
        kind: "server".into(),
        initial_prompt: Some("Task".into()),
        dry_run: true,
        ..session_request()
    };
    assert!(
        app.create_session_from_request(&request)
            .unwrap_err()
            .to_string()
            .contains("initial_prompt requires an agent harness")
    );
    assert!(app.store.projects[0].features[0].sessions.is_empty());
}

#[test]
fn batch_creation_seeds_the_composer_for_every_started_primary_agent() {
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
    assert!(app.pending_automation_prompts.is_empty());
    assert_eq!(app.compose_drafts.len(), 2);
    assert!(
        app.compose_drafts
            .values()
            .all(|draft| draft.text == "Review the code")
    );
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
    assert_eq!(
        feature.workdir.canonicalize().unwrap(),
        wt.canonicalize().unwrap()
    );
    assert!(feature.is_worktree);
    assert_eq!(feature.name, "feature-on-side");
    assert_eq!(feature.branch, "side");

    request.branch = "side-2".into();
    let err = app.create_feature_from_request(&request).unwrap_err();
    assert!(err.to_string().contains("already used by feature"), "{err}");
}

fn app_with_terminal_and_agent(tmux: MockTmuxOps) -> (App, String, String) {
    let mut store = store_with_feature(ProjectStatus::Active);
    let feature = &mut store.projects[0].features[0];
    let agent_id = feature
        .add_session_named(SessionKind::Claude, "Claude".into())
        .id
        .clone();
    let terminal_id = feature
        .add_session_named(SessionKind::Terminal, "dev server".into())
        .id
        .clone();
    let app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    (app, agent_id, terminal_id)
}

#[test]
fn feature_target_fills_only_when_empty() {
    use crate::automation::FeatureTarget;
    let mut empty = FeatureTarget::default();
    empty.fill_from_env(Some("sess-1".into()));
    assert_eq!(empty.feature_session_id.as_deref(), Some("sess-1"));

    let mut named = FeatureTarget {
        project_name: "p".into(),
        feature: "f".into(),
        ..Default::default()
    };
    named.fill_from_env(Some("sess-1".into()));
    assert!(named.feature_session_id.is_none());

    let mut blank = FeatureTarget::default();
    blank.fill_from_env(Some("  ".into()));
    assert!(blank.is_empty());
}

#[test]
fn create_session_targets_callers_feature_by_session_id_and_runs_command() {
    let (mut app, agent_id, _) = app_with_terminal_and_agent({
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_create_window().returning(|_, _, _| Ok(()));
        tmux.expect_paste_text()
            .withf(|_, _, text| text == "npm run dev")
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_send_key_name()
            .withf(|_, _, key| key == "Enter")
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux
    });
    let dir = TempDir::new().unwrap();
    app.store.projects[0].features[0].workdir = dir.path().into();
    let request = crate::automation::CreateSessionRequest {
        feature_session_id: Some(agent_id),
        kind: "terminal".into(),
        label: Some("web".into()),
        command: Some("npm run dev".into()),
        run: true,
        ..Default::default()
    };
    let response = app.create_session_from_request(&request).unwrap();
    assert!(!response.reused);
    assert_eq!(response.message, "Created session and ran command");
    assert!(
        app.store.projects[0].features[0]
            .sessions
            .iter()
            .any(|s| s.label == "web" && s.kind == SessionKind::Terminal)
    );
}

#[test]
fn create_session_validates_terminal_only_fields() {
    let (mut app, agent_id, _) = app_with_terminal_and_agent(MockTmuxOps::new());
    let base = crate::automation::CreateSessionRequest {
        feature_session_id: Some(agent_id),
        kind: "terminal".into(),
        dry_run: true,
        ..Default::default()
    };
    let mut request = base.clone();
    request.run = true;
    assert!(
        app.create_session_from_request(&request)
            .unwrap_err()
            .to_string()
            .contains("run requires command")
    );
    let mut request = base.clone();
    request.kind = "claude".into();
    request.command = Some("ls".into());
    assert!(app.create_session_from_request(&request).is_err());
    let mut request = base.clone();
    request.kind = "claude".into();
    request.reuse = true;
    assert!(app.create_session_from_request(&request).is_err());
    let mut request = base;
    request.feature_session_id = Some("nope".into());
    assert!(
        app.create_session_from_request(&request)
            .unwrap_err()
            .to_string()
            .contains("No AMF session")
    );
}

#[test]
fn create_session_reuse_returns_live_terminal_with_label() {
    let (mut app, agent_id, terminal_id) = app_with_terminal_and_agent({
        let mut tmux = MockTmuxOps::new();
        tmux.expect_window_exists().return_const(true);
        tmux.expect_paste_text()
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux.expect_send_key_name().times(0);
        tmux
    });
    let request = crate::automation::CreateSessionRequest {
        feature_session_id: Some(agent_id),
        kind: "terminal".into(),
        label: Some("dev server".into()),
        command: Some("rs".into()),
        reuse: true,
        ..Default::default()
    };
    let response = app.create_session_from_request(&request).unwrap();
    assert!(response.reused);
    assert_eq!(response.session_id.as_deref(), Some(terminal_id.as_str()));
    assert_eq!(app.store.projects[0].features[0].sessions.len(), 2);
}

#[test]
fn list_and_capture_sessions() {
    use crate::automation::{CaptureSessionRequest, FeatureTarget, ListSessionsRequest};
    let (app, agent_id, terminal_id) = app_with_terminal_and_agent({
        let mut tmux = MockTmuxOps::new();
        tmux.expect_window_exists().return_const(true);
        tmux.expect_capture_pane_history()
            .withf(|_, _, lines| *lines == 5000)
            .returning(|_, _, _| Ok("ready on :3000\n\n\n".into()));
        tmux
    });
    let target = FeatureTarget {
        feature_session_id: Some(agent_id),
        ..Default::default()
    };
    let listed = app
        .list_sessions_from_request(&ListSessionsRequest {
            target: target.clone(),
        })
        .unwrap();
    assert_eq!(listed.sessions.len(), 2);
    assert!(listed.sessions.iter().all(|s| s.running));
    assert_eq!(listed.sessions[1].kind, "terminal");

    let captured = app
        .capture_session_from_request(&CaptureSessionRequest {
            target,
            session: crate::automation::SessionSelector {
                label: Some("dev server".into()),
                ..Default::default()
            },
            lines: Some(999_999),
        })
        .unwrap();
    assert_eq!(captured.session_id, terminal_id);
    assert_eq!(captured.text, "ready on :3000");
}

#[test]
fn send_input_and_remove_refuse_agent_sessions_and_bad_keys() {
    use crate::automation::{
        FeatureTarget, RemoveSessionRequest, SendSessionInputRequest, SessionSelector,
    };
    let (mut app, agent_id, terminal_id) = app_with_terminal_and_agent({
        let mut tmux = MockTmuxOps::new();
        tmux.expect_window_exists().return_const(true);
        tmux.expect_send_key_name()
            .withf(|_, _, key| key == "C-c")
            .times(1)
            .returning(|_, _, _| Ok(()));
        tmux
    });
    let target = FeatureTarget {
        feature_session_id: Some(agent_id.clone()),
        ..Default::default()
    };
    let agent = SessionSelector {
        session_id: Some(agent_id),
        ..Default::default()
    };
    let terminal = SessionSelector {
        session_id: Some(terminal_id),
        ..Default::default()
    };
    let send = |key: &str, session: &SessionSelector| SendSessionInputRequest {
        target: target.clone(),
        session: session.clone(),
        key: Some(key.into()),
        ..Default::default()
    };
    let err = app
        .send_session_input_from_request(&send("C-c", &agent))
        .unwrap_err();
    assert!(err.to_string().contains("agent session"));
    assert!(
        app.send_session_input_from_request(&send("-t", &terminal))
            .is_err()
    );
    app.send_session_input_from_request(&send("C-c", &terminal))
        .unwrap();
    assert!(
        app.remove_session_from_request(&RemoveSessionRequest {
            target,
            session: agent,
        })
        .is_err()
    );
    assert_eq!(app.store.projects[0].features[0].sessions.len(), 2);
}

#[test]
fn create_session_notes_are_stored_replaced_and_removed_with_the_terminal() {
    use crate::app::terminal_notes::{load, sidebar_text};
    let dir = TempDir::new().unwrap();
    let (mut app, agent_id, _) = app_with_terminal_and_agent({
        let mut tmux = MockTmuxOps::new();
        tmux.expect_session_exists().return_const(true);
        tmux.expect_window_exists().return_const(true);
        tmux.expect_create_window().returning(|_, _, _| Ok(()));
        tmux.expect_kill_window().returning(|_, _| Ok(()));
        tmux
    });
    app.store.projects[0].features[0].workdir = dir.path().into();
    let request = crate::automation::CreateSessionRequest {
        feature_session_id: Some(agent_id.clone()),
        kind: "terminal".into(),
        label: Some("web".into()),
        notes: Some("Run `npm run dev`, then open :3000".into()),
        ..Default::default()
    };
    let created = app.create_session_from_request(&request).unwrap();
    let id = created.session_id.clone().unwrap();
    assert_eq!(sidebar_text(dir.path()).as_deref(), Some("web"));
    let entries = load(dir.path());
    assert_eq!(entries[0].id, id);
    assert!(entries[0].body.contains("open :3000"));

    // Reusing the terminal replaces its notes rather than adding a second block.
    let mut again = request.clone();
    again.reuse = true;
    again.notes = Some("Now run the tests".into());
    let reused = app.create_session_from_request(&again).unwrap();
    assert!(reused.reused);
    let entries = load(dir.path());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].body, "Now run the tests");

    // Removing the terminal removes its notes, and with them the section.
    app.remove_session_from_request(&crate::automation::RemoveSessionRequest {
        target: crate::automation::FeatureTarget {
            feature_session_id: Some(agent_id),
            ..Default::default()
        },
        session: crate::automation::SessionSelector {
            session_id: Some(id),
            ..Default::default()
        },
    })
    .unwrap();
    assert!(sidebar_text(dir.path()).is_none());
}

#[test]
fn create_session_rejects_notes_for_agents_and_oversize_notes() {
    let (mut app, agent_id, _) = app_with_terminal_and_agent(MockTmuxOps::new());
    let base = crate::automation::CreateSessionRequest {
        feature_session_id: Some(agent_id),
        kind: "terminal".into(),
        dry_run: true,
        notes: Some("n".into()),
        ..Default::default()
    };
    assert!(app.create_session_from_request(&base).is_ok());
    let mut agent = base.clone();
    agent.kind = "claude".into();
    assert!(
        app.create_session_from_request(&agent)
            .unwrap_err()
            .to_string()
            .contains("notes require a terminal")
    );
    let mut big = base;
    big.notes = Some("n".repeat(crate::app::terminal_notes::MAX_NOTES_BYTES + 1));
    assert!(app.create_session_from_request(&big).is_err());
}
