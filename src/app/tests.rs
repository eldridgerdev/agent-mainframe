use super::setup::{
    cleanup_agent_injected_files, ensure_notification_hooks, strip_between_markers,
};
use super::steering::PromptConstraint;
use super::sync::pane_shows_thinking_hint;
use super::util::{latest_prompt_path, read_latest_prompt, shorten_path, slugify};
use super::*;
use crate::automation::{CreateBatchFeaturesRequest, CreateFeatureRequest, CreateProjectRequest};
use crate::extension::{ExtensionConfig, HookConfig, HookPrompt, LifecycleHooks};
use crossterm::event::KeyCode;
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

fn prompt_entry(text: &str) -> String {
    text.to_string()
}

#[test]
fn startup_harness_setup_allows_quit_with_q() {
    let store = ProjectStore {
        version: 5,
        projects: vec![],
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

    app.open_harness_setup(true);
    crate::handlers::handle_harness_setup_key(&mut app, KeyCode::Char('q')).unwrap();

    assert!(app.should_quit);
}

#[test]
fn control_view_parser_cursor_move_keeps_relative_redraws_on_tmux_row() {
    let mut parser = vt100::Parser::new(6, 40, 0);
    parser.process(b"\x1b[2;1Hscreen");

    position_parser_cursor(&mut parser, (8, 1), 40, 6);
    parser.process(b"\r\x1b[10Cabc");

    assert_eq!(parser_cursor(&parser), Some((13, 1)));
    assert_eq!(
        parser
            .screen()
            .contents()
            .lines()
            .nth(1)
            .unwrap_or_default()
            .trim_end(),
        "screen    abc"
    );
}

#[test]
fn drain_view_snapshots_updates_rendered_lines_when_content_is_unchanged() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "terminal".to_string(),
        "Terminal".to_string(),
        SessionKind::Terminal,
        VibeMode::default(),
        false,
    ));
    app.pane_content_cols = 4;
    app.pane_content_rows = 1;
    app.pane_content = "same formatted content".to_string();
    app.pane_lines = vec![ratatui::text::Line::from("ABCD")];

    app.view_snapshot_tx.send(ViewSnapshot {
        session: "amf-my-feat".to_string(),
        window: "terminal".to_string(),
        pane_content: Some("same formatted content".to_string()),
        rendered_lines: Some(vec![ratatui::text::Line::from("A  D")]),
        cursor: None,
        capture_duration: None,
        render_duration: None,
        cursor_duration: None,
        pipe_read_duration: None,
    });

    let (pane_changed, cursor_changed) = app.drain_view_snapshots();

    assert!(pane_changed);
    assert!(!cursor_changed);
    assert_eq!(app.pane_content, "same formatted content");
    assert_eq!(app.pane_lines, vec![ratatui::text::Line::from("A  D")]);
}

#[test]
fn snapshot_mailbox_merges_cursor_only_update_with_pending_content() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "terminal".to_string(),
        "Terminal".to_string(),
        SessionKind::Terminal,
        VibeMode::default(),
        false,
    ));
    app.pane_content_cols = 4;
    app.pane_content_rows = 1;

    let base = |content: Option<&str>, cursor: Option<Option<(u16, u16)>>| ViewSnapshot {
        session: "amf-my-feat".to_string(),
        window: "terminal".to_string(),
        pane_content: content.map(|c| c.to_string()),
        rendered_lines: content.map(|_| vec![ratatui::text::Line::from("ABCD")]),
        cursor,
        capture_duration: None,
        render_duration: None,
        cursor_duration: None,
        pipe_read_duration: None,
    };

    // Content snapshot followed by a cursor-only snapshot before the
    // main loop drains: the merged slot must keep both.
    app.view_snapshot_tx.send(base(Some("hello"), None));
    app.view_snapshot_tx.send(base(None, Some(Some((2, 0)))));

    let (pane_changed, cursor_changed) = app.drain_view_snapshots();
    assert!(pane_changed);
    assert!(cursor_changed);
    assert_eq!(app.pane_content, "hello");
    assert_eq!(app.tmux_cursor, Some((2, 0)));

    // Slot is now empty.
    let (pane_changed, cursor_changed) = app.drain_view_snapshots();
    assert!(!pane_changed);
    assert!(!cursor_changed);
}

#[test]
fn frozen_display_holds_last_frame_until_freeze_lapses() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "terminal".to_string(),
        "Terminal".to_string(),
        SessionKind::Terminal,
        VibeMode::default(),
        false,
    ));
    app.pane_content_cols = 4;
    app.pane_content_rows = 1;
    app.pane_lines = vec![ratatui::text::Line::from("OLD ")];

    app.view_snapshot_tx.send(ViewSnapshot {
        session: "amf-my-feat".to_string(),
        window: "terminal".to_string(),
        pane_content: Some("new".to_string()),
        rendered_lines: Some(vec![ratatui::text::Line::from("NEW ")]),
        cursor: None,
        capture_duration: None,
        render_duration: None,
        cursor_duration: None,
        pipe_read_duration: None,
    });

    // While frozen, the pending frame is held back and the display keeps
    // the previous frame (this is what hides the re-anchor bounce).
    app.freeze_view_display(std::time::Duration::from_secs(30));
    let (pane_changed, _) = app.drain_view_snapshots();
    assert!(!pane_changed);
    assert_eq!(app.pane_lines, vec![ratatui::text::Line::from("OLD ")]);

    // Once the freeze lapses, the freshest frame is revealed.
    app.view_display_frozen_until = Some(std::time::Instant::now());
    let (pane_changed, _) = app.drain_view_snapshots();
    assert!(pane_changed);
    assert_eq!(app.pane_lines, vec![ratatui::text::Line::from("NEW ")]);
}

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
fn poll_sidebar_load_results_updates_feature_caches() {
    let repo = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Stopped),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.pending_sidebar_loads.insert("amf-my-feat".to_string());
    app.sidebar_load_tx
        .send(super::SidebarLoadResult {
            tmux_session: "amf-my-feat".to_string(),
            signature: 7,
            changed: true,
            latest_prompt: Some("lazy prompt".to_string()),
            model_text: Some("Model: openai/gpt-5.5".to_string()),
            opencode_sidebar: Some(crate::app::opencode_storage::OpencodeSidebarData {
                session_id: "ses-1".to_string(),
                title: Some("Loaded later".to_string()),
                latest_prompt: Some("lazy prompt".to_string()),
                status: Some("busy".to_string()),
                last_tool: Some("edit".to_string()),
                todo_count: Some(2),
                todo_preview: vec!["finish parser".to_string(), "wire UI".to_string()],
                pending_permission: None,
                last_error: None,
                lsp_summary: Some("ready".to_string()),
                live_summary: Some("live summary".to_string()),
                model: Some("gpt-5.5".to_string()),
                provider: Some("openai".to_string()),
                reasoning_tokens: Some(12),
                additions: Some(3),
                deletions: Some(1),
                files: Some(1),
            }),
        })
        .unwrap();

    app.poll_sidebar_load_results();

    assert_eq!(
        app.latest_prompt_for_session("amf-my-feat"),
        Some("lazy prompt")
    );
    assert_eq!(
        app.opencode_sidebar_cache
            .get("amf-my-feat")
            .and_then(|data| data.title.as_deref()),
        Some("Loaded later")
    );
    assert_eq!(
        app.sidebar_model_cache
            .get("amf-my-feat")
            .map(String::as_str),
        Some("Model: openai/gpt-5.5")
    );
    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
}

// ── AppConfig defaults ───────────────────────────────────

#[test]
fn app_config_default_leader_timeout_is_five_seconds() {
    let config = AppConfig::default();
    assert_eq!(config.leader_timeout_seconds, 5);
}

#[test]
fn app_config_default_input_request_wait_is_one_point_five_seconds() {
    let config = AppConfig::default();
    assert_eq!(
        config.input_request_wait_duration(),
        std::time::Duration::from_millis(1500)
    );
}

#[test]
fn app_config_default_tmux_control_mode_is_disabled() {
    // Direct transport is the default; control mode is opt-in.
    let config = AppConfig::default();
    assert!(!config.tmux_control_mode);
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
fn app_config_missing_input_request_wait_uses_default() {
    let config: AppConfig = serde_json::from_str(r#"{"nerd_font":false}"#).unwrap();
    assert_eq!(config.input_request_wait_seconds, 1.5);
}

#[test]
fn app_config_input_request_wait_can_be_configured() {
    let config: AppConfig = serde_json::from_str(r#"{"input_request_wait_seconds":0.75}"#).unwrap();
    assert_eq!(
        config.input_request_wait_duration(),
        std::time::Duration::from_millis(750)
    );
}

#[test]
fn app_config_invalid_input_request_wait_falls_back_to_default_duration() {
    let config: AppConfig = serde_json::from_str(r#"{"input_request_wait_seconds":-1.0}"#).unwrap();
    assert_eq!(
        config.input_request_wait_duration(),
        std::time::Duration::from_millis(1500)
    );
}

#[test]
fn app_config_missing_tmux_control_mode_uses_default() {
    let config: AppConfig = serde_json::from_str(r#"{"nerd_font":false}"#).unwrap();
    assert!(!config.tmux_control_mode);
}

#[test]
fn app_config_tmux_control_mode_can_be_disabled() {
    let config: AppConfig = serde_json::from_str(r#"{"tmux_control_mode":false}"#).unwrap();
    assert!(!config.tmux_control_mode);
}

#[test]
fn app_config_tmux_control_mode_can_be_enabled() {
    let config: AppConfig = serde_json::from_str(r#"{"tmux_control_mode":true}"#).unwrap();
    assert!(config.tmux_control_mode);
}

#[test]
fn migrate_app_config_flips_pre_v1_defaults() {
    // A config written before versioning: control mode on, old popup hold.
    let mut config: AppConfig = serde_json::from_str(
        r#"{"tmux_control_mode":true,"diff_review_popup_hold_secs":3.0}"#,
    )
    .unwrap();
    assert_eq!(config.config_version, 0);

    let changed = crate::app::setup::migrate_app_config(&mut config);

    assert!(changed);
    assert!(!config.tmux_control_mode);
    assert_eq!(config.diff_review_popup_hold_secs, 1.5);
    assert_eq!(config.config_version, crate::app::APP_CONFIG_VERSION);
}

#[test]
fn migrate_app_config_preserves_deliberate_pre_v1_values() {
    // Pre-v1 config that already chose non-default values: only the
    // version is stamped, the user's choices are kept.
    let mut config: AppConfig = serde_json::from_str(
        r#"{"tmux_control_mode":false,"diff_review_popup_hold_secs":5.0}"#,
    )
    .unwrap();

    let changed = crate::app::setup::migrate_app_config(&mut config);

    assert!(changed);
    assert!(!config.tmux_control_mode);
    assert_eq!(config.diff_review_popup_hold_secs, 5.0);
    assert_eq!(config.config_version, crate::app::APP_CONFIG_VERSION);
}

#[test]
fn migrate_app_config_is_noop_when_current() {
    // An already-current config (e.g. someone who re-enabled control mode
    // after migrating) is left untouched and triggers no rewrite.
    let mut config = AppConfig {
        config_version: crate::app::APP_CONFIG_VERSION,
        tmux_control_mode: true,
        diff_review_popup_hold_secs: 3.0,
        ..AppConfig::default()
    };

    let changed = crate::app::setup::migrate_app_config(&mut config);

    assert!(!changed);
    assert!(config.tmux_control_mode);
    assert_eq!(config.diff_review_popup_hold_secs, 3.0);
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
fn app_config_diff_review_viewer_nvim_maps_to_amf() {
    // The legacy vimdiff viewer was retired; old configs still load.
    let config: AppConfig = serde_json::from_str(r#"{"diff_review_viewer":"nvim"}"#).unwrap();
    assert_eq!(config.diff_review_viewer, DiffReviewViewer::Amf);
}

#[test]
fn app_config_diff_review_viewer_accepts_custom_alias() {
    let config: AppConfig = serde_json::from_str(r#"{"diff_review_viewer":"custom"}"#).unwrap();
    assert_eq!(config.diff_review_viewer, DiffReviewViewer::Amf);
}

#[test]
fn app_config_diff_review_viewer_accepts_legacy_alias() {
    let config: AppConfig = serde_json::from_str(r#"{"diff_review_viewer":"legacy"}"#).unwrap();
    assert_eq!(config.diff_review_viewer, DiffReviewViewer::Amf);
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
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
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
    tmux_session_name, worktree_name,
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
        remote_control: false,
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
        remote_control: false,
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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

#[test]
fn redraw_signature_changes_when_view_selection_moves() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "claude".to_string(),
        "Claude".to_string(),
        SessionKind::Claude,
        VibeMode::default(),
        false,
    ));

    let initial = app.redraw_signature();
    if let AppMode::Viewing(view) = &mut app.mode {
        view.selection.is_selecting = true;
        view.selection.start_row = 1;
        view.selection.start_col = 2;
        view.selection.end_row = 1;
        view.selection.end_col = 2;
    }
    let selection_started = app.redraw_signature();

    if let AppMode::Viewing(view) = &mut app.mode {
        view.selection.end_col = 8;
        view.selection.has_selection = true;
    }
    let selection_dragged = app.redraw_signature();

    assert_ne!(initial, selection_started);
    assert_ne!(selection_started, selection_dragged);
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
    app.sync_statuses();

    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
    assert!(app.latest_prompt_for_session("amf-my-feat").is_none());
    assert!(!app.opencode_sidebar_cache.contains_key("amf-my-feat"));
    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
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
fn sync_thinking_status_drains_sidebar_results_for_opencode_features() {
    let repo = TempDir::new().unwrap();
    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Idle);
    store.projects[0].features[0].agent = AgentKind::Opencode;

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.pending_sidebar_loads.insert("amf-my-feat".to_string());
    app.sidebar_load_tx
        .send(super::SidebarLoadResult {
            tmux_session: "amf-my-feat".to_string(),
            signature: 9,
            changed: true,
            latest_prompt: Some("warm prompt".to_string()),
            model_text: None,
            opencode_sidebar: Some(crate::app::opencode_storage::OpencodeSidebarData {
                session_id: "ses-1".to_string(),
                title: Some("Warm cache".to_string()),
                latest_prompt: Some("warm prompt".to_string()),
                status: Some("busy".to_string()),
                last_tool: Some("edit".to_string()),
                todo_count: Some(1),
                todo_preview: vec!["finish parser".to_string()],
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
            }),
        })
        .unwrap();

    assert!(app.sync_thinking_status());
    assert_eq!(
        app.latest_prompt_for_session("amf-my-feat"),
        Some("warm prompt")
    );
    assert_eq!(
        app.opencode_sidebar_cache
            .get("amf-my-feat")
            .and_then(|data| data.title.as_deref()),
        Some("Warm cache")
    );
    assert!(app.is_feature_thinking("amf-my-feat"));
    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
}

#[test]
fn ipc_opencode_sidebar_update_queues_sidebar_refresh() {
    let repo = TempDir::new().unwrap();
    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Idle);
    store.projects[0].features[0].agent = AgentKind::Opencode;

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "opencode-sidebar-updated",
        "source": "opencode-sidebar",
        "session_id": "ses-1",
        "cwd": repo.path(),
    }));

    assert!(app.pending_sidebar_loads.contains("amf-my-feat"));
}

#[test]
fn visible_animation_is_enabled_for_dashboard_thinking_features() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    assert!(!app.has_visible_animation());
    app.thinking_features.insert("amf-my-feat".to_string());
    assert!(app.has_visible_animation());
}

#[test]
fn visible_animation_is_enabled_for_dashboard_summary_generation() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.summary_state
        .generating
        .insert("amf-my-feat".to_string());
    assert!(app.has_visible_animation());
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
                remote_control: false,
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
                remote_control: false,
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
        "Claude 1".to_string(),
        false,
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
fn start_worktree_hook_clears_sidebar_state_for_reused_feature() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
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

    app.start_worktree_hook(
        "true",
        workdir.path().to_path_buf(),
        "my-project".to_string(),
        "my-feat".to_string(),
        VibeMode::default(),
        false,
        false,
        AgentKind::Claude,
        false,
        "Claude 1".to_string(),
        false,
        false,
        false,
        None,
    );

    assert!(app.latest_prompt_for_session("amf-my-feat").is_none());
    assert!(!app.opencode_sidebar_cache.contains_key("amf-my-feat"));
    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
    assert!(app.store.projects[0].features[0].pending_worktree_script);
    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
        .times(0)
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
        create_terminal: false,
        session_name: "Claude 1".to_string(),
        enable_chrome: false,
        remote_control: false,
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
    assert_eq!(feature.sessions.len(), 1);
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
            assert!(!state.remote_control, "default must not override z.ai guard");
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
                .withf(|session, window, resume, extra_args| {
                    session == "amf-my-project-coached"
                        && window == "codex"
                        && resume.is_none()
                        && extra_args.iter().any(|arg| arg == "--add-dir")
                })
                .returning(|_, _, _, _| Ok(()));
        }
        AgentKind::Pi => {
            tmux.expect_launch_pi().times(1).returning(|_, _| Ok(()));
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
        agent,
        create_terminal: true,
        session_name: "Claude 1".to_string(),
        enable_chrome: false,
        remote_control: false,
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
        remote_control: false,
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
    app.viewport_total_rows = 41;
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
        .times(0)
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
        create_terminal: false,
        session_name: "Claude 1".to_string(),
        enable_chrome: false,
        remote_control: false,
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
        .times(0)
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
        create_terminal: false,
        session_name: "Opencode 1".to_string(),
        enable_chrome: false,
        remote_control: false,
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

    // Remove the version stamp so refresh_opencode_plugins_for_store
    // doesn't short-circuit even if another test already marked hooks current.
    let stamp = crate::project::amf_config_dir().join("last_hook_refresh_version");
    let _ = std::fs::remove_file(&stamp);

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
            && sidebar_plugin.contains("event: async ({ event })")
            && sidebar_plugin.contains("switch (event?.type)")
            && sidebar_plugin.contains("case \"todo.updated\"")
            && sidebar_plugin.contains("case \"message.updated\""),
        "expected sidebar-state.js to be installed, got: {sidebar_plugin}"
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

fn compose_test_view() -> ViewState {
    ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "claude".to_string(),
        "Claude 1".to_string(),
        SessionKind::Claude,
        VibeMode::Vibeless,
        false,
    )
}

fn compose_command(name: &str, interactive: bool) -> ComposeCommandEntry {
    ComposeCommandEntry {
        name: name.to_string(),
        description: String::new(),
        source: ComposeCommandSource::BuiltIn,
        interactive,
    }
}

#[test]
fn submit_compose_pastes_prose_into_session() {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().to_path_buf();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_paste_text()
        .withf(|session, window, text| {
            session == "amf-my-feat" && window == "claude" && text == "Fix the flaky timer test."
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|session, window, key| {
            session == "amf-my-feat" && window == "claude" && key == "C-u"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|session, window, key| {
            session == "amf-my-feat" && window == "claude" && key == "Enter"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::Compose(ComposeState::new(
        compose_test_view(),
        workdir.clone(),
        "Fix the flaky timer test.".to_string(),
        Vec::new(),
    ));

    app.submit_compose().unwrap();

    assert!(matches!(&app.mode, AppMode::Viewing(view) if view.session == "amf-my-feat"));
    assert_eq!(
        std::fs::read_to_string(workdir.join(".claude").join("latest-prompt.txt")).unwrap(),
        "Fix the flaky timer test."
    );
}

#[test]
fn submit_compose_types_slash_command_literally() {
    let repo = TempDir::new().unwrap();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_send_literal()
        .withf(|session, window, text| {
            session == "amf-my-feat" && window == "claude" && text == "/compact"
        })
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|_, _, key| key == "C-u")
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|_, _, key| key == "Enter")
        .times(1)
        .returning(|_, _, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::Compose(ComposeState::new(
        compose_test_view(),
        repo.path().to_path_buf(),
        "/compact".to_string(),
        vec![compose_command("compact", false)],
    ));

    app.submit_compose().unwrap();

    assert!(matches!(&app.mode, AppMode::Viewing(_)));
    assert!(app.compose_direct_targets.is_empty());
}

#[test]
fn submit_compose_interactive_command_enables_direct_input() {
    let repo = TempDir::new().unwrap();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_send_literal()
        .withf(|_, _, text| text == "/model")
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|_, _, key| key == "C-u")
        .times(1)
        .returning(|_, _, _| Ok(()));
    tmux.expect_send_key_name()
        .withf(|_, _, key| key == "Enter")
        .times(1)
        .returning(|_, _, _| Ok(()));

    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active),
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::Compose(ComposeState::new(
        compose_test_view(),
        repo.path().to_path_buf(),
        "/model".to_string(),
        vec![compose_command("model", true)],
    ));

    app.submit_compose().unwrap();

    assert!(matches!(&app.mode, AppMode::Viewing(_)));
    assert!(app.compose_direct_targets.contains("amf-my-feat:claude"));
}

#[test]
fn submit_compose_rejects_empty_buffer() {
    let repo = TempDir::new().unwrap();
    let tmp = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.mode = AppMode::Compose(ComposeState::new(
        compose_test_view(),
        repo.path().to_path_buf(),
        "   ".to_string(),
        Vec::new(),
    ));

    app.submit_compose().unwrap();

    assert!(matches!(&app.mode, AppMode::Compose(_)));
}

#[test]
fn compose_suggestions_filter_by_prefix_and_complete() {
    let catalog = vec![
        compose_command("clear", false),
        compose_command("compact", false),
        compose_command("config", true),
        compose_command("model", true),
    ];
    let mut state = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        "/c".to_string(),
        catalog,
    );

    let names: Vec<&str> = state
        .suggestions
        .iter()
        .filter_map(|idx| state.catalog.get(*idx))
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, vec!["clear", "compact", "config"]);

    state.select_next_suggestion();
    assert!(state.complete_selected_suggestion());
    assert_eq!(state.editor.text(), "/compact");
    assert!(state.exact_command_match().is_some());
}

#[test]
fn compose_suggestions_match_namespaced_commands_fuzzily() {
    let catalog = vec![
        compose_command("clear", false),
        compose_command("stn:commit", false),
        compose_command("config", true),
    ];
    let mut state = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        "/commit".to_string(),
        catalog,
    );

    let names: Vec<&str> = state
        .suggestions
        .iter()
        .filter_map(|idx| state.catalog.get(*idx))
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, vec!["stn:commit"]);

    assert!(state.complete_selected_suggestion());
    assert_eq!(state.editor.text(), "/stn:commit");
    assert!(state.exact_command_match().is_some());
}

#[test]
fn compose_suggestions_rank_literal_above_namespaced() {
    let catalog = vec![
        compose_command("stn:commit", false),
        compose_command("commit", false),
    ];
    let state = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        "/commit".to_string(),
        catalog,
    );

    let names: Vec<&str> = state
        .suggestions
        .iter()
        .filter_map(|idx| state.catalog.get(*idx))
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(names, vec!["commit", "stn:commit"]);
}

#[test]
fn compose_parts_interleave_text_and_images() {
    use super::compose::{ComposePart, split_compose_parts};

    let image = |placeholder: &str| ComposeImage {
        placeholder: placeholder.to_string(),
        data: vec![1, 2, 3],
        mime: "image/png".to_string(),
    };
    let images = vec![image("[Image 1]"), image("[Image 2]")];

    let parts = split_compose_parts("look at [Image 1] and [Image 2] please", &images);
    assert_eq!(
        parts,
        vec![
            ComposePart::Text("look at ".to_string()),
            ComposePart::Image(0),
            ComposePart::Text(" and ".to_string()),
            ComposePart::Image(1),
            ComposePart::Text(" please".to_string()),
        ]
    );

    // A deleted placeholder drops its image from delivery.
    let parts = split_compose_parts("only [Image 2] remains", &images);
    assert_eq!(
        parts,
        vec![
            ComposePart::Text("only ".to_string()),
            ComposePart::Image(1),
            ComposePart::Text(" remains".to_string()),
        ]
    );

    // No placeholders: one text part.
    let parts = split_compose_parts("plain prose", &images);
    assert_eq!(parts, vec![ComposePart::Text("plain prose".to_string())]);

    // Image-only submission.
    let parts = split_compose_parts("[Image 1]", &images);
    assert_eq!(parts, vec![ComposePart::Image(0)]);
}

#[test]
fn compose_add_image_numbers_placeholders() {
    let mut state = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        String::new(),
        Vec::new(),
    );

    assert_eq!(
        state.add_image(vec![0], "image/png".to_string()),
        "[Image 1]"
    );
    assert_eq!(
        state.add_image(vec![1], "image/jpeg".to_string()),
        "[Image 2]"
    );
    assert_eq!(state.images.len(), 2);

    assert!(state.clear_prompt());
    assert!(state.images.is_empty());
}

#[test]
fn compose_slash_detection_rules() {
    let state = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        "/compact keep recent context".to_string(),
        Vec::new(),
    );
    assert!(state.is_slash_command());
    // Arguments present: the popup should no longer filter.
    assert!(state.pending_command_prefix().is_none());

    let multiline = ComposeState::new(
        compose_test_view(),
        PathBuf::from("/tmp"),
        "/not a command\njust prose".to_string(),
        Vec::new(),
    );
    assert!(!multiline.is_slash_command());
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
        .withf(|session, window, resume_id, extra_args| {
            session == "amf-automation-project-feature-1"
                && window == "claude"
                && resume_id.is_none()
                && extra_args.is_empty()
        })
        .times(1)
        .returning(|_, _, _, _| Ok(()));
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
        token_usage_source: None,
        token_usage_source_match: None,
        created_at: Utc::now(),
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
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
        workdir
            .path()
            .join(".codex")
            .join("amf-codex-notify.sh")
            .exists(),
        "repo-root codex feature should get local notify hook script"
    );
    assert!(
        !workdir.path().join(".codex").join("config.toml").exists(),
        "repo-root codex feature should not write unsupported project-local config"
    );

    let second = TempDir::new().unwrap();
    call_ensure_hooks_for(&second, VibeMode::Vibe, AgentKind::Codex, true);
    assert!(
        second
            .path()
            .join(".codex")
            .join("amf-codex-notify.sh")
            .exists(),
        "worktree codex feature should get local notify hook script"
    );
    assert!(
        !second.path().join(".codex").join("config.toml").exists(),
        "worktree codex feature should not write unsupported project-local config"
    );
}

#[test]
fn codex_hook_only_writes_helper_script() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks_for(&workdir, VibeMode::Vibe, AgentKind::Codex, true);

    assert!(
        workdir
            .path()
            .join(".codex")
            .join("amf-codex-notify.sh")
            .exists(),
        "Codex setup should still write the helper script"
    );
    assert!(
        !workdir.path().join(".codex").join("config.toml").exists(),
        "Codex setup should not write unsupported local config"
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
fn cleanup_codex_hooks_removes_helper_script() {
    let workdir = TempDir::new().unwrap();
    let codex_dir = workdir.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();

    call_ensure_hooks_for(&workdir, VibeMode::Vibe, AgentKind::Codex, true);
    cleanup_agent_injected_files(workdir.path(), &AgentKind::Codex);

    assert!(
        !codex_dir.join("amf-codex-notify.sh").exists(),
        "cleanup should remove AMF Codex hook script"
    );
    assert!(
        !codex_dir.join("config.toml").exists(),
        "cleanup should not leave behind unsupported project-local config"
    );
}

#[test]
fn cleanup_opencode_hooks_removes_sidebar_state_artifacts() {
    let workdir = TempDir::new().unwrap();
    let plugin_dir = workdir.path().join(".opencode").join("plugins");
    let theme_dir = workdir.path().join(".opencode").join("themes");
    let sidebar_dir = workdir.path().join(".amf").join("opencode-sidebar");

    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::create_dir_all(&theme_dir).unwrap();
    std::fs::create_dir_all(&sidebar_dir).unwrap();
    std::fs::write(plugin_dir.join("sidebar-state.js"), "plugin").unwrap();
    std::fs::write(plugin_dir.join("change-tracker.js"), "plugin").unwrap();
    std::fs::write(theme_dir.join("amf.json"), "{}").unwrap();
    std::fs::write(sidebar_dir.join("ses-1.json"), "{\"session_id\":\"ses-1\"}").unwrap();

    cleanup_agent_injected_files(workdir.path(), &AgentKind::Opencode);

    assert!(
        !plugin_dir.join("sidebar-state.js").exists(),
        "cleanup should remove the Opencode sidebar plugin"
    );
    assert!(
        !plugin_dir.join("change-tracker.js").exists(),
        "cleanup should remove the Opencode diff-review plugin"
    );
    assert!(
        !theme_dir.join("amf.json").exists(),
        "cleanup should remove injected Opencode themes"
    );
    assert!(
        !sidebar_dir.exists(),
        "cleanup should remove Opencode sidebar state files"
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
        remote_control: false,
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
        workdir
            .path()
            .join(".codex")
            .join("amf-codex-notify.sh")
            .exists(),
        "Codex notify script should be injected after switching"
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
        remote_control: false,
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
        remote_control: false,
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    }
}

#[test]
fn sync_session_status_reads_first_line() {
    let workdir = TempDir::new().unwrap();
    let session_id = "test-sess-123";

    let store = store_with_custom_session(workdir.path(), session_id);
    let feature_id = store.projects[0].features[0].id.clone();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("amf.db");
    let db = crate::db::AmfDb::open(&db_path).unwrap();
    db.save_store(&store).unwrap();
    db.upsert_session_status(session_id, &feature_id, "API :3000 | DB :5432", None)
        .unwrap();

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(db);
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        Some("API :3000 | DB :5432".to_string()),
    );
}

#[test]
fn sync_session_status_migrates_legacy_file_to_db() {
    let workdir = TempDir::new().unwrap();
    let session_id = "test-sess-migrate";
    let status_dir = workdir.path().join(".amf").join("session-status");
    std::fs::create_dir_all(&status_dir).unwrap();
    std::fs::write(
        status_dir.join(format!("{}.txt", session_id)),
        "API :3000 | DB :5432\nextra line\n",
    )
    .unwrap();

    let store = store_with_custom_session(workdir.path(), session_id);
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("amf.db");
    let db = crate::db::AmfDb::open(&db_path).unwrap();
    db.save_store(&store).unwrap();

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(db);
    app.sync_session_status();

    assert_eq!(
        app.store.projects[0].features[0].sessions[0].status_text,
        Some("API :3000 | DB :5432".to_string()),
    );
    assert_eq!(
        app.db
            .as_ref()
            .unwrap()
            .load_session_status(session_id)
            .unwrap(),
        Some("API :3000 | DB :5432".to_string())
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
        remote_control: false,
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
        remote_control: false,
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
fn sync_session_status_checks_sidebar_inputs_off_thread() {
    let workdir = TempDir::new().unwrap();
    let prompt_path = workdir.path().join(".claude").join("latest-prompt.txt");
    std::fs::create_dir_all(prompt_path.parent().unwrap()).unwrap();
    std::fs::write(&prompt_path, "first prompt").unwrap();

    let created_at = Utc.with_ymd_and_hms(2026, 3, 13, 13, 59, 30).unwrap();
    let session = FeatureSession {
        id: "claude-sess".to_string(),
        kind: SessionKind::Claude,
        label: "Claude".to_string(),
        tmux_window: "claude".to_string(),
        claude_session_id: Some("claude-session-1".to_string()),
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
        agent: AgentKind::Claude,
        enable_chrome: false,
        remote_control: false,
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
        preferred_agent: AgentKind::Claude,
        is_git: false,
    };
    let store = ProjectStore {
        version: 5,
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
    let signature =
        super::SidebarLoadRequest::from_feature(&app.store.projects[0].features[0]).signature();
    app.sidebar_load_signatures
        .insert("amf-my-feat".to_string(), signature);

    let mut tracker = SessionTokenTracker::default();
    app.sync_session_status_with_tracker(&mut tracker);

    assert!(
        app.pending_sidebar_loads.contains("amf-my-feat"),
        "sidebar input checks should be queued off the UI thread"
    );
    for _ in 0..20 {
        app.poll_sidebar_load_results();
        if !app.pending_sidebar_loads.contains("amf-my-feat") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(
        !app.pending_sidebar_loads.contains("amf-my-feat"),
        "unchanged sidebar input check should complete"
    );
    assert!(
        app.latest_prompt_for_session("amf-my-feat") == Some("first prompt"),
        "unchanged sidebar input check should leave the existing cache alone"
    );

    std::thread::sleep(std::time::Duration::from_millis(2));
    std::fs::write(&prompt_path, "updated prompt").unwrap();

    app.sync_session_status_with_tracker(&mut tracker);

    assert!(
        app.pending_sidebar_loads.contains("amf-my-feat"),
        "changed prompt inputs should queue a fresh sidebar load"
    );
    for _ in 0..20 {
        app.poll_sidebar_load_results();
        if !app.pending_sidebar_loads.contains("amf-my-feat") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        app.latest_prompt_for_session("amf-my-feat"),
        Some("updated prompt")
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
            model_text: Some("Model: gpt-5.5".into()),
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
    assert_eq!(
        app.cached_codex_session_model(workdir.path(), "sess-current"),
        Some("Model: gpt-5.5")
    );
    assert!(!app.codex_sidebar_metadata_inflight.contains(&cache_key));
}

#[test]
fn sync_session_status_skips_sidebar_refresh_when_sidebar_hidden() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let mut view = ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "codex".to_string(),
        "Codex".to_string(),
        SessionKind::Codex,
        VibeMode::default(),
        false,
    );
    view.sidebar_visible = false;
    app.mode = AppMode::Viewing(view);

    app.sync_session_status();

    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
}

#[test]
fn toggling_sidebar_back_on_triggers_sidebar_refresh() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let mut view = ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "codex".to_string(),
        "Codex".to_string(),
        SessionKind::Codex,
        VibeMode::default(),
        false,
    );
    view.sidebar_visible = false;
    app.mode = AppMode::Viewing(view);

    app.toggle_sidebar_in_view();

    assert!(app.pending_sidebar_loads.contains("amf-my-feat"));
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
        "message": "Need approval before applying the patch.",
        "tool_name": "Bash",
        "relative_path": "src/main.rs"
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text())
            .as_deref(),
        Some(
            "State: waiting for input\nRequest: Need approval before applying the patch.\nTool: Bash\nFile: src/main.rs"
        )
    );
    assert_eq!(app.toasts.len(), 1);
    assert!(
        app.toasts[0]
            .message
            .contains("New input request from my-feat")
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
fn ipc_prompt_submit_clears_codex_live_review_work() {
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
            "type": "fileChange",
            "payload": {
                "relative_path": "src/main.rs",
                "status": "proposed"
            }
        }),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "prompt-submit",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "prompt": "Reviewed the diff and continue."
    }));

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
fn ipc_diff_review_updates_codex_live_review_state() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "diff-review",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "file_path": "src/main.rs",
        "message": "Review the change before continuing.",
        "tool_name": "Edit"
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text())
            .as_deref(),
        Some(
            "State: waiting for diff review\nFile: src/main.rs\nTool: Edit\nRequest: Review the change before continuing.\nHint: use leader V if the review prompt is not appearing."
        )
    );
}

#[test]
fn ipc_diff_review_opens_from_amf_session_when_cwd_does_not_match_feature() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "custom-session");
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
        "custom".to_string(),
        "Claude".to_string(),
        SessionKind::Claude,
        VibeMode::Vibeless,
        false,
    ));

    app.handle_ipc_message_value(serde_json::json!({
        "type": "diff-review",
        "session_id": "claude-hook-session",
        "amf_session": "amf-my-feat",
        "cwd": "/tmp/not-the-feature-workdir",
        "message": "Review: src/main.rs",
        "file_path": workdir.path().join("src/main.rs").display().to_string(),
        "relative_path": "src/main.rs",
        "tool": "edit",
        "change_id": "chg-ipc",
        "old_snippet": "old",
        "new_snippet": "new",
        "response_file": workdir.path().join("response.json").display().to_string(),
        "proceed_signal": workdir.path().join("proceed").display().to_string(),
        "request_id": "req-1",
        "reply_socket": "/tmp/amf-ipc-reply/req-1.sock"
    }));

    match &app.mode {
        AppMode::DiffReviewPrompt(state) => {
            assert_eq!(state.relative_path, "src/main.rs");
            assert_eq!(state.request_id.as_deref(), Some("req-1"));
            assert_eq!(
                state
                    .return_to_view
                    .as_ref()
                    .map(|view| view.feature_name.as_str()),
                Some("my-feat")
            );
        }
        _ => panic!("expected diff review prompt"),
    }
}

#[test]
fn ipc_diff_review_queues_with_toast_from_normal_mode() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "custom-session");
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Normal;

    app.handle_ipc_message_value(serde_json::json!({
        "type": "diff-review",
        "session_id": "claude-hook-session",
        "amf_session": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Review: src/main.rs",
        "relative_path": "src/main.rs",
        "tool": "edit",
        "change_id": "chg-ipc",
        "request_id": "req-1"
    }));

    // From the dashboard the review must not steal focus; it is queued
    // as a pending input and announced with a toast.
    assert!(
        matches!(app.mode, AppMode::Normal),
        "expected to stay on the dashboard"
    );
    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.pending_inputs[0].notification_type, "diff-review");
    assert!(
        app.toasts
            .last()
            .map(|toast| toast.message.contains("New diff review from my-feat"))
            .unwrap_or(false),
        "expected a diff review toast, got: {:?}",
        app.toasts.last().map(|toast| toast.message.clone())
    );
}

#[test]
fn ipc_diff_review_queues_when_viewing_a_different_feature() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "custom-session");
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Viewing(ViewState::new(
        "other-project".to_string(),
        "other-feat".to_string(),
        "amf-other-feat".to_string(),
        "custom".to_string(),
        "Claude".to_string(),
        SessionKind::Claude,
        VibeMode::Vibeless,
        false,
    ));

    app.handle_ipc_message_value(serde_json::json!({
        "type": "diff-review",
        "session_id": "claude-hook-session",
        "amf_session": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Review: src/main.rs",
        "relative_path": "src/main.rs",
        "tool": "edit",
        "change_id": "chg-ipc",
        "request_id": "req-1"
    }));

    // Viewing a different feature: stay in that view, queue the review,
    // and announce it with a toast.
    match &app.mode {
        AppMode::Viewing(view) => assert_eq!(view.feature_name, "other-feat"),
        _ => panic!("expected to stay in the other feature's view"),
    }
    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.pending_inputs[0].notification_type, "diff-review");
    assert!(
        app.toasts
            .last()
            .map(|toast| toast.message.contains("New diff review from my-feat"))
            .unwrap_or(false),
        "expected a diff review toast, got: {:?}",
        app.toasts.last().map(|toast| toast.message.clone())
    );
}

#[test]
fn ipc_tool_activity_temporarily_overrides_older_review_work() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "diff-review",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "file_path": "src/main.rs",
        "message": "Review the change before continuing."
    }));
    app.handle_ipc_message_value(serde_json::json!({
        "type": "tool-start",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "tool_name": "Bash"
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text())
            .as_deref(),
        Some("State: running tool\nTool: Bash")
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "tool-stop",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "tool_name": "Bash"
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text())
            .as_deref(),
        Some(
            "State: waiting for diff review\nFile: src/main.rs\nRequest: Review the change before continuing.\nHint: use leader V if the review prompt is not appearing."
        )
    );
}

#[test]
fn ipc_change_reason_adds_default_request_when_message_is_missing() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "change-reason",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "file_path": "src/main.rs",
        "tool_name": "Edit"
    }));

    assert_eq!(
        app.codex_live_thread("amf-my-feat")
            .and_then(|live| live.sidebar_work_text())
            .as_deref(),
        Some(
            "State: waiting for change reason\nFile: src/main.rs\nTool: Edit\nRequest: Explain why this change is needed."
        )
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
            assert_eq!(state.commands[1].name, "demo-work-change-reason");
            assert_eq!(state.commands[2].name, "demo-work-diff-review");
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
    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.pending_inputs[0].notification_type, "diff-review");
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
fn custom_diff_review_notification_queues_with_toast_from_normal_mode() {
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

    // From the dashboard the review is queued as a pending input and
    // announced with a toast instead of stealing focus.
    assert!(
        matches!(app.mode, AppMode::Normal),
        "expected to stay on the dashboard"
    );
    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.pending_inputs[0].notification_type, "diff-review");
    assert!(
        app.toasts
            .last()
            .map(|toast| toast.message.contains("New diff review"))
            .unwrap_or(false),
        "expected a diff review toast, got: {:?}",
        app.toasts.last().map(|toast| toast.message.clone())
    );
}

#[test]
fn check_pending_diff_review_opens_pending_review_from_normal_mode() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_custom_session(workdir.path(), "amf-my-feat");
    // The V flow enters the feature view, which checks the session.
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.config.diff_review_viewer = DiffReviewViewer::Amf;
    app.mode = AppMode::Normal;
    app.selection = Selection::Feature(0, 0);

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
        "change_id": "chg-3",
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

    // The scan inside check_pending_diff_review queues the review; the
    // V flow then enters the feature view, which opens the prompt.
    app.check_pending_diff_review().unwrap();

    match &app.mode {
        AppMode::DiffReviewPrompt(state) => {
            assert_eq!(state.relative_path, "src/lib.rs");
        }
        _ => panic!("expected diff review prompt"),
    }
    assert!(app.pending_inputs.is_empty());
    assert!(
        app.toasts
            .last()
            .map(|toast| toast.message.contains("Opened pending diff review"))
            .unwrap_or(false),
        "expected an opened-review toast, got: {:?}",
        app.toasts.last().map(|toast| toast.message.clone())
    );
}

#[test]
fn scan_notifications_pushes_input_request_toast() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_codex_session(workdir.path(), false);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let tmp = NamedTempFile::new().unwrap();
    app.store_path = tmp.path().to_path_buf();

    let notify_dir = workdir.path().join(".claude").join("notifications");
    std::fs::create_dir_all(&notify_dir).unwrap();
    let notification = serde_json::json!({
        "session_id": "amf-my-feat",
        "cwd": workdir.path().display().to_string(),
        "message": "Need input before continuing.",
        "type": "input-request",
        "tool": "write",
        "response_file": workdir.path().join("response.json").display().to_string(),
    });
    std::fs::write(
        notify_dir.join("input-request.json"),
        serde_json::to_string(&notification).unwrap(),
    )
    .unwrap();

    app.scan_notifications();

    assert_eq!(app.pending_inputs.len(), 1);
    assert_eq!(app.toasts.len(), 1);
    assert!(
        app.toasts[0]
            .message
            .contains("New input request from my-feat")
    );
}

#[test]
fn contextual_syntax_install_returns_to_diff_viewer_and_refreshes() {
    let workdir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
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
    // Returning to the diff viewer triggers a refresh (DiffViewerLoading);
    // drive it to completion as the event loop does before asserting.
    app.complete_diff_viewer_loading();

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
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
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
            opened_at: std::time::Instant::now(),
            hold_secs: 0.0,
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
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
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
        remote_control: false,
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
    let feature_id = store.projects[0].features[0].id.clone();
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("amf.db");
    let db = crate::db::AmfDb::open(&db_path).unwrap();
    db.save_store(&store).unwrap();
    db.upsert_session_status(session_id, &feature_id, "running", None)
        .unwrap();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_sessions().returning(|| Ok(vec![]));

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.db = Some(db);
    app.pending_sidebar_loads
        .insert("amf-custom-cleanup-test-sess".to_string());
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
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .load_session_status(session_id)
            .unwrap()
            .is_none(),
        "status row should be removed on session removal"
    );
    assert!(app.latest_prompt_for_session("amf-my-feat").is_none());
    assert!(!app.opencode_sidebar_cache.contains_key("amf-my-feat"));
    assert!(!app.pending_sidebar_loads.contains("amf-my-feat"));
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
        remote_control: false,
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
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
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
        .withf(|_, _, _, extra_args| extra_args.iter().any(|arg| arg == "--add-dir"))
        .returning(|_, _, _, _| Ok(()));
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
        .withf(|_, _, _, extra_args| extra_args.iter().any(|arg| arg == "--add-dir"))
        .returning(|_, _, _, _| Ok(()));
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

// ── Prompt library picker ─────────────────────────────────────

fn store_with_prompt_templates(names: &[&str]) -> ProjectStore {
    let templates = names
        .iter()
        .map(|name| {
            crate::prompt_library::PromptTemplate::new(
                name.to_string(),
                format!("body of {name}"),
            )
        })
        .collect();
    ProjectStore {
        version: 5,
        projects: vec![],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: templates,
        extra: HashMap::new(),
    }
}

#[test]
fn prompt_library_opens_with_all_templates_visible() {
    let store = store_with_prompt_templates(&["alpha", "beta", "gamma"]);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.open_prompt_library(None);

    match &app.mode {
        AppMode::PromptLibrary(state) => {
            assert_eq!(state.templates.len(), 3);
            assert_eq!(state.filtered.len(), 3);
            assert_eq!(state.selected, 0);
        }
        _ => panic!("expected PromptLibrary mode"),
    }
}

#[test]
fn prompt_library_nav_wraps_around() {
    let store = store_with_prompt_templates(&["a", "b", "c"]);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.open_prompt_library(None);

    app.prompt_library_select_prev(); // wraps from 0 to last
    match &app.mode {
        AppMode::PromptLibrary(state) => assert_eq!(state.selected, 2),
        _ => panic!("expected PromptLibrary mode"),
    }
    app.prompt_library_select_next(); // wraps back to 0
    match &app.mode {
        AppMode::PromptLibrary(state) => assert_eq!(state.selected, 0),
        _ => panic!("expected PromptLibrary mode"),
    }
}

#[test]
fn prompt_library_filter_narrows_and_clamps_selection() {
    let store = store_with_prompt_templates(&["alpha", "beta", "gamma"]);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.open_prompt_library(None);

    // Move selection to the last row, then filter to a single match so
    // the selection must clamp back into range.
    app.prompt_library_select_prev();
    if let AppMode::PromptLibrary(state) = &mut app.mode {
        assert_eq!(state.selected, 2);
        state.query = "alpha".to_string();
    }
    app.prompt_library_filter();

    match &app.mode {
        AppMode::PromptLibrary(state) => {
            assert_eq!(state.filtered.len(), 1);
            assert!(state.selected < state.filtered.len());
            let entry = state.selected_entry().unwrap();
            assert_eq!(entry.template.name, "alpha");
        }
        _ => panic!("expected PromptLibrary mode"),
    }
}

#[test]
fn prompt_library_inject_with_no_session_copies_and_exits() {
    let store = store_with_prompt_templates(&["only"]);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.open_prompt_library(None);

    // No from_view, so injection falls back to a clipboard copy (which may
    // fail in headless CI) and returns to Normal either way.
    let _ = app.inject_selected_template();
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn prompt_library_surfaces_global_config_templates_with_badge() {
    let store = store_with_prompt_templates(&["mine"]);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.config.extension.prompt_templates =
        vec![crate::prompt_library::PromptTemplate::new(
            "shared".to_string(),
            "shared body".to_string(),
        )];

    app.open_prompt_library(None);

    match &app.mode {
        AppMode::PromptLibrary(state) => {
            assert_eq!(state.templates.len(), 2);
            let user = state
                .templates
                .iter()
                .find(|e| e.template.name == "mine")
                .unwrap();
            assert_eq!(user.source, crate::prompt_library::PromptSource::User);
            let global = state
                .templates
                .iter()
                .find(|e| e.template.name == "shared")
                .unwrap();
            assert_eq!(global.source, crate::prompt_library::PromptSource::Global);
            // Config entries can be edited but not deleted in place.
            assert!(global.source.is_editable());
            assert!(!global.source.is_deletable());
        }
        _ => panic!("expected PromptLibrary mode"),
    }
}

#[test]
fn prompt_library_export_to_global_updates_extension_config() {
    let store = store_with_prompt_templates(&["only"]);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.open_prompt_library(None);

    app.export_selected_template(crate::app::PromptExportTarget::Global)
        .unwrap();

    // The in-memory global extension config gains the template (the file
    // write is a no-op in tests, which use an empty store_path).
    assert_eq!(app.config.extension.prompt_templates.len(), 1);
    assert_eq!(app.config.extension.prompt_templates[0].name, "only");

    // Exporting the same name again replaces rather than duplicates.
    app.open_prompt_library(None);
    app.export_selected_template(crate::app::PromptExportTarget::Global)
        .unwrap();
    assert_eq!(app.config.extension.prompt_templates.len(), 1);
}

#[test]
fn bare_uppercase_a_opens_harness_setup_from_dashboard() {
    // Regression: the `A` dispatch used to live in the leader-chord
    // handler, so a bare `A` on the dashboard did nothing. It must be
    // handled in the normal key handler like the other capital actions.
    let store = ProjectStore {
        version: 5,
        projects: vec![],
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
    app.mode = AppMode::Normal;
    let key = crossterm::event::KeyEvent::new(
        KeyCode::Char('A'),
        crossterm::event::KeyModifiers::NONE,
    );
    crate::handlers::handle_normal_key(&mut app, key).unwrap();
    assert!(matches!(app.mode, AppMode::HarnessSetup(_)));
}
