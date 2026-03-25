use super::setup::{
    cleanup_agent_injected_files, ensure_notification_hooks, strip_between_markers,
};
use super::steering::PromptConstraint;
use super::sync::pane_shows_thinking_hint;
use super::util::{
    latest_prompt_path, read_all_prompts, read_latest_prompt, shorten_path, slugify,
};
use super::*;
use crate::automation::{CreateBatchFeaturesRequest, CreateFeatureRequest, CreateProjectRequest};
use crate::extension::{ExtensionConfig, HookConfig, HookPrompt, LifecycleHooks};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

// ── slugify ───────────────────────────────────────────────

#[test]
fn slugify_spaces_become_hyphens() {
    assert_eq!(slugify("hello world"), "hello-world");
}

#[test]
fn slugify_special_chars_become_hyphens() {
    assert_eq!(slugify("foo/bar.baz"), "foo-bar-baz");
}

#[test]
fn slugify_consecutive_hyphens_collapsed() {
    assert_eq!(slugify("foo--bar"), "foo-bar");
}

#[test]
fn slugify_empty_input() {
    assert_eq!(slugify(""), "");
}

#[test]
fn slugify_all_specials() {
    assert_eq!(slugify("!@#$%"), "");
}

#[test]
fn slugify_preserves_hyphens() {
    assert_eq!(slugify("my-feature"), "my-feature");
}

// ── shorten_path ──────────────────────────────────────────

#[test]
fn shorten_path_inside_home() {
    if let Some(home) = dirs::home_dir() {
        let path = home.join("projects").join("my-app");
        let result = shorten_path(&path);
        assert_eq!(result, "~/projects/my-app");
    }
}

#[test]
fn shorten_path_outside_home() {
    let path = std::path::Path::new("/tmp/some/path");
    let result = shorten_path(path);
    assert_eq!(result, "/tmp/some/path");
}

#[test]
fn read_latest_prompt_prefers_claude_path() {
    let workdir = TempDir::new().unwrap();
    let claude_path = latest_prompt_path(workdir.path());
    let codex_path = workdir.path().join(".codex").join("latest-prompt.txt");
    std::fs::create_dir_all(claude_path.parent().unwrap()).unwrap();
    std::fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
    std::fs::write(&claude_path, "claude prompt").unwrap();
    std::fs::write(&codex_path, "codex prompt").unwrap();

    assert_eq!(
        read_latest_prompt(workdir.path()).as_deref(),
        Some("claude prompt")
    );
}

#[test]
fn read_latest_prompt_falls_back_to_codex_path() {
    let workdir = TempDir::new().unwrap();
    let codex_path = workdir.path().join(".codex").join("latest-prompt.txt");
    std::fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
    std::fs::write(&codex_path, "codex prompt").unwrap();

    assert_eq!(
        read_latest_prompt(workdir.path()).as_deref(),
        Some("codex prompt")
    );
}

#[test]
fn read_all_prompts_falls_back_to_codex_latest_prompt_file() {
    let workdir = TempDir::new().unwrap();
    let codex_path = workdir.path().join(".codex").join("latest-prompt.txt");
    std::fs::create_dir_all(codex_path.parent().unwrap()).unwrap();
    std::fs::write(&codex_path, "codex prompt history").unwrap();

    let prompts = read_all_prompts(workdir.path());
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].text, "codex prompt history");
}

// ── AppConfig defaults ───────────────────────────────────

#[test]
fn app_config_default_leader_timeout_is_five_seconds() {
    let config = AppConfig::default();
    assert_eq!(config.leader_timeout_seconds, 5);
}

#[test]
fn app_config_default_diff_review_viewer_is_amf() {
    let config = AppConfig::default();
    assert_eq!(config.diff_review_viewer, DiffReviewViewer::Amf);
}

#[test]
fn app_config_default_diff_viewer_layout_is_unified() {
    let config = AppConfig::default();
    assert_eq!(config.diff_viewer_layout, DiffViewerLayout::Unified);
}

#[test]
fn app_config_missing_leader_timeout_uses_default() {
    let config: AppConfig = serde_json::from_str(r#"{"nerd_font":false}"#).unwrap();
    assert_eq!(config.leader_timeout_seconds, 5);
    assert!(!config.nerd_font);
}

#[test]
fn app_config_missing_diff_review_viewer_uses_amf_default() {
    let config: AppConfig = serde_json::from_str(r#"{"nerd_font":false}"#).unwrap();
    assert_eq!(config.diff_review_viewer, DiffReviewViewer::Amf);
}

#[test]
fn app_config_missing_diff_viewer_layout_uses_unified_default() {
    let config: AppConfig = serde_json::from_str(r#"{"nerd_font":false}"#).unwrap();
    assert_eq!(config.diff_viewer_layout, DiffViewerLayout::Unified);
}

#[test]
fn app_config_diff_review_viewer_deserializes_amf() {
    let config: AppConfig = serde_json::from_str(r#"{"diff_review_viewer":"amf"}"#).unwrap();
    assert_eq!(config.diff_review_viewer, DiffReviewViewer::Amf);
}

#[test]
fn app_config_diff_review_viewer_deserializes_nvim() {
    let config: AppConfig = serde_json::from_str(r#"{"diff_review_viewer":"nvim"}"#).unwrap();
    assert_eq!(config.diff_review_viewer, DiffReviewViewer::Nvim);
}

#[test]
fn app_config_diff_review_viewer_accepts_custom_alias() {
    let config: AppConfig = serde_json::from_str(r#"{"diff_review_viewer":"custom"}"#).unwrap();
    assert_eq!(config.diff_review_viewer, DiffReviewViewer::Amf);
}

#[test]
fn app_config_diff_review_viewer_accepts_legacy_alias() {
    let config: AppConfig = serde_json::from_str(r#"{"diff_review_viewer":"legacy"}"#).unwrap();
    assert_eq!(config.diff_review_viewer, DiffReviewViewer::Nvim);
}

#[test]
fn app_config_missing_projects_uses_default_preferred_agent_none() {
    let config: AppConfig = serde_json::from_str(r#"{"nerd_font":false}"#).unwrap();
    assert_eq!(config.projects.default_preferred_agent, None);
}

#[test]
fn app_config_projects_default_preferred_agent_deserializes() {
    let config: AppConfig =
        serde_json::from_str(r#"{"projects":{"default_preferred_agent":"codex"}}"#).unwrap();
    assert_eq!(
        config.projects.default_preferred_agent,
        Some(AgentKind::Codex)
    );
}

#[test]
fn default_project_preferred_agent_comes_from_config() {
    let mut app = App::new_for_test(
        ProjectStore {
            version: 4,
            projects: vec![],
            session_bookmarks: vec![],
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.projects.default_preferred_agent = Some(AgentKind::Opencode);

    assert_eq!(app.default_project_preferred_agent(), AgentKind::Opencode);
}

// ── strip_between_markers ─────────────────────────────────

#[test]
fn strip_between_markers_basic_removal() {
    let result = strip_between_markers(
        "hello <!-- BEGIN -->REMOVED<!-- END --> world",
        "<!-- BEGIN -->",
        "<!-- END -->",
    );
    assert_eq!(result, "hello  world");
}

#[test]
fn strip_between_markers_eats_trailing_newline() {
    let result = strip_between_markers(
        "before <!-- BEGIN -->X<!-- END -->\nafter",
        "<!-- BEGIN -->",
        "<!-- END -->",
    );
    assert_eq!(result, "before after");
}

#[test]
fn strip_between_markers_eats_leading_blank_line() {
    let result = strip_between_markers(
        "before\n\n<!-- BEGIN -->X<!-- END -->\nafter",
        "<!-- BEGIN -->",
        "<!-- END -->",
    );
    assert_eq!(result, "before\nafter");
}

#[test]
fn strip_between_markers_absent_returns_unchanged() {
    let s = "no markers here";
    let result = strip_between_markers(s, "<!-- BEGIN -->", "<!-- END -->");
    assert_eq!(result, "no markers here");
}

#[test]
fn strip_between_markers_adjacent_markers() {
    let result = strip_between_markers(
        "<!-- BEGIN --><!-- END -->",
        "<!-- BEGIN -->",
        "<!-- END -->",
    );
    assert_eq!(result, "");
}

// ── thinking hint parsing ─────────────────────────────────

#[test]
fn pane_shows_thinking_hint_detects_supported_markers() {
    assert!(pane_shows_thinking_hint("Esc to interrupt"));
    assert!(pane_shows_thinking_hint("press ESC interrupt to stop"));
    assert!(pane_shows_thinking_hint("Ctrl+C to interrupt"));
}

#[test]
fn pane_shows_thinking_hint_ignores_unrelated_text() {
    assert!(!pane_shows_thinking_hint("waiting for input"));
    assert!(!pane_shows_thinking_hint("all done"));
}

// ── prompt steering analysis ──────────────────────────────

#[test]
fn analyze_prompt_flags_missing_constraint_categories() {
    let analysis = analyze_prompt("Add a steering coach dialog before launch.");

    assert_eq!(analysis.score, 0);
    assert_eq!(analysis.checks.len(), 5);
    assert!(
        analysis
            .missing_checks()
            .any(|check| check.constraint == PromptConstraint::FileScope)
    );
    assert!(
        analysis
            .missing_checks()
            .any(|check| check.constraint == PromptConstraint::ValidationCommands)
    );
}

#[test]
fn analyze_prompt_rewards_concrete_constraints() {
    let analysis = analyze_prompt(
        "Update only src/app/feature_ops.rs and src/ui/dialogs/feature.rs. \
         Done when the feature creation flow shows coaching before launch. \
         Do not change the session picker flow. \
         Run `cargo check`. \
         Watch out for SuperVibe confirmation and tmux launch behavior.",
    );

    assert_eq!(analysis.score, analysis.max_score);
    assert_eq!(analysis.missing_checks().count(), 0);
}

// ── ZaiPlanConfig::get_monthly_limit ─────────────────────

#[test]
fn zai_free_plan_monthly_limit() {
    let config = ZaiPlanConfig {
        plan: "free".to_string(),
        ..Default::default()
    };
    assert_eq!(config.get_monthly_limit(), Some(10_000_000));
}

#[test]
fn zai_coding_plan_monthly_limit() {
    let config = ZaiPlanConfig {
        plan: "coding-plan".to_string(),
        ..Default::default()
    };
    assert_eq!(config.get_monthly_limit(), Some(500_000_000));
}

#[test]
fn zai_unlimited_plan_monthly_limit_is_none() {
    let config = ZaiPlanConfig {
        plan: "unlimited".to_string(),
        ..Default::default()
    };
    assert_eq!(config.get_monthly_limit(), None);
}

#[test]
fn zai_custom_plan_monthly_limit_is_none() {
    let config = ZaiPlanConfig {
        plan: "enterprise".to_string(),
        ..Default::default()
    };
    assert_eq!(config.get_monthly_limit(), None);
}

#[test]
fn zai_explicit_token_limit_overrides_plan() {
    let config = ZaiPlanConfig {
        plan: "free".to_string(),
        monthly_token_limit: Some(999),
        ..Default::default()
    };
    assert_eq!(config.get_monthly_limit(), Some(999));
}

// ── Phase 3: App integration tests using mock trait objects ──

use crate::project::{
    AgentKind, Feature, FeatureSession, Project, SessionKind, TokenUsageSourceMatch,
};
use crate::token_tracking::{SessionTokenTracker, TokenUsageProvider, TokenUsageSource};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use chrono::{Duration, TimeZone, Utc};
use tempfile::NamedTempFile;

/// Build a minimal `ProjectStore` with one project and one
/// feature at the requested status.
fn store_with_feature(status: ProjectStatus) -> ProjectStore {
    let now = Utc::now();
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: PathBuf::from("/tmp/test-workdir"),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::default(),
        enable_chrome: false,
        pending_worktree_script: false,
        ready: false,
        status,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
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
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        extra: HashMap::new(),
    }
}

fn store_with_repo(repo: PathBuf, status: ProjectStatus) -> ProjectStore {
    let now = Utc::now();
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: repo.clone(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::default(),
        enable_chrome: false,
        pending_worktree_script: false,
        ready: false,
        status,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo,
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: false,
    };
    ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        extra: HashMap::new(),
    }
}

fn make_session(label: &str, status_text: Option<&str>) -> FeatureSession {
    FeatureSession {
        id: format!("session-{label}"),
        kind: SessionKind::Claude,
        label: label.to_string(),
        tmux_window: label.to_string(),
        claude_session_id: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at: Utc::now(),
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: status_text.map(str::to_string),
    }
}

// ── sync_statuses ─────────────────────────────────────────────

#[test]
fn sync_statuses_stopped_becomes_idle_when_session_live() {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_sessions()
        .times(1)
        .returning(|| Ok(vec!["amf-my-feat".to_string()]));

    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.sync_statuses();

    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Idle
    );
}

#[test]
fn sync_statuses_active_becomes_stopped_when_session_gone() {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_sessions()
        .times(1)
        .returning(|| Ok(vec![]));

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.sync_statuses();

    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
}

#[test]
fn sync_statuses_idle_stays_idle_when_session_live() {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_sessions()
        .times(1)
        .returning(|| Ok(vec!["amf-my-feat".to_string()]));

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.sync_statuses();

    // Already Idle; stays Idle (not overwritten)
    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Idle
    );
}

#[test]
fn visible_items_prioritizes_non_worktree_features() {
    let now = Utc::now();
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: PathBuf::from("/tmp/test-repo"),
        collapsed: false,
        features: vec![
            Feature {
                id: "feat-worktree".to_string(),
                name: "worktree-newer".to_string(),
                branch: "worktree-newer".to_string(),
                workdir: PathBuf::from("/tmp/test-repo/.worktrees/worktree-newer"),
                is_worktree: true,
                tmux_session: "amf-worktree-newer".to_string(),
                sessions: vec![],
                collapsed: false,
                mode: VibeMode::default(),
                review: false,
                plan_mode: false,
                agent: AgentKind::default(),
                enable_chrome: false,
                pending_worktree_script: false,
                ready: false,
                status: ProjectStatus::Stopped,
                created_at: now + Duration::minutes(1),
                last_accessed: now + Duration::minutes(1),
                summary: None,
                summary_updated_at: None,
                nickname: None,
            },
            Feature {
                id: "feat-repo".to_string(),
                name: "repo-older".to_string(),
                branch: "repo-older".to_string(),
                workdir: PathBuf::from("/tmp/test-repo"),
                is_worktree: false,
                tmux_session: "amf-repo-older".to_string(),
                sessions: vec![],
                collapsed: false,
                mode: VibeMode::default(),
                review: false,
                plan_mode: false,
                agent: AgentKind::default(),
                enable_chrome: false,
                pending_worktree_script: false,
                ready: false,
                status: ProjectStatus::Stopped,
                created_at: now,
                last_accessed: now,
                summary: None,
                summary_updated_at: None,
                nickname: None,
            },
        ],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: true,
    };
    let store = ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        extra: HashMap::new(),
    };

    let app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let visible = app.visible_items();

    assert!(matches!(visible[0], VisibleItem::Project(0)));
    assert!(matches!(visible[1], VisibleItem::Feature(0, 1)));
    assert!(matches!(visible[2], VisibleItem::Feature(0, 0)));
}

#[test]
fn ensure_selection_visible_accounts_for_multi_line_sessions() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].sessions = vec![
        make_session("claude-1", Some("running")),
        make_session("claude-2", Some("running")),
        make_session("claude-3", None),
    ];

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Session(0, 0, 1);

    app.ensure_selection_visible(4);

    assert_eq!(app.scroll_offset, 2);
}

#[test]
fn item_index_at_visible_row_maps_status_line_to_same_session() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].sessions = vec![
        make_session("claude-1", Some("running")),
        make_session("claude-2", None),
    ];

    let app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    assert!(matches!(app.item_index_at_visible_row(0, 4), Some(0)));
    assert!(matches!(app.item_index_at_visible_row(1, 4), Some(1)));
    assert!(matches!(app.item_index_at_visible_row(2, 4), Some(2)));
    assert!(matches!(app.item_index_at_visible_row(3, 4), Some(2)));
    assert_eq!(app.item_index_at_visible_row(4, 4), None);
}

#[test]
fn start_worktree_hook_adds_pending_feature_immediately() {
    let repo = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let now = Utc::now();
    let store = ProjectStore {
        version: 2,
        projects: vec![Project {
            id: "proj-1".to_string(),
            name: "my-project".to_string(),
            repo: repo.path().to_path_buf(),
            collapsed: true,
            features: vec![],
            created_at: now,
            preferred_agent: AgentKind::Claude,
            is_git: true,
        }],
        session_bookmarks: vec![],
        extra: std::collections::HashMap::new(),
    };
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.start_worktree_hook(
        "true",
        workdir.path().to_path_buf(),
        "my-project".to_string(),
        "new-feature".to_string(),
        VibeMode::default(),
        false,
        false,
        AgentKind::Claude,
        false,
        false,
        None,
    );

    assert!(matches!(app.mode, AppMode::RunningHook(_)));
    assert!(matches!(app.selection, Selection::Feature(0, 0)));
    assert_eq!(app.store.projects[0].features.len(), 1);

    let feature = &app.store.projects[0].features[0];
    assert_eq!(feature.name, "new-feature");
    assert_eq!(feature.workdir, workdir.path());
    assert!(feature.is_worktree);
    assert!(feature.pending_worktree_script);
    assert_eq!(feature.status, ProjectStatus::Stopped);
}

#[test]
fn start_feature_is_blocked_while_worktree_script_pending() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.store.projects[0].features[0].pending_worktree_script = true;

    app.start_feature().unwrap();

    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("worktree script")
    );
    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
}

#[test]
fn complete_running_hook_clears_pending_state_and_starts_feature() {
    let repo = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let now = Utc::now();
    let mut feature = Feature::new(
        "new-feature".to_string(),
        "new-feature".to_string(),
        workdir.path().to_path_buf(),
        true,
        VibeMode::default(),
        false,
        false,
        AgentKind::Claude,
        false,
    );
    feature.pending_worktree_script = true;
    let store = ProjectStore {
        version: 2,
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
        extra: std::collections::HashMap::new(),
    };

    let workdir_path = workdir.path().to_path_buf();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists()
        .withf(|session| session == "amf-new-feature")
        .times(1)
        .return_const(false);
    let expected_workdir = workdir_path.clone();
    tmux.expect_create_session_with_window()
        .withf(move |session, window, workdir| {
            session == "amf-new-feature" && window == "claude" && workdir == expected_workdir
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .withf(|session, key, value| {
            session == "amf-new-feature" && key == "AMF_SESSION" && value == "amf-new-feature"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    let expected_workdir = workdir_path.clone();
    tmux.expect_create_window()
        .withf(move |session, window, workdir| {
            session == "amf-new-feature" && window == "terminal" && workdir == expected_workdir
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .withf(|session, window, resume_id, extra_args| {
            session == "amf-new-feature"
                && window == "claude"
                && resume_id.is_none()
                && extra_args.is_empty()
        })
        .times(1)
        .returning(|_, _, _, _| Ok(()));
    tmux.expect_select_window()
        .withf(|session, window| session == "amf-new-feature" && window == "claude")
        .times(1)
        .returning(|_, _| Ok(()));

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.selection = Selection::Feature(0, 0);
    app.mode = AppMode::RunningHook(RunningHookState {
        script: "true".to_string(),
        workdir: workdir_path,
        project_name: "my-project".to_string(),
        branch: "new-feature".to_string(),
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::Claude,
        enable_chrome: false,
        steering_enabled: false,
        child: None,
        output: String::new(),
        success: Some(true),
        output_rx: None,
    });

    app.complete_running_hook().unwrap();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(matches!(app.selection, Selection::Feature(0, 0)));
    let feature = &app.store.projects[0].features[0];
    assert!(!feature.pending_worktree_script);
    assert_eq!(feature.status, ProjectStatus::Idle);
    assert_eq!(feature.sessions.len(), 2);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Created and started feature 'new-feature'")
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
        branch: branch.to_string(),
        step: CreateFeatureStep::Branch,
        agent: AgentKind::default(),
        agent_index: 0,
        mode: VibeMode::default(),
        mode_index: 0,
        mode_focus: 0,
        review: false,
        plan_mode: false,
        source_index: 0,
        worktrees: vec![],
        worktree_index: 0,
        use_worktree,
        enable_chrome: false,
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
            assert!(state.steering_enabled);
        }
        _ => panic!("expected CreatingFeature mode"),
    }
}

fn startup_prompt_overlay_test(agent: AgentKind, expected_window: &'static str) {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().join(".worktrees").join("coached");
    std::fs::create_dir_all(&workdir).unwrap();
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
        extra: std::collections::HashMap::new(),
    };

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists()
        .withf(|session| session == "amf-coached")
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
                .returning(|_, _, _, _| Ok(()));
        }
        AgentKind::Opencode => {
            tmux.expect_launch_opencode()
                .times(1)
                .returning(|_, _| Ok(()));
        }
        AgentKind::Codex => {
            tmux.expect_launch_codex()
                .times(1)
                .withf(|session, window, resume| {
                    session == "amf-coached" && window == "codex" && resume.is_none()
                })
                .returning(|_, _, _| Ok(()));
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
        branch: "coached".to_string(),
        step: CreateFeatureStep::Mode,
        agent: agent.clone(),
        agent_index: 0,
        mode: mode.clone(),
        mode_index: 0,
        mode_focus: 0,
        review: false,
        plan_mode: false,
        source_index: 0,
        worktrees: vec![],
        worktree_index: 0,
        use_worktree: true,
        enable_chrome: false,
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
        agent,
        enable_chrome: false,
        steering_enabled: true,
        hook_succeeded: None,
        startup_prompt: None,
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
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Stopped,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
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
        extra: HashMap::new(),
    };

    let resized = Arc::new(AtomicBool::new(false));
    let resized_for_launch = resized.clone();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists()
        .withf(|session| session == "amf-restore-me")
        .times(2)
        .return_const(false);
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_create_window()
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_resize_pane()
        .times(2)
        .withf(|session, window, cols, rows| {
            session == "amf-restore-me"
                && (window == "claude" || window == "terminal")
                && *cols == 120
                && *rows == 40
        })
        .returning(move |_, window, _, _| {
            if window == "claude" {
                resized.store(true, Ordering::SeqCst);
            }
            Ok(())
        });
    tmux.expect_launch_claude()
        .times(1)
        .withf(move |session, window, resume_id, extra_args| {
            resized_for_launch.load(Ordering::SeqCst)
                && session == "amf-restore-me"
                && window == "claude"
                && resume_id.as_deref() == Some("claude-session-123")
                && extra_args.is_empty()
        })
        .returning(|_, _, _, _| Ok(()));
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
    app.mode = AppMode::ConfirmingClaudeSession {
        session_id: "claude-session-123".to_string(),
        workdir,
    };

    app.confirm_and_start_claude().unwrap();

    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert_eq!(app.message.as_deref(), Some("Restored claude session"));
    assert!(matches!(app.selection, Selection::Session(0, 0, 0)));
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
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .times(1)
        .returning(|_, _, _, _| Ok(()));
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
        agent: AgentKind::Claude,
        enable_chrome: false,
        steering_enabled: false,
        hook_succeeded: None,
        startup_prompt: None,
    })
    .unwrap();

    let settings =
        std::fs::read_to_string(workdir.join(".claude").join("settings.local.json")).unwrap();
    assert!(
        settings.contains("custom-diff-review.sh"),
        "expected vibeless worktree creation to inject custom diff review hook, got: {settings}"
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
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_opencode()
        .times(1)
        .returning(|_, _| Ok(()));
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
        agent: AgentKind::Opencode,
        enable_chrome: false,
        steering_enabled: false,
        hook_succeeded: None,
        startup_prompt: None,
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
}

#[test]
fn refresh_opencode_plugins_overwrites_stale_change_tracker_plugin() {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().join(".worktrees").join("diffy-opencode");
    let plugin_dir = workdir.join(".opencode").join("plugins");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(plugin_dir.join("change-tracker.js"), "stale plugin").unwrap();

    super::setup::ensure_notify_scripts();

    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped);
    let feature = &mut store.projects[0].features[0];
    feature.workdir = workdir.clone();
    feature.is_worktree = true;
    feature.agent = AgentKind::Opencode;
    feature.mode = VibeMode::Vibeless;

    let refreshed = super::setup::refresh_opencode_plugins_for_store(&store);
    assert_eq!(refreshed, 1);

    let installed = std::fs::read_to_string(plugin_dir.join("change-tracker.js")).unwrap();
    assert!(
        installed.contains("original_file")
            && installed.contains("proposed_file")
            && installed.contains("buildReviewFiles"),
        "expected stale change-tracker.js to be replaced with the structured diff-review version, got: {installed}"
    );
}

#[test]
fn submit_steering_prompt_pastes_into_running_session() {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().join(".worktrees").join("coached");
    std::fs::create_dir_all(&workdir).unwrap();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_paste_text()
        .withf(|session, window, text| {
            session == "amf-coached"
                && window == "claude"
                && text == "Implement steering coach automatically."
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|session, window, key| {
            session == "amf-coached" && window == "claude" && key == "Enter"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::SteeringPrompt(SteeringPromptState::new(
        ViewState::new(
            "my-project".to_string(),
            "coached".to_string(),
            "amf-coached".to_string(),
            "claude".to_string(),
            "Claude 1".to_string(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            false,
        ),
        workdir.clone(),
        "Implement steering coach automatically.".to_string(),
    ));

    app.submit_steering_prompt().unwrap();

    match &app.mode {
        AppMode::Viewing(view) => {
            assert_eq!(view.session, "amf-coached");
            assert_eq!(view.window, "claude");
        }
        _ => panic!("expected Viewing mode"),
    }

    let prompt_path = workdir.join(".claude").join("latest-prompt.txt");
    assert_eq!(
        std::fs::read_to_string(prompt_path).unwrap(),
        "Implement steering coach automatically."
    );
}

#[test]
fn inject_latest_prompt_pastes_into_running_session() {
    let repo = TempDir::new().unwrap();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_paste_text()
        .withf(|session, window, text| {
            session == "amf-coached"
                && window == "claude"
                && text == "Resume from the latest saved prompt."
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|session, window, key| {
            session == "amf-coached" && window == "claude" && key == "Enter"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));

    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::LatestPrompt(LatestPromptState {
        view: ViewState::new(
            "my-project".to_string(),
            "coached".to_string(),
            "amf-coached".to_string(),
            "claude".to_string(),
            "Claude 1".to_string(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            false,
        ),
        prompts: vec![crate::app::util::PromptEntry {
            text: "Resume from the latest saved prompt.".to_string(),
            timestamp: None,
        }],
        selected: 0,
    });

    app.inject_latest_prompt().unwrap();

    match &app.mode {
        AppMode::Viewing(view) => {
            assert_eq!(view.session, "amf-coached");
            assert_eq!(view.window, "claude");
        }
        _ => panic!("expected Viewing mode"),
    }
}

#[test]
fn create_project_persists_selected_preferred_agent() {
    let repo = TempDir::new().unwrap();
    let tmp = NamedTempFile::new().unwrap();
    let store = ProjectStore {
        version: 4,
        projects: vec![],
        session_bookmarks: vec![],
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
        }
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
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Stopped,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
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
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Stopped,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
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
}

// ── ensure_notification_hooks ─────────────────────────────

use tempfile::TempDir;

fn read_settings(dir: &TempDir) -> serde_json::Value {
    let path = dir.path().join(".claude").join("settings.local.json");
    let s = std::fs::read_to_string(&path).expect("settings.local.json should exist");
    serde_json::from_str(&s).expect("valid JSON")
}

fn hook_commands_for(settings: &serde_json::Value, event: &str) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| {
            entry["hooks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|h| h["command"].as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .collect()
}

fn call_ensure_hooks_for(workdir: &TempDir, mode: VibeMode, agent: AgentKind, is_worktree: bool) {
    let repo = workdir.path(); // repo = workdir in tests
    ensure_notification_hooks(workdir.path(), repo, &mode, &agent, is_worktree);
}

fn call_ensure_hooks_for_with_config(
    workdir: &TempDir,
    mode: VibeMode,
    agent: AgentKind,
    is_worktree: bool,
    config: &AppConfig,
) {
    let repo = workdir.path(); // repo = workdir in tests
    super::setup::ensure_notification_hooks_with_config(
        workdir.path(),
        repo,
        &mode,
        &agent,
        is_worktree,
        config,
    );
}

fn call_ensure_hooks(workdir: &TempDir, mode: VibeMode) {
    call_ensure_hooks_for(workdir, mode, AgentKind::Claude, true);
}

#[test]
fn stop_hook_has_thinking_stop_and_notify() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks(&workdir, VibeMode::Vibe);
    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "Stop");
    assert!(
        cmds.iter().any(|c| c.contains("thinking-stop.sh")),
        "Stop hook missing thinking-stop.sh; got: {cmds:?}"
    );
    assert!(
        cmds.iter().any(|c| c.contains("notify.sh")),
        "Stop hook missing notify.sh; got: {cmds:?}"
    );
}

#[test]
fn pre_tool_use_hook_has_thinking_tool_and_clear() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks(&workdir, VibeMode::Vibe);
    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "PreToolUse");
    assert!(
        cmds.iter().any(|c| c.contains("thinking-start.sh")),
        "PreToolUse missing thinking-start.sh; got: {cmds:?}"
    );
    assert!(
        cmds.iter().any(|c| c.contains("tool-start.sh")),
        "PreToolUse missing tool-start.sh; got: {cmds:?}"
    );
    assert!(
        cmds.iter().any(|c| c.contains("clear-notify.sh")),
        "PreToolUse missing clear-notify.sh; got: {cmds:?}"
    );
}

#[test]
fn post_tool_use_hook_has_tool_stop() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks(&workdir, VibeMode::Vibe);
    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "PostToolUse");
    assert!(
        cmds.iter().any(|c| c.contains("tool-stop.sh")),
        "PostToolUse missing tool-stop.sh; got: {cmds:?}"
    );
}

#[test]
fn notification_hook_is_removed() {
    let workdir = TempDir::new().unwrap();
    // Pre-populate with the legacy Notification hook.
    let claude_dir = workdir.path().join(".claude");
    let notify_cmd = crate::project::amf_config_dir()
        .join("notify.sh")
        .to_string_lossy()
        .into_owned();
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.local.json"),
        serde_json::json!({
            "hooks": {
                "Notification": [{
                    "matcher": "",
                    "hooks": [{
                        "type": "command",
                        "command": notify_cmd
                    }]
                }]
            }
        })
        .to_string(),
    )
    .unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibe);

    let s = read_settings(&workdir);
    assert!(
        s["hooks"].get("Notification").is_none(),
        "legacy Notification hook should be removed"
    );
}

#[test]
fn claude_hooks_use_settings_local_json() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks(&workdir, VibeMode::Vibe);

    assert!(
        workdir
            .path()
            .join(".claude")
            .join("settings.local.json")
            .exists(),
        "Claude hooks should be written to settings.local.json"
    );
    assert!(
        !workdir
            .path()
            .join(".claude")
            .join("settings.json")
            .exists(),
        "Claude hook injection should avoid settings.json"
    );
}

#[test]
fn claude_hooks_preserve_existing_user_hooks() {
    let workdir = TempDir::new().unwrap();
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.local.json"),
        r#"{"hooks":{"Stop":[{"matcher":"custom","hooks":[{"type":"command","command":"/tmp/user-stop.sh"}]}]}}"#,
    )
    .unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibe);

    let s = read_settings(&workdir);
    let stop_entries = s["hooks"]["Stop"].as_array().cloned().unwrap_or_default();
    assert!(
        stop_entries
            .iter()
            .any(|entry| entry["matcher"].as_str() == Some("custom")),
        "custom Stop hook should be preserved"
    );
    let cmds = hook_commands_for(&s, "Stop");
    assert!(
        cmds.iter().any(|cmd| cmd == "/tmp/user-stop.sh"),
        "custom Stop command should still exist"
    );
    assert!(
        cmds.iter().any(|cmd| cmd.contains("thinking-stop.sh")),
        "AMF Stop command should still be injected"
    );
}

#[test]
fn vibeless_pre_tool_use_includes_custom_diff_review_when_script_present_by_default() {
    let workdir = TempDir::new().unwrap();
    // Create the custom diff-review script so it gets picked up.
    let scripts_dir = workdir
        .path()
        .join("plugins")
        .join("diff-review")
        .join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();
    std::fs::write(scripts_dir.join("custom-diff-review.sh"), "").unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibeless);

    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "PreToolUse");
    assert!(
        cmds.iter().any(|c| c.contains("custom-diff-review.sh")),
        "Vibeless PreToolUse should include custom-diff-review; got: {cmds:?}"
    );
}

#[test]
fn vibeless_pre_tool_use_can_use_legacy_diff_review_when_configured() {
    let workdir = TempDir::new().unwrap();
    let scripts_dir = workdir
        .path()
        .join("plugins")
        .join("diff-review")
        .join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();
    std::fs::write(scripts_dir.join("diff-review.sh"), "").unwrap();

    let config = AppConfig {
        diff_review_viewer: DiffReviewViewer::Nvim,
        ..AppConfig::default()
    };
    call_ensure_hooks_for_with_config(
        &workdir,
        VibeMode::Vibeless,
        AgentKind::Claude,
        true,
        &config,
    );

    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "PreToolUse");
    assert!(
        cmds.iter().any(|c| c.contains("diff-review.sh")),
        "Vibeless PreToolUse should include legacy diff-review; got: {cmds:?}"
    );
}

#[test]
fn vibeless_permissions_include_edit_and_write() {
    let workdir = TempDir::new().unwrap();
    // Need custom diff-review script for vibeless path to complete.
    let scripts_dir = workdir
        .path()
        .join("plugins")
        .join("diff-review")
        .join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();
    std::fs::write(scripts_dir.join("custom-diff-review.sh"), "").unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibeless);

    let s = read_settings(&workdir);
    let allow = s["permissions"]["allow"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let strs: Vec<&str> = allow.iter().filter_map(|v| v.as_str()).collect();
    assert!(strs.contains(&"Edit"), "permissions should allow Edit");
    assert!(strs.contains(&"Write"), "permissions should allow Write");
}

#[test]
fn vibe_mode_strips_edit_write_permissions_left_from_vibeless() {
    let workdir = TempDir::new().unwrap();
    // Pre-populate with permissions that would have been added by vibeless.
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.local.json"),
        r#"{"permissions":{"allow":["Edit","Write","Bash"]}}"#,
    )
    .unwrap();
    std::fs::write(
        claude_dir.join("amf-hook-state.json"),
        r#"{"permissions_added":["Edit","Write"]}"#,
    )
    .unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibe);

    let s = read_settings(&workdir);
    let allow = s["permissions"]["allow"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let strs: Vec<&str> = allow.iter().filter_map(|v| v.as_str()).collect();
    assert!(
        !strs.contains(&"Edit"),
        "Edit should be removed for Vibe mode"
    );
    assert!(
        !strs.contains(&"Write"),
        "Write should be removed for Vibe mode"
    );
    // Unrelated permissions are preserved.
    assert!(
        strs.contains(&"Bash"),
        "unrelated permissions should remain"
    );
}

#[test]
fn ensure_hooks_is_idempotent() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks(&workdir, VibeMode::Vibe);
    let first = read_settings(&workdir);
    call_ensure_hooks(&workdir, VibeMode::Vibe);
    let second = read_settings(&workdir);
    assert_eq!(
        first, second,
        "calling twice should produce identical output"
    );
}

#[test]
fn codex_hooks_are_injected_for_repo_root_and_worktrees() {
    let workdir = TempDir::new().unwrap();

    call_ensure_hooks_for(&workdir, VibeMode::Vibe, AgentKind::Codex, false);
    assert!(
        workdir.path().join(".codex").join("config.toml").exists(),
        "repo-root codex feature should get local codex config"
    );
    assert!(
        workdir
            .path()
            .join(".codex")
            .join("amf-codex-notify.sh")
            .exists(),
        "repo-root codex feature should get local notify hook script"
    );

    let second = TempDir::new().unwrap();
    call_ensure_hooks_for(&second, VibeMode::Vibe, AgentKind::Codex, true);
    assert!(
        second.path().join(".codex").join("config.toml").exists(),
        "worktree codex feature should get local codex config"
    );
    assert!(
        second
            .path()
            .join(".codex")
            .join("amf-codex-notify.sh")
            .exists(),
        "worktree codex feature should get local notify hook script"
    );
}

#[test]
fn codex_hook_merges_existing_notify_entries() {
    let workdir = TempDir::new().unwrap();
    let codex_dir = workdir.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let cfg = codex_dir.join("config.toml");
    std::fs::write(&cfg, "notify = [\"/tmp/existing-hook.sh\"]\n").unwrap();

    call_ensure_hooks_for(&workdir, VibeMode::Vibe, AgentKind::Codex, true);

    let rendered = std::fs::read_to_string(&cfg).unwrap();
    let parsed: toml::Value = toml::from_str(&rendered).unwrap();
    let notify = parsed
        .get("notify")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let entries: Vec<String> = notify
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    assert!(
        entries.iter().any(|entry| entry == "/tmp/existing-hook.sh"),
        "existing Codex notify entry should be preserved"
    );
    assert!(
        entries
            .iter()
            .any(|e| e.ends_with("/.codex/amf-codex-notify.sh")),
        "amf codex notify hook should be added"
    );
    assert_eq!(
        entries.len(),
        2,
        "notify should merge user and AMF commands"
    );
}

#[test]
fn cleanup_claude_hooks_removes_amf_artifacts() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks_for(&workdir, VibeMode::Vibeless, AgentKind::Claude, true);

    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(claude_dir.join("notifications")).unwrap();
    std::fs::write(claude_dir.join("latest-prompt.txt"), "prompt").unwrap();

    cleanup_agent_injected_files(workdir.path(), &AgentKind::Claude);

    let settings_path = claude_dir.join("settings.local.json");
    assert!(
        !settings_path.exists(),
        "cleanup should remove settings.local.json when only AMF hooks were present"
    );
    assert!(
        !claude_dir.join("notifications").exists(),
        "cleanup should remove Claude notification directory"
    );
    assert!(
        !claude_dir.join("latest-prompt.txt").exists(),
        "cleanup should remove Claude latest prompt file"
    );
}

#[test]
fn cleanup_claude_hooks_preserves_user_settings() {
    let workdir = TempDir::new().unwrap();
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.local.json"),
        r#"{"hooks":{"Stop":[{"matcher":"custom","hooks":[{"type":"command","command":"/tmp/user-stop.sh"}]}]},"permissions":{"allow":["Bash"]}}"#,
    )
    .unwrap();

    call_ensure_hooks_for(&workdir, VibeMode::Vibe, AgentKind::Claude, true);
    cleanup_agent_injected_files(workdir.path(), &AgentKind::Claude);

    let rendered = std::fs::read_to_string(claude_dir.join("settings.local.json")).unwrap();
    let settings: serde_json::Value = serde_json::from_str(&rendered).unwrap();
    let cmds = hook_commands_for(&settings, "Stop");
    assert_eq!(cmds, vec!["/tmp/user-stop.sh".to_string()]);
    let allow = settings["permissions"]["allow"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let strs: Vec<&str> = allow.iter().filter_map(|value| value.as_str()).collect();
    assert_eq!(strs, vec!["Bash"]);
}

#[test]
fn cleanup_codex_hooks_restores_previous_notify_command() {
    let workdir = TempDir::new().unwrap();
    let codex_dir = workdir.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let cfg = codex_dir.join("config.toml");
    std::fs::write(&cfg, "notify = [\"/tmp/existing-hook.sh\"]\n").unwrap();

    call_ensure_hooks_for(&workdir, VibeMode::Vibe, AgentKind::Codex, true);
    cleanup_agent_injected_files(workdir.path(), &AgentKind::Codex);

    let rendered = std::fs::read_to_string(&cfg).unwrap();
    let parsed: toml::Value = toml::from_str(&rendered).unwrap();
    let notify = parsed
        .get("notify")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let entries: Vec<String> = notify
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    assert_eq!(
        entries,
        vec!["/tmp/existing-hook.sh".to_string()],
        "cleanup should restore the previous Codex notify command"
    );
    assert!(
        !codex_dir.join("amf-codex-notify.sh").exists(),
        "cleanup should remove AMF Codex hook script"
    );
    assert!(
        !codex_dir.join("amf-codex-notify-original.json").exists(),
        "cleanup should remove any legacy Codex sidecar backup"
    );
}

fn store_with_worktree_agent(
    repo: &std::path::Path,
    workdir: &std::path::Path,
    agent: AgentKind,
    status: ProjectStatus,
    sessions: Vec<crate::project::FeatureSession>,
) -> ProjectStore {
    let now = Utc::now();
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.to_path_buf(),
        is_worktree: true,
        tmux_session: "amf-my-feat".to_string(),
        sessions,
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent,
        enable_chrome: false,
        pending_worktree_script: false,
        ready: false,
        status,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: repo.to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: true,
    };
    ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        extra: HashMap::new(),
    }
}

#[test]
fn apply_session_config_switches_agent_and_rewrites_agent_sessions() {
    let repo = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();

    ensure_notification_hooks(
        workdir.path(),
        repo.path(),
        &VibeMode::Vibe,
        &AgentKind::Claude,
        true,
    );

    let now = Utc::now();
    let sessions = vec![
        crate::project::FeatureSession {
            id: "agent-session".to_string(),
            kind: SessionKind::Claude,
            label: "Claude 1".to_string(),
            tmux_window: "claude".to_string(),
            claude_session_id: Some("resume-me".to_string()),
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: now,
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
        },
        crate::project::FeatureSession {
            id: "terminal-session".to_string(),
            kind: SessionKind::Terminal,
            label: "Terminal 1".to_string(),
            tmux_window: "terminal".to_string(),
            claude_session_id: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: now,
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
        },
    ];

    let mut store = store_with_worktree_agent(
        repo.path(),
        workdir.path(),
        AgentKind::Claude,
        ProjectStatus::Stopped,
        sessions,
    );
    store.projects[0].features[0].mode = VibeMode::Vibe;
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists()
        .withf(|session| session == "amf-my-feat")
        .times(1)
        .return_const(false);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.selection = Selection::Feature(0, 0);

    app.start_session_config().unwrap();
    if let AppMode::SessionConfig(state) = &mut app.mode {
        state.selected_agent = state
            .allowed_agents
            .iter()
            .position(|agent| *agent == AgentKind::Codex)
            .unwrap();
    } else {
        panic!("session config dialog should be open");
    }

    app.apply_session_config().unwrap();

    let feature = &app.store.projects[0].features[0];
    assert_eq!(feature.agent, AgentKind::Codex);
    assert_eq!(feature.sessions[0].kind, SessionKind::Codex);
    assert_eq!(feature.sessions[0].label, "Codex 1");
    assert_eq!(feature.sessions[0].tmux_window, "codex");
    assert_eq!(feature.sessions[0].claude_session_id, None);
    assert_eq!(feature.sessions[1].kind, SessionKind::Terminal);
    assert!(
        !workdir
            .path()
            .join(".claude")
            .join("settings.local.json")
            .exists(),
        "Claude hook settings should be removed after switching away"
    );
    assert!(
        workdir.path().join(".codex").join("config.toml").exists(),
        "Codex config should be injected after switching"
    );
}

#[test]
fn apply_project_agent_config_updates_preferred_agent_only() {
    let now = Utc::now();
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: PathBuf::from("/tmp/test-repo"),
        collapsed: false,
        features: vec![],
        created_at: now,
        preferred_agent: AgentKind::Claude,
        is_git: false,
    };
    let store = ProjectStore {
        version: 4,
        projects: vec![project],
        session_bookmarks: vec![],
        extra: HashMap::new(),
    };
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.selection = Selection::Project(0);

    app.start_project_agent_config().unwrap();
    if let AppMode::ProjectAgentConfig(state) = &mut app.mode {
        state.selected_agent = state
            .allowed_agents
            .iter()
            .position(|agent| *agent == AgentKind::Opencode)
            .unwrap();
    } else {
        panic!("project config dialog should be open");
    }

    app.apply_session_config().unwrap();

    assert_eq!(app.store.projects[0].preferred_agent, AgentKind::Opencode);
    assert!(
        app.store.projects[0].features.is_empty(),
        "changing project preference should not create or mutate features"
    );
}

// ── sync_session_status ──────────────────────────────────────

fn store_with_custom_session(workdir: &std::path::Path, session_id: &str) -> ProjectStore {
    let now = Utc::now();
    let session = FeatureSession {
        id: session_id.to_string(),
        kind: SessionKind::Custom,
        label: "Dev Servers".to_string(),
        tmux_window: "custom".to_string(),
        claude_session_id: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at: now,
        command: Some("./start.sh".to_string()),
        on_stop: None,
        pre_check: None,
        status_text: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::default(),
        enable_chrome: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: workdir.to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: false,
    };
    ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        extra: HashMap::new(),
    }
}

fn store_with_codex_session(workdir: &std::path::Path, is_worktree: bool) -> ProjectStore {
    let now = Utc::now();
    let session = FeatureSession {
        id: "codex-sess".to_string(),
        kind: SessionKind::Codex,
        label: "Codex".to_string(),
        tmux_window: "codex".to_string(),
        claude_session_id: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at: now,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.to_path_buf(),
        is_worktree,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::Codex,
        enable_chrome: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: workdir.to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: false,
    };
    ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        extra: HashMap::new(),
    }
}

#[test]
fn sync_session_status_reads_first_line() {
    let workdir = TempDir::new().unwrap();
    let session_id = "test-sess-123";
    let status_dir = workdir.path().join(".amf").join("session-status");
    std::fs::create_dir_all(&status_dir).unwrap();
    std::fs::write(
        status_dir.join(format!("{}.txt", session_id)),
        "API :3000 | DB :5432\nextra line\n",
    )
    .unwrap();

    let store = store_with_custom_session(workdir.path(), session_id);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        Some("API :3000 | DB :5432".to_string()),
    );
}

#[test]
fn sync_session_status_shows_agent_token_usage() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let encoded = workdir
        .path()
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>();
    let claude_dir = home.path().join(".claude").join("projects").join(encoded);
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("claude-123.jsonl"),
        "{\"requestId\":\"req-1\",\"message\":{\"id\":\"msg-1\",\"usage\":{\"input_tokens\":12,\"output_tokens\":4}}}\n",
    )
    .unwrap();

    let now = Utc::now();
    let session = FeatureSession {
        id: "claude-sess".to_string(),
        kind: SessionKind::Claude,
        label: "Claude 1".to_string(),
        tmux_window: "claude".to_string(),
        claude_session_id: None,
        token_usage_source: Some(TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "claude-123".to_string(),
        }),
        token_usage_source_match: Some(TokenUsageSourceMatch::Exact),
        created_at: now,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.path().to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: true,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::Claude,
        enable_chrome: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Stopped,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: workdir.path().to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::Claude,
        is_git: false,
    };
    let store = ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        extra: HashMap::new(),
    };

    let mut tracker = SessionTokenTracker::new(
        Some(home.path().to_path_buf()),
        Some(data.path().to_path_buf()),
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status_with_tracker(&mut tracker);

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        Some("12 in · 4 out · 32 eff · <$0.01".to_string()),
    );
    assert_eq!(
        app.store.projects[0].features[0].sessions[0].token_usage_source_match,
        Some(TokenUsageSourceMatch::Exact),
    );
}

#[test]
fn sync_session_status_marks_discovered_codex_usage_as_inferred() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let session_dir = home
        .path()
        .join(".codex")
        .join("sessions")
        .join("2026")
        .join("03")
        .join("13");
    std::fs::create_dir_all(&session_dir).unwrap();
    std::fs::write(
        session_dir.join("rollout.jsonl"),
        format!(
            concat!(
                "{{\"timestamp\":\"2026-03-13T14:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"codex-1\",\"cwd\":\"{}\"}}}}\n",
                "{{\"timestamp\":\"2026-03-13T14:01:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":100,\"cached_input_tokens\":40,\"output_tokens\":7,\"reasoning_output_tokens\":3,\"total_tokens\":110}}}}}}}}\n"
            ),
            workdir.path().display()
        ),
    )
    .unwrap();

    let created_at = Utc.with_ymd_and_hms(2026, 3, 13, 13, 59, 30).unwrap();
    let session = FeatureSession {
        id: "codex-sess".to_string(),
        kind: SessionKind::Codex,
        label: "Codex".to_string(),
        tmux_window: "codex".to_string(),
        claude_session_id: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.path().to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::Codex,
        enable_chrome: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
        created_at,
        last_accessed: created_at,
        summary: None,
        summary_updated_at: None,
        nickname: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: workdir.path().to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at,
        preferred_agent: AgentKind::Codex,
        is_git: false,
    };
    let store = ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        extra: HashMap::new(),
    };

    let mut tracker = SessionTokenTracker::new(
        Some(home.path().to_path_buf()),
        Some(data.path().to_path_buf()),
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status_with_tracker(&mut tracker);

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "codex-1".to_string(),
        }),
    );
    assert_eq!(
        app.store.projects[0].features[0].sessions[0].token_usage_source_match,
        Some(TokenUsageSourceMatch::Inferred),
    );
}

#[test]
fn note_codex_prompt_submit_marks_repo_root_feature_thinking() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.pending_inputs.push(PendingInput {
        session_id: "amf-my-feat".to_string(),
        cwd: workdir.path().display().to_string(),
        message: "Codex finished and is waiting for input".to_string(),
        notification_type: "input-request".to_string(),
        file_path: PathBuf::new(),
        target_file_path: None,
        relative_path: None,
        change_id: None,
        tool: None,
        old_snippet: None,
        new_snippet: None,
        original_file: None,
        proposed_file: None,
        is_new_file: None,
        reason: None,
        response_file: None,
        project_name: Some("my-project".to_string()),
        feature_name: Some("my-feat".to_string()),
        proceed_signal: None,
        request_id: None,
        reply_socket: None,
    });

    app.note_codex_prompt_submit("amf-my-feat", "codex");

    assert!(
        app.ipc_thinking_sessions.contains("amf-my-feat"),
        "repo-root codex feature should be marked thinking"
    );
    assert!(
        app.pending_inputs.is_empty(),
        "prompt submit should clear stale input-request notifications"
    );
}

#[test]
fn apply_codex_live_event_updates_feature_live_state() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    let changed = app.apply_codex_live_event(
        "amf-my-feat",
        &serde_json::json!({
            "type": "plan",
            "payload": { "text": "1. Inspect\n2. Patch" }
        }),
    );

    assert!(changed);
    let live = app
        .codex_live_thread("amf-my-feat")
        .expect("expected live codex state");
    assert_eq!(live.plan_text.as_deref(), Some("1. Inspect\n2. Patch"));
}

#[test]
fn poll_codex_sidebar_metadata_updates_caches() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let cache_key = format!("{}::sess-current", workdir.path().display());
    app.codex_sidebar_metadata_inflight
        .insert(cache_key.clone());
    app.codex_sidebar_metadata_tx
        .send(CodexSidebarMetadataResult {
            cache_key: cache_key.clone(),
            title: Some("Sidebar title".into()),
            prompt: Some("Sidebar prompt".into()),
        })
        .unwrap();

    app.poll_codex_sidebar_metadata();

    assert_eq!(
        app.cached_codex_session_title(workdir.path(), "sess-current"),
        Some("Sidebar title")
    );
    assert_eq!(
        app.cached_codex_session_prompt(workdir.path(), "sess-current"),
        Some("Sidebar prompt")
    );
    assert!(!app.codex_sidebar_metadata_inflight.contains(&cache_key));
}

#[test]
fn ipc_input_request_updates_codex_live_work_state() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "input-request",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Need approval before applying the patch."
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text())
            .as_deref(),
        Some("Pending input: Need approval before applying the patch.")
    );
}

#[test]
fn ipc_prompt_submit_clears_codex_live_input_and_marks_thinking() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.apply_codex_live_event(
        "amf-my-feat",
        &serde_json::json!({
            "type": "requestUserInput",
            "payload": { "prompt": "Need approval before applying the patch." }
        }),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "prompt-submit",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "prompt": "Continue with the patch."
    }));

    assert!(app.ipc_thinking_sessions.contains("amf-my-feat"));
    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text()),
        None
    );
}

#[test]
fn ipc_prompt_submit_refreshes_sidebar_plan_cache() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("plan.md"),
        "# Plan\n\n1. Refresh plan cache\n2. Render update\n",
    )
    .unwrap();

    app.handle_ipc_message_value(serde_json::json!({
        "type": "prompt-submit",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "prompt": "Continue with the patch."
    }));

    assert_eq!(
        app.sidebar_plan_for_session("amf-my-feat"),
        Some("Plan\n1. Refresh plan cache\n2. Render update")
    );
}

#[test]
fn open_command_picker_prepends_codex_debug_commands() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);

    app.open_command_picker(None);

    match &app.mode {
        AppMode::CommandPicker(state) => {
            assert!(!state.commands.is_empty(), "expected commands");
            assert_eq!(state.commands[0].source, "AMF Debug");
            assert_eq!(state.commands[0].name, "demo-plan");
            assert!(matches!(
                state.commands[0].action,
                CommandAction::CodexLiveDemo(CodexDebugCommand::PlanDemo)
            ));
        }
        _ => panic!("expected command picker"),
    }
}

#[test]
fn custom_diff_review_notification_opens_prompt_while_viewing() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_worktree_agent(
        workdir.path(),
        workdir.path(),
        AgentKind::Claude,
        ProjectStatus::Idle,
        vec![FeatureSession {
            id: "claude-1".to_string(),
            kind: SessionKind::Claude,
            label: "Claude".to_string(),
            tmux_window: "claude".to_string(),
            claude_session_id: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
        }],
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "claude".to_string(),
        "Claude".to_string(),
        SessionKind::Claude,
        VibeMode::Vibe,
        false,
    ));

    let notify_dir = workdir.path().join(".claude").join("notifications");
    std::fs::create_dir_all(&notify_dir).unwrap();
    let notification = serde_json::json!({
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Review: src/main.rs",
        "type": "diff-review",
        "file_path": workdir.path().join("src/main.rs").display().to_string(),
        "relative_path": "src/main.rs",
        "tool": "edit",
        "change_id": "chg-1",
        "old_snippet": "old",
        "new_snippet": "new",
        "response_file": workdir.path().join("response.json").display().to_string(),
        "proceed_signal": workdir.path().join("proceed").display().to_string()
    });
    std::fs::write(
        notify_dir.join("diff-review.json"),
        serde_json::to_string(&notification).unwrap(),
    )
    .unwrap();

    app.scan_notifications();

    match &app.mode {
        AppMode::DiffReviewPrompt(state) => {
            assert_eq!(state.relative_path, "src/main.rs");
            assert_eq!(state.tool, "edit");
            assert_eq!(state.old_snippet, "old");
            assert_eq!(state.new_snippet, "new");
        }
        _ => panic!("expected diff review prompt"),
    }
}

#[test]
fn custom_diff_review_notification_marks_new_files_as_added() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "amf-my-feat");
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "claude".to_string(),
        "Claude".to_string(),
        SessionKind::Claude,
        VibeMode::Vibeless,
        false,
    ));

    let temp_dir = workdir.path().join("tmp-review");
    std::fs::create_dir_all(&temp_dir).unwrap();
    let original = temp_dir.join("original.md");
    let proposed = temp_dir.join("proposed.md");
    std::fs::write(&original, "").unwrap();
    std::fs::write(&proposed, "# Plan\n").unwrap();

    let notify_dir = workdir.path().join(".claude").join("notifications");
    std::fs::create_dir_all(&notify_dir).unwrap();
    let notification = serde_json::json!({
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Review: plans/new.md",
        "type": "diff-review",
        "file_path": workdir.path().join("plans/new.md").display().to_string(),
        "relative_path": "plans/new.md",
        "tool": "write",
        "change_id": "chg-new",
        "old_snippet": "",
        "new_snippet": "# Plan\n",
        "original_file": original.display().to_string(),
        "proposed_file": proposed.display().to_string(),
        "is_new_file": true,
        "response_file": workdir.path().join("response.json").display().to_string(),
        "proceed_signal": workdir.path().join("proceed").display().to_string()
    });
    std::fs::write(
        notify_dir.join("diff-review-new-file.json"),
        serde_json::to_string(&notification).unwrap(),
    )
    .unwrap();

    app.scan_notifications();

    match &app.mode {
        AppMode::DiffReviewPrompt(state) => {
            let file = state.diff_file.as_ref().expect("expected parsed diff file");
            assert_eq!(file.status, crate::diff::DiffFileStatus::Added);
            assert_eq!(file.path, "plans/new.md");
            assert_eq!(state.layout, app.config.diff_viewer_layout);
        }
        _ => panic!("expected diff review prompt"),
    }
}

#[test]
fn custom_diff_review_notification_queues_from_normal_mode_and_opens_on_enter_view() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "amf-my-feat");
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(1).return_const(true);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Normal;

    let notify_dir = workdir.path().join(".claude").join("notifications");
    std::fs::create_dir_all(&notify_dir).unwrap();
    let notification = serde_json::json!({
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Review: src/lib.rs",
        "type": "diff-review",
        "file_path": workdir.path().join("src/lib.rs").display().to_string(),
        "relative_path": "src/lib.rs",
        "tool": "write",
        "change_id": "chg-2",
        "old_snippet": "",
        "new_snippet": "new body",
        "response_file": workdir.path().join("response.json").display().to_string(),
        "proceed_signal": workdir.path().join("proceed").display().to_string()
    });
    std::fs::write(
        notify_dir.join("diff-review.json"),
        serde_json::to_string(&notification).unwrap(),
    )
    .unwrap();

    app.scan_notifications();

    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(app.pending_inputs.len(), 1);

    app.selection = Selection::Feature(0, 0);
    app.enter_view().unwrap();

    match &app.mode {
        AppMode::DiffReviewPrompt(state) => {
            assert_eq!(state.relative_path, "src/lib.rs");
            assert!(state.return_to_view.is_some());
        }
        _ => panic!("expected diff review prompt after entering view"),
    }
    assert!(app.pending_inputs.is_empty());
}

#[test]
fn contextual_syntax_install_returns_to_diff_viewer_and_refreshes() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let mut diff_viewer = DiffViewerState::new(
        ViewState::new(
            "proj".into(),
            "feat".into(),
            "sess".into(),
            "claude".into(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibe,
            false,
        ),
        workdir.path().to_path_buf(),
    );
    diff_viewer.files = vec![crate::diff::DiffFile {
        old_path: Some("src/main.rs".into()),
        path: "src/main.rs".into(),
        status: crate::diff::DiffFileStatus::Modified,
        additions: 1,
        deletions: 1,
        is_binary: false,
        old_content: Some("fn old() {}\n".into()),
        new_content: Some("fn new() {}\n".into()),
        patch: String::new(),
        hunks: vec![],
    }];

    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(SyntaxOperationEvent::Finished(Ok(
        "Installed Rust parser".to_string()
    )))
    .unwrap();
    drop(tx);

    app.mode = AppMode::SyntaxLanguagePicker(SyntaxLanguagePickerState {
        languages: vec![],
        selected: 0,
        notice: None,
        operation: Some(SyntaxOperationState {
            language: crate::highlight::HighlightLanguage::Rust,
            action: SyntaxOperationAction::Install,
            last_output: None,
            started_at: std::time::Instant::now(),
            output_rx: rx,
        }),
        return_to: Some(Box::new(AppMode::DiffViewer(diff_viewer))),
        auto_return_on_success: true,
        return_language: Some(crate::highlight::HighlightLanguage::Rust),
    });

    app.poll_syntax_language_picker().unwrap();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.error.is_some());
            assert!(state.files.is_empty());
        }
        _ => panic!("expected diff viewer after successful install"),
    }
}

#[test]
fn contextual_syntax_install_returns_to_diff_review_prompt() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(SyntaxOperationEvent::Finished(Ok(
        "Installed Rust parser".to_string()
    )))
    .unwrap();
    drop(tx);

    app.mode = AppMode::SyntaxLanguagePicker(SyntaxLanguagePickerState {
        languages: vec![],
        selected: 0,
        notice: None,
        operation: Some(SyntaxOperationState {
            language: crate::highlight::HighlightLanguage::Rust,
            action: SyntaxOperationAction::Install,
            last_output: None,
            started_at: std::time::Instant::now(),
            output_rx: rx,
        }),
        return_to: Some(Box::new(AppMode::DiffReviewPrompt(DiffReviewState {
            session_id: "sess-1".to_string(),
            workdir: workdir.path().to_path_buf(),
            file_path: workdir.path().join("src/main.rs").display().to_string(),
            relative_path: "src/main.rs".to_string(),
            change_id: "chg-1".to_string(),
            tool: "edit".to_string(),
            old_snippet: "old".to_string(),
            new_snippet: "new".to_string(),
            diff_file: None,
            diff_error: None,
            reason: String::new(),
            editing_feedback: false,
            layout: DiffViewerLayout::Unified,
            explanation: None,
            explanation_child: None,
            response_file: workdir.path().join("response.json"),
            proceed_signal: workdir.path().join("proceed"),
            request_id: None,
            reply_socket: None,
            return_to_view: None,
            patch_scroll: 0,
        }))),
        auto_return_on_success: true,
        return_language: Some(crate::highlight::HighlightLanguage::Rust),
    });

    app.poll_syntax_language_picker().unwrap();

    match &app.mode {
        AppMode::DiffReviewPrompt(state) => {
            assert_eq!(state.relative_path, "src/main.rs");
        }
        _ => panic!("expected diff review prompt after successful install"),
    }
}

#[test]
fn contextual_syntax_install_stays_open_for_non_matching_language() {
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let (tx, rx) = std::sync::mpsc::channel();
    tx.send(SyntaxOperationEvent::Finished(Ok(
        "Installed JSON parser".to_string()
    )))
    .unwrap();
    drop(tx);

    app.mode = AppMode::SyntaxLanguagePicker(SyntaxLanguagePickerState {
        languages: vec![],
        selected: 0,
        notice: None,
        operation: Some(SyntaxOperationState {
            language: crate::highlight::HighlightLanguage::Json,
            action: SyntaxOperationAction::Install,
            last_output: None,
            started_at: std::time::Instant::now(),
            output_rx: rx,
        }),
        return_to: Some(Box::new(AppMode::Normal)),
        auto_return_on_success: true,
        return_language: Some(crate::highlight::HighlightLanguage::Rust),
    });

    app.poll_syntax_language_picker().unwrap();

    match &app.mode {
        AppMode::SyntaxLanguagePicker(state) => {
            assert!(state.operation.is_none());
            assert_eq!(state.notice.as_deref(), Some("Installed JSON parser"));
        }
        _ => panic!("expected syntax picker to remain open"),
    }
}

#[test]
fn sync_session_status_none_when_file_missing() {
    let workdir = TempDir::new().unwrap();
    let session_id = "test-sess-456";
    // No status file created

    let store = store_with_custom_session(workdir.path(), session_id);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        None,
    );
}

#[test]
fn sync_session_status_none_when_file_empty() {
    let workdir = TempDir::new().unwrap();
    let session_id = "test-sess-789";
    let status_dir = workdir.path().join(".amf").join("session-status");
    std::fs::create_dir_all(&status_dir).unwrap();
    std::fs::write(status_dir.join(format!("{}.txt", session_id)), "").unwrap();

    let store = store_with_custom_session(workdir.path(), session_id);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        None,
    );
}

#[test]
fn sync_session_status_skips_non_custom_sessions() {
    let workdir = TempDir::new().unwrap();
    let now = Utc::now();

    // Create a Claude session (not Custom)
    let session = FeatureSession {
        id: "claude-sess".to_string(),
        kind: SessionKind::Claude,
        label: "Claude 1".to_string(),
        tmux_window: "claude".to_string(),
        claude_session_id: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at: now,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.path().to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: AgentKind::default(),
        enable_chrome: false,
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: workdir.path().to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: false,
    };
    let store = ProjectStore {
        version: 2,
        projects: vec![project],
        session_bookmarks: vec![],
        extra: HashMap::new(),
    };

    // Even if a status file exists for this ID, it should
    // be ignored because the session is not Custom.
    let status_dir = workdir.path().join(".amf").join("session-status");
    std::fs::create_dir_all(&status_dir).unwrap();
    std::fs::write(status_dir.join("claude-sess.txt"), "should be ignored").unwrap();

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        None,
    );
}

#[test]
fn on_stop_persists_on_feature_session() {
    let mut feat = crate::project::Feature::new(
        "test".to_string(),
        "test".to_string(),
        PathBuf::from("/tmp/test"),
        false,
        VibeMode::default(),
        false,
        false,
        AgentKind::default(),
        false,
    );
    let s = feat.add_custom_session_named(
        "Dev Servers".to_string(),
        "devservers".to_string(),
        Some("docker compose up".to_string()),
        Some("docker compose down".to_string()),
        None,
    );
    assert_eq!(s.on_stop, Some("docker compose down".to_string()));
    assert_eq!(s.command, Some("docker compose up".to_string()));
}

#[test]
fn on_stop_none_when_not_provided() {
    let mut feat = crate::project::Feature::new(
        "test".to_string(),
        "test".to_string(),
        PathBuf::from("/tmp/test"),
        false,
        VibeMode::default(),
        false,
        false,
        AgentKind::default(),
        false,
    );
    let s =
        feat.add_custom_session_named("Terminal".to_string(), "term".to_string(), None, None, None);
    assert_eq!(s.on_stop, None);
}

#[test]
fn status_file_cleanup_during_remove() {
    let workdir = TempDir::new().unwrap();
    let session_id = "cleanup-test-sess";
    let status_dir = workdir.path().join(".amf").join("session-status");
    std::fs::create_dir_all(&status_dir).unwrap();
    let status_file = status_dir.join(format!("{}.txt", session_id));
    std::fs::write(&status_file, "running").unwrap();
    assert!(status_file.exists());

    // Build a store with a custom session
    let store = store_with_custom_session(workdir.path(), session_id);

    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_sessions().returning(|| Ok(vec![]));

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));

    // Selecting the session and removing it should clean
    // up the status file.
    app.selection = Selection::Session(0, 0, 0);
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();
    app.remove_session().unwrap();

    assert!(
        !status_file.exists(),
        "status file should be removed on session removal"
    );
}

#[test]
fn sync_session_status_trims_whitespace() {
    let workdir = TempDir::new().unwrap();
    let session_id = "test-sess-trim";
    let status_dir = workdir.path().join(".amf").join("session-status");
    std::fs::create_dir_all(&status_dir).unwrap();
    std::fs::write(
        status_dir.join(format!("{}.txt", session_id)),
        "  API :3000  \n",
    )
    .unwrap();

    let store = store_with_custom_session(workdir.path(), session_id);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        Some("API :3000".to_string()),
    );
}

fn store_with_single_claude_session() -> ProjectStore {
    let now = Utc::now();
    let session = FeatureSession {
        id: "sess-1".to_string(),
        kind: SessionKind::Claude,
        label: "Claude 1".to_string(),
        tmux_window: "claude".to_string(),
        claude_session_id: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at: now,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
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
        pending_worktree_script: false,
        ready: false,
        status: ProjectStatus::Idle,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
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
fn jump_to_bookmark_enters_view_for_slot() {
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
    assert!(matches!(app.mode, AppMode::Viewing(_)));
}

fn store_with_empty_project(repo: PathBuf, is_git: bool) -> ProjectStore {
    let now = Utc::now();
    let project = Project {
        id: "proj-1".to_string(),
        name: "automation-project".to_string(),
        repo,
        collapsed: false,
        features: vec![],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git,
    };
    ProjectStore {
        version: 4,
        projects: vec![project],
        session_bookmarks: vec![],
        extra: HashMap::new(),
    }
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
    assert_eq!(response.workdir, repo.join(".worktrees").join("feature-1"));
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
    let worktree_path = repo.join(".worktrees").join("feature-1");

    let mut worktree = MockWorktreeOps::new();
    let repo_for_create = repo.clone();
    let worktree_clone = worktree_path.clone();
    worktree
        .expect_create()
        .times(1)
        .withf(move |repo_path, name, branch| {
            repo_path == repo_for_create.as_path() && name == "feature-1" && branch == "feature-1"
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
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_codex()
        .times(1)
        .returning(|_, _, _| Ok(()));
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
    assert!(app.store.projects[0].features[0].is_worktree);
    assert!(app.store.projects[0].features[0].review);
    assert_eq!(app.store.projects[0].features[0].sessions.len(), 2);
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
        repo.join(".worktrees").join("plan-1")
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
    let worktree_one = repo.join(".worktrees").join("plan-1");
    let worktree_two = repo.join(".worktrees").join("plan-2");
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
            repo_path == repo_for_first.as_path() && name == "plan-1" && branch == "plan-1"
        })
        .returning(move |_, _, _| Ok(worktree_one_clone.clone()));
    let repo_for_second = repo.clone();
    let worktree_two_clone = worktree_two.clone();
    worktree
        .expect_create()
        .times(1)
        .withf(move |repo_path, name, branch| {
            repo_path == repo_for_second.as_path() && name == "plan-2" && branch == "plan-2"
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
        .times(2)
        .returning(|_, _, _| Ok(()));
    tmux.expect_launch_codex()
        .times(2)
        .returning(|_, _, _| Ok(()));
    tmux.expect_select_window()
        .times(2)
        .returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        ProjectStore {
            version: 4,
            projects: vec![],
            session_bookmarks: vec![],
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
            .all(|feature| feature.sessions.len() == 2)
    );
}
