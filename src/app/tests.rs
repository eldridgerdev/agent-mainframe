use super::setup::{
    cleanup_agent_injected_files, ensure_notification_hooks, ensure_plan_mode_instructions,
    ensure_review_claude_md, strip_between_markers,
};
use super::steering::PromptConstraint;
use super::sync::pane_shows_thinking_hint;
use super::util::{latest_prompt_path, read_latest_prompt, shorten_path, slugify};
use super::*;
use crate::automation::{CreateBatchFeaturesRequest, CreateFeatureRequest, CreateProjectRequest};
use crate::extension::{ExtensionConfig, FeaturePreset, HookConfig, HookPrompt, LifecycleHooks};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
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
fn startup_mask_expires_without_snapshot_updates() {
    let mut view = ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "codex-2".to_string(),
        "Codex 2".to_string(),
        SessionKind::Codex,
        VibeMode::default(),
        false,
    );
    view.show_startup_mask();
    assert!(view.startup_mask_active());

    view.startup_mask_started_at = Some(Instant::now() - STARTUP_MASK_MAX_DURATION);
    assert!(!view.startup_mask_active());
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
fn app_config_default_view_auto_refresh_is_disabled() {
    let config = AppConfig::default();
    assert!(!config.view_auto_refresh);
}

#[test]
fn app_config_default_agent_restart_limit_is_one() {
    let config = AppConfig::default();
    assert_eq!(config.max_agent_autostart_sessions, 1);
}

#[test]
fn app_config_resource_guard_defaults() {
    let config = AppConfig::default();
    assert_eq!(config.max_concurrent_agents, 4);
    assert_eq!(config.low_memory_warn_mb, 1536);
    assert!(config.kill_editor_on_stop);
    assert_eq!(config.dormant_idle_minutes, 60);
    assert_eq!(config.dormant_last_accessed_hours, 4);
}

#[test]
fn app_config_missing_resource_guard_keys_use_defaults() {
    // Configs written before these keys existed must keep loading.
    let config: AppConfig = serde_json::from_str(r#"{"nerd_font":false}"#).unwrap();
    assert_eq!(config.agent_concurrency_limit(), Some(4));
    assert_eq!(config.low_memory_threshold_mb(), Some(1536));
    assert!(config.kill_editor_on_stop);
    assert_eq!(
        config.dormant_thresholds(),
        Some((
            std::time::Duration::from_secs(60 * 60),
            std::time::Duration::from_secs(4 * 3600)
        ))
    );
}

#[test]
fn app_config_resource_guards_can_be_configured() {
    let config: AppConfig = serde_json::from_str(
        r#"{"max_concurrent_agents":8,"low_memory_warn_mb":2048,"kill_editor_on_stop":false,
            "dormant_idle_minutes":15,"dormant_last_accessed_hours":24}"#,
    )
    .unwrap();
    assert_eq!(config.agent_concurrency_limit(), Some(8));
    assert_eq!(config.low_memory_threshold_mb(), Some(2048));
    assert!(!config.kill_editor_on_stop);
    assert_eq!(
        config.dormant_thresholds(),
        Some((
            std::time::Duration::from_secs(15 * 60),
            std::time::Duration::from_secs(24 * 3600)
        ))
    );
}

#[test]
fn app_config_zero_disables_each_resource_guard() {
    let config: AppConfig =
        serde_json::from_str(r#"{"max_concurrent_agents":0,"low_memory_warn_mb":0}"#).unwrap();
    assert_eq!(config.agent_concurrency_limit(), None);
    assert_eq!(config.low_memory_threshold_mb(), None);

    // Dormancy is an AND of both halves, so zeroing either one turns the
    // whole check off instead of marking every feature dormant.
    let idle_off: AppConfig = serde_json::from_str(r#"{"dormant_idle_minutes":0}"#).unwrap();
    assert_eq!(idle_off.dormant_thresholds(), None);
    let accessed_off: AppConfig =
        serde_json::from_str(r#"{"dormant_last_accessed_hours":0}"#).unwrap();
    assert_eq!(accessed_off.dormant_thresholds(), None);
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
fn review_model_for_falls_back_to_shared_default_when_unset() {
    let config = AppConfig {
        review_model: Some("opus".to_string()),
        ..AppConfig::default()
    };
    assert_eq!(
        config.review_model_for(ReviewAction::Walkthrough),
        Some("opus".to_string())
    );
    assert_eq!(
        config.review_model_for(ReviewAction::ChangesetOverview),
        Some("opus".to_string())
    );
}

#[test]
fn review_model_for_prefers_per_action_override() {
    let mut config = AppConfig {
        review_model: Some("opus".to_string()),
        ..AppConfig::default()
    };
    config.review_models.insert(
        ReviewAction::Walkthrough.config_key().to_string(),
        "haiku".to_string(),
    );
    assert_eq!(
        config.review_model_for(ReviewAction::Walkthrough),
        Some("haiku".to_string())
    );
    // Unaffected actions still see the shared default.
    assert_eq!(
        config.review_model_for(ReviewAction::CoReview),
        Some("opus".to_string())
    );
}

#[test]
fn review_model_for_is_none_when_nothing_configured() {
    let config = AppConfig::default();
    assert_eq!(config.review_model_for(ReviewAction::PrReview), None);
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
fn app_config_missing_view_auto_refresh_uses_default() {
    let config: AppConfig = serde_json::from_str(r#"{"nerd_font":false}"#).unwrap();
    assert!(!config.view_auto_refresh);
}

#[test]
fn app_config_view_auto_refresh_can_be_enabled() {
    let config: AppConfig = serde_json::from_str(r#"{"view_auto_refresh":true}"#).unwrap();
    assert!(config.view_auto_refresh);
}

#[test]
fn app_config_agent_restart_limit_can_be_configured() {
    let config: AppConfig = serde_json::from_str(r#"{"max_agent_autostart_sessions":0}"#).unwrap();
    assert_eq!(config.max_agent_autostart_sessions, 0);
}

#[test]
fn migrate_app_config_flips_pre_v1_defaults() {
    // A config written before versioning: control mode on, old popup hold.
    let mut config: AppConfig =
        serde_json::from_str(r#"{"tmux_control_mode":true,"diff_review_popup_hold_secs":3.0}"#)
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
    let mut config: AppConfig =
        serde_json::from_str(r#"{"tmux_control_mode":false,"diff_review_popup_hold_secs":5.0}"#)
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

#[test]
fn strip_between_markers_handles_unicode_before_marker() {
    let result = strip_between_markers(
        "café\n\n<!-- BEGIN -->\nmanaged\n<!-- END -->\n",
        "<!-- BEGIN -->",
        "<!-- END -->",
    );
    assert_eq!(result, "café\n");
}

#[test]
fn plan_mode_instructions_use_claude_local_and_per_workdir_plan() {
    let workdir = tempfile::TempDir::new().unwrap();
    std::fs::write(workdir.path().join("CLAUDE.local.md"), "# Existing\n").unwrap();

    ensure_plan_mode_instructions(workdir.path(), &AgentKind::Claude, true);
    ensure_plan_mode_instructions(workdir.path(), &AgentKind::Claude, true);

    let instructions = std::fs::read_to_string(workdir.path().join("CLAUDE.local.md")).unwrap();
    assert!(instructions.starts_with("# Existing\n"));
    assert!(instructions.contains("`.claude/plan.md`"));
    assert_eq!(
        instructions
            .matches("<!-- AMF:plan-instructions:begin -->")
            .count(),
        1,
        "instruction injection should be idempotent"
    );
    assert!(
        std::fs::read_to_string(workdir.path().join(".gitignore"))
            .unwrap()
            .lines()
            .any(|line| line == "CLAUDE.local.md")
    );
    assert!(!workdir.path().join("PLAN.md").exists());

    ensure_plan_mode_instructions(workdir.path(), &AgentKind::Claude, false);
    assert_eq!(
        std::fs::read_to_string(workdir.path().join("CLAUDE.local.md")).unwrap(),
        "# Existing\n"
    );
}

#[test]
fn review_mode_instructions_refresh_managed_block_and_ignore_archives() {
    let workdir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        workdir.path().join("CLAUDE.local.md"),
        "# Existing\n\n\
         <!-- AMF:review-instructions:begin -->\n\n\
         stale managed text\n\n\
         <!-- AMF:review-instructions:end -->\n",
    )
    .unwrap();

    ensure_review_claude_md(workdir.path(), true);
    ensure_review_claude_md(workdir.path(), true);

    let instructions = std::fs::read_to_string(workdir.path().join("CLAUDE.local.md")).unwrap();
    assert!(instructions.starts_with("# Existing\n"));
    assert!(!instructions.contains("stale managed text"));
    assert!(instructions.contains("review-notes-archive.md"));
    assert_eq!(
        instructions
            .matches("<!-- AMF:review-instructions:begin -->")
            .count(),
        1,
        "managed instruction replacement should be idempotent"
    );

    let ignore =
        std::fs::read_to_string(workdir.path().join(".claude").join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|line| line == "review-notes.md"));
    assert!(ignore.lines().any(|line| line == "review-notes-archive.md"));
}

#[test]
fn plan_mode_instructions_use_agents_for_non_claude_harnesses() {
    for agent in [AgentKind::Codex, AgentKind::Opencode, AgentKind::Pi] {
        let workdir = tempfile::TempDir::new().unwrap();

        ensure_plan_mode_instructions(workdir.path(), &agent, true);

        let instructions = std::fs::read_to_string(workdir.path().join("AGENTS.md")).unwrap();
        assert!(instructions.contains("`.claude/plan.md`"));
        assert!(!workdir.path().join("CLAUDE.local.md").exists());
        let ignore = std::fs::read_to_string(workdir.path().join(".gitignore")).unwrap();
        assert!(ignore.lines().any(|line| line == "AGENTS.md"));

        ensure_plan_mode_instructions(workdir.path(), &agent, false);
        assert!(!workdir.path().join("AGENTS.md").exists());
        assert!(
            !workdir.path().join(".gitignore").exists(),
            "AMF-owned AGENTS.md ignore entry should be cleaned up"
        );
    }
}

#[test]
fn plan_mode_instructions_preserve_user_agents_file_and_ignore_rules() {
    let workdir = tempfile::TempDir::new().unwrap();
    std::fs::write(workdir.path().join("AGENTS.md"), "# User instructions\n").unwrap();
    std::fs::write(workdir.path().join(".gitignore"), "target/\n").unwrap();

    ensure_plan_mode_instructions(workdir.path(), &AgentKind::Codex, true);

    let ignore = std::fs::read_to_string(workdir.path().join(".gitignore")).unwrap();
    assert_eq!(ignore, "target/\n", "a user's AGENTS.md must stay tracked");

    ensure_plan_mode_instructions(workdir.path(), &AgentKind::Codex, false);
    assert_eq!(
        std::fs::read_to_string(workdir.path().join("AGENTS.md")).unwrap(),
        "# User instructions\n"
    );
    assert_eq!(
        std::fs::read_to_string(workdir.path().join(".gitignore")).unwrap(),
        "target/\n"
    );
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
use crate::token_tracking::{
    SessionTokenTracker, SessionTokenUsage, TokenUsageProvider, TokenUsageSource,
};
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
        triage_source: None,
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
        triage_source: None,
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
        token_usage: None,
    }
}

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
fn active_pr_updates_cache_current_branch_and_remove_confirmed_absence() {
    use super::sync::{ActivePrLookup, ActivePrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();

    assert!(app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch: branch.clone(),
        lookup: ActivePrLookup::Found(ActivePrStatus {
            branch: branch.clone(),
            head_sha: "abc123".to_string(),
            number: 321,
            unresolved_threads: Some(4),
        }),
    }]));
    assert_eq!(app.active_pr_for_feature(&feature_id).unwrap().number, 321);
    assert_eq!(
        app.active_pr_for_feature(&feature_id)
            .unwrap()
            .unresolved_threads,
        Some(4)
    );

    // A result started for an old branch cannot overwrite the current badge.
    assert!(!app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch: "old-branch".to_string(),
        lookup: ActivePrLookup::NoPr,
    }]));
    assert!(app.active_pr_for_feature(&feature_id).is_some());

    assert!(app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch,
        lookup: ActivePrLookup::NoPr,
    }]));
    assert!(app.active_pr_for_feature(&feature_id).is_none());
}

#[test]
fn failed_active_pr_refresh_preserves_last_known_badge() {
    use super::sync::{ActivePrLookup, ActivePrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    app.active_prs.insert(
        feature_id.clone(),
        ActivePrStatus {
            branch: branch.clone(),
            head_sha: "abc123".to_string(),
            number: 321,
            unresolved_threads: Some(1),
        },
    );

    assert!(!app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch,
        lookup: ActivePrLookup::Failed("network unavailable".to_string()),
    }]));
    assert_eq!(app.active_pr_for_feature(&feature_id).unwrap().number, 321);
}

#[test]
fn active_pr_successor_replaces_badge_and_invalidates_old_pr_targets() {
    use super::sync::{ActivePrLookup, ActivePrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let old_state = match &mut app.mode {
        AppMode::PrReview(state) => {
            state.review.pr.number = 449;
            state.clone()
        }
        _ => unreachable!(),
    };
    app.pr_review_return = Some(PrReviewReturn {
        session: "amf-my-feat".to_string(),
        window: "pr-triage".to_string(),
        state: old_state.clone(),
    });
    let mut old_ai_pr = old_state.review.pr.clone();
    old_ai_pr.number = 449;
    app.ai_review_pending = Some(sample_ai_review_state(old_state.workdir.clone(), old_ai_pr));

    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    app.active_prs.insert(
        feature_id.clone(),
        ActivePrStatus {
            branch: branch.clone(),
            head_sha: "old-head".to_string(),
            number: 449,
            unresolved_threads: Some(0),
        },
    );
    assert!(app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id: feature_id.clone(),
        branch: branch.clone(),
        lookup: ActivePrLookup::Found(ActivePrStatus {
            branch,
            head_sha: "new-head".to_string(),
            number: 450,
            unresolved_threads: Some(2),
        }),
    }]));

    assert_eq!(app.active_pr_for_feature(&feature_id).unwrap().number, 450);
    assert!(app.pr_review_return.is_none());
    assert!(app.ai_review_pending.is_none());
    // Explicitly opened closed PRs remain viewable; only implicit restore
    // targets are invalidated by discovering the current open successor.
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state) if state.review.pr.number == 449
    ));
}

#[test]
fn active_pr_successor_prevents_returning_to_stashed_predecessor() {
    use super::sync::{ActivePrLookup, ActivePrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let old_state = match &mut app.mode {
        AppMode::PrReview(state) => {
            state.review.pr.number = 449;
            state.clone()
        }
        _ => unreachable!(),
    };
    app.pr_review_return = Some(PrReviewReturn {
        session: "amf-my-feat".to_string(),
        window: "pr-triage".to_string(),
        state: old_state,
    });
    app.mode = AppMode::Viewing(ViewState::new(
        "my-project".to_string(),
        "my-feat".to_string(),
        "amf-my-feat".to_string(),
        "pr-triage".to_string(),
        "PR Triage".to_string(),
        SessionKind::Claude,
        VibeMode::default(),
        false,
    ));

    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    app.active_prs.insert(
        feature_id.clone(),
        ActivePrStatus {
            branch: branch.clone(),
            head_sha: "old-head".to_string(),
            number: 449,
            unresolved_threads: Some(0),
        },
    );
    app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id,
        branch: branch.clone(),
        lookup: ActivePrLookup::Found(ActivePrStatus {
            branch,
            head_sha: "new-head".to_string(),
            number: 450,
            unresolved_threads: Some(0),
        }),
    }]);
    app.pr_review_return_to_pane();

    assert!(matches!(&app.mode, AppMode::Viewing(_)));
    assert!(app.pr_review_return.is_none());
}

#[test]
fn unchanged_active_pr_badge_does_not_cancel_explicit_closed_pr_fetch() {
    use super::sync::{ActivePrLookup, ActivePrUpdate};

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let feature = &app.store.projects[0].features[0];
    let feature_id = feature.id.clone();
    let branch = feature.branch.clone();
    app.active_prs.insert(
        feature_id.clone(),
        ActivePrStatus {
            branch: branch.clone(),
            head_sha: "open-head".to_string(),
            number: 450,
            unresolved_threads: Some(0),
        },
    );
    let (_tx, rx) = std::sync::mpsc::channel();
    app.pr_review_bg = Some(rx);
    app.mode = AppMode::PrReviewLoading(crate::app::PrReviewLoadState {
        workdir: feature.workdir.clone(),
        pr: crate::github::PrRef {
            number: 449,
            head_sha: "closed-head".to_string(),
            url: "https://github.com/o/r/pull/449".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            head_ref: "main".to_string(),
        },
        usage_baselines: std::collections::HashMap::new(),
    });

    assert!(!app.apply_active_pr_updates(vec![ActivePrUpdate {
        feature_id,
        branch: branch.clone(),
        lookup: ActivePrLookup::Found(ActivePrStatus {
            branch,
            head_sha: "open-head".to_string(),
            number: 450,
            unresolved_threads: Some(0),
        }),
    }]));

    assert!(matches!(
        &app.mode,
        AppMode::PrReviewLoading(state) if state.pr.number == 449
    ));
    assert!(app.pr_review_bg.is_some());
}

#[test]
fn predecessor_invalidation_preserves_live_successor_ai_review() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let successor = match &mut app.mode {
        AppMode::PrReview(state) => {
            state.review.pr.number = 450;
            state.clone()
        }
        _ => unreachable!(),
    };
    app.pr_review_return = Some(PrReviewReturn {
        session: "amf-my-feat".to_string(),
        window: "pr-triage".to_string(),
        state: successor.clone(),
    });
    let ai_origin = sample_ai_review_state(successor.workdir.clone(), successor.review.pr.clone());
    let (_tx, rx) = std::sync::mpsc::channel();
    app.ai_review_bg = Some(rx);
    app.ai_review_pending = Some(ai_origin.clone());
    app.ai_review_progress = Some(crate::app::AiReviewRunProgress {
        stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
        started_at: std::time::Instant::now(),
        activity: None,
        usage: None,
    });
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin: ai_origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    assert!(
        !app.invalidate_pr_context_for_transition(std::path::Path::new("/tmp/test-workdir"), 449,)
    );

    assert!(app.pr_review_return.is_some());
    assert!(app.ai_review_pending.is_some());
    assert!(app.ai_review_bg.is_some());
    assert!(app.ai_review_progress.is_some());
    assert!(matches!(
        &app.mode,
        AppMode::AiReviewRunning(state) if state.origin.pr.number == 450
    ));
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
fn sync_thinking_status_raises_review_ready_after_watched_session_finishes() {
    let repo = TempDir::new().unwrap();
    let mut store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Idle);
    store.projects[0].features[0].agent = AgentKind::Opencode;

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.awaiting_review_fixes.insert(
        "amf-my-feat".to_string(),
        AwaitingReviewFix {
            started_thinking: false,
        },
    );

    let sidebar_data = |status: &str| crate::app::opencode_storage::OpencodeSidebarData {
        session_id: "ses-1".to_string(),
        title: None,
        latest_prompt: None,
        status: Some(status.to_string()),
        last_tool: None,
        todo_count: None,
        todo_preview: vec![],
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
    };

    // Still working: `started_thinking` flips true, no notification yet.
    app.opencode_sidebar_cache
        .insert("amf-my-feat".to_string(), sidebar_data("busy"));
    app.sync_thinking_status();
    assert!(
        app.awaiting_review_fixes
            .get("amf-my-feat")
            .is_some_and(|w| w.started_thinking),
        "watched session should be marked as having started thinking"
    );
    assert!(
        app.pending_inputs.is_empty(),
        "no notification should fire while still thinking"
    );

    // Goes idle: the watched session raises a distinct "review-ready"
    // notification instead of (not in addition to) the generic one, and the
    // watch entry is cleared.
    app.opencode_sidebar_cache
        .insert("amf-my-feat".to_string(), sidebar_data("idle"));
    app.sync_thinking_status();

    assert!(!app.awaiting_review_fixes.contains_key("amf-my-feat"));
    assert_eq!(app.pending_inputs.len(), 1);
    let notif = &app.pending_inputs[0];
    assert_eq!(notif.notification_type, "review-ready");
    assert_eq!(notif.message, "Fixes ready — re-review?");
    assert_eq!(notif.feature_name.as_deref(), Some("my-feat"));
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
fn visible_animation_is_enabled_while_ai_pr_review_runs_in_the_background() {
    // AI Review is its own pane/mode now (independent of PR Triage), so the
    // background-throbber animation only applies while sitting in
    // `AppMode::AiReview` — not `PrReview`, even though the same
    // `ai_review_bg` background job can outlive either.
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    assert!(!app.has_visible_animation());

    let (_tx, rx) = std::sync::mpsc::channel();
    app.ai_review_bg = Some(rx);
    assert!(app.has_visible_animation());

    app.ai_review_bg = None;
    assert!(!app.has_visible_animation());
}

#[test]
fn visible_animation_is_enabled_for_pr_review_running_screens() {
    // Regression: `redraw_signature()` only hashes the mode's discriminant,
    // not its stage, so these full-screen loading/running views need an
    // unconditional `has_visible_animation` arm — otherwise the throbber only
    // advances on the rare frame something else forces a redraw, and just
    // sits frozen for the (potentially long) blocking `gh`/`claude` call in
    // between, reading as "nothing is happening".
    let mut app = pr_review_test_app();

    enter_pr_review(&mut app, 1);
    let pr = match &app.mode {
        AppMode::PrReview(state) => state.review.pr.clone(),
        _ => unreachable!(),
    };
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin: sample_ai_review_state(std::path::PathBuf::from("/tmp/wd"), pr),
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });
    assert!(app.has_visible_animation());

    app.mode = AppMode::PrReviewLoading(crate::app::PrReviewLoadState {
        workdir: std::path::PathBuf::from("/tmp/wd"),
        pr: crate::github::PrRef {
            number: 1,
            head_sha: "sha".to_string(),
            url: "https://github.com/o/r/pull/1".to_string(),
            owner: "o".to_string(),
            repo: "r".to_string(),
            head_ref: "main".to_string(),
        },
        usage_baselines: std::collections::HashMap::new(),
    });
    assert!(app.has_visible_animation());

    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };
    app.mode = AppMode::ReviewMemoryBootstrapRunning(crate::app::BootstrapRunState {
        scope: crate::app::review_memory::MemoryScope::Project,
        origin,
        depth: crate::app::pr_review::BootstrapDepth::default(),
        stage: crate::app::pr_review::BootstrapStage::FetchingComments,
    });
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
                triage_source: None,
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
                triage_source: None,
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
        .withf(
            |session, window, feature_session_id, resume_id, extra_args| {
                session == "amf-new-feature"
                    && window == "claude"
                    && !feature_session_id.is_empty()
                    && resume_id.is_none()
                    && extra_args.is_empty()
            },
        )
        .times(1)
        .returning(|_, _, _, _, _| Ok(()));
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
        agent: AgentKind::Claude,
        create_terminal: false,
        session_name: "Claude 1".into(),
        enable_chrome: false,
        remote_control: false,
        steering_enabled: false,
        hook_succeeded: None,
        startup_prompt: None,
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
fn accepting_an_on_demand_plan_writes_it_into_the_features_own_workdir() {
    let (mut app, _store_file, repo) = app_on_selected_feature();
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
    let plan = std::fs::read_to_string(repo.path().join(".claude/plan.md")).unwrap();
    assert!(plan.contains("Tighten the sidebar"));
    // Writing the file is not enough on its own: without the instruction block
    // the agent is never told the plan exists, and without the flag a restart
    // would stop injecting it.
    assert!(app.store.projects[0].features[0].plan_mode);
    let instructions = std::fs::read_to_string(repo.path().join("CLAUDE.local.md")).unwrap();
    assert!(instructions.contains(".claude/plan.md"));
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
            assert_eq!(target.plan_path, repo.path().join(".claude/plan.md"));
        }
        _ => panic!("expected the handoff prompt"),
    }
    // The offer comes after the accept has fully landed, so declining it can
    // never cost the plan.
    assert!(
        std::fs::read_to_string(repo.path().join(".claude/plan.md"))
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
            .contains(&repo.path().join(".claude/plan.md").display().to_string())
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
            assert!(seed.contains(".claude/plan.md"));
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
            .contains(&repo.path().join(".claude/plan.md").display().to_string())
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
            .contains(&repo.path().join(".claude/plan.md").display().to_string())
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
    assert!(message.contains(&repo.path().join(".claude/plan.md").display().to_string()));
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
    assert!(!repo.path().join(".claude/plan.md").exists());
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
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));
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
        mode: VibeMode::default(),
        review: false,
        plan_mode: true,
        agent: AgentKind::Claude,
        create_terminal: false,
        session_name: "Claude 1".into(),
        enable_chrome: false,
        remote_control: false,
        steering_enabled: false,
        hook_succeeded: None,
        startup_prompt: None,
    })
    .unwrap();
    // The NamedTempFile return value keeps the store file alive for the
    // caller; `repo` is returned too so the workdir it created stays alive
    // rather than being deleted the instant this function returns.
    (app, store_file, repo)
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
    assert!(!workdir.join(".claude/plan.md").exists());
    assert!(
        !app.store.projects[0]
            .features
            .iter()
            .any(|f| f.name == "planned-feature" && !f.pending_worktree_script)
    );

    crate::handlers::handle_plan_interview_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert_eq!(
        std::fs::read_to_string(workdir.join(".claude/plan.md")).unwrap(),
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
            assert!(seed.contains(".claude/plan.md"));
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
    assert!(!workdir.join(".claude/plan.md").exists());
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
        agent: AgentKind::Claude,
        create_terminal: false,
        session_name: "Claude 1".into(),
        enable_chrome: false,
        remote_control: false,
        steering_enabled: false,
        hook_succeeded: None,
        startup_prompt: None,
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
        agent: agent.clone(),
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
        triage_source: None,
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
fn poll_compose_submit_keeps_pasting_indicator_until_delivery_finishes() {
    let repo = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let mut state = ComposeState::new(
        compose_test_view(),
        repo.path().to_path_buf(),
        "Describe [Image 1]".to_string(),
        Vec::new(),
    );
    state.submit_in_progress = true;
    assert!(state.paste_in_progress());
    app.mode = AppMode::Compose(state);

    let (tx, rx) = std::sync::mpsc::channel();
    app.compose_submit = Some(ComposeSubmit {
        target: "amf-my-feat:claude".to_string(),
        rx,
    });

    assert!(!app.poll_compose_submit());
    assert!(matches!(&app.mode, AppMode::Compose(state) if state.paste_in_progress()));

    tx.send(Ok(())).unwrap();
    assert!(app.poll_compose_submit());
    assert!(matches!(&app.mode, AppMode::Viewing(view) if view.session == "amf-my-feat"));
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
        triage_source: None,
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
        triage_source: None,
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
fn claude_hook_commands_are_shell_quoted_for_paths_with_spaces() {
    let path = PathBuf::from("/Users/me/Library/Application Support/amf/save-prompt.sh");
    let command = crate::app::setup::claude_hook_command(&path);

    assert_eq!(
        command,
        "'/Users/me/Library/Application Support/amf/save-prompt.sh'"
    );
}

#[test]
fn cleanup_recognizes_quoted_managed_claude_hooks() {
    let workdir = TempDir::new().unwrap();
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let managed = crate::project::amf_config_dir().join("save-prompt.sh");
    let quoted = crate::app::setup::claude_hook_command(&managed);
    std::fs::write(
        claude_dir.join("settings.local.json"),
        format!(
            r#"{{
              "hooks": {{
                "UserPromptSubmit": [{{
                  "matcher": "",
                  "hooks": [{{
                    "type": "command",
                    "command": {quoted:?}
                  }}]
                }}]
              }}
            }}"#
        ),
    )
    .unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibe);

    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "UserPromptSubmit");
    assert_eq!(
        cmds.iter()
            .filter(|cmd| cmd.contains("save-prompt.sh"))
            .count(),
        1,
        "quoted managed hook should be replaced, not duplicated: {cmds:?}"
    );
}

#[test]
fn cleanup_recognizes_unquoted_macos_managed_claude_hooks() {
    let workdir = TempDir::new().unwrap();
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.local.json"),
        r#"{
          "hooks": {
            "Stop": [{
              "matcher": "",
              "hooks": [{
                "type": "command",
                "command": "/Users/me/Library/Application Support/amf/thinking-stop.sh"
              }]
            }]
          }
        }"#,
    )
    .unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibe);

    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "Stop");
    assert!(
        cmds.iter()
            .all(|cmd| !cmd.starts_with("/Users/me/Library/Application Support/amf/")),
        "unquoted macOS AMF hooks should be removed, got: {cmds:?}"
    );
    assert_eq!(
        cmds.iter()
            .filter(|cmd| cmd.contains("thinking-stop.sh"))
            .count(),
        1,
        "current quoted Stop hook should be the only thinking-stop hook: {cmds:?}"
    );
}

#[test]
fn cleanup_recognizes_quoted_temp_xdg_config_managed_claude_hooks() {
    // Regression test: the screenshot dev tool runs AMF with XDG_CONFIG_HOME
    // pointed at an isolated temp dir (e.g. /tmp/amf-shots/<ts>/config), so
    // AMF's own quoted hook commands land under `.../config/amf/<script>`
    // with no leading dot on `config`. These must still be recognized as
    // AMF-managed and replaced rather than accumulating forever.
    let workdir = TempDir::new().unwrap();
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.local.json"),
        r#"{
          "hooks": {
            "Stop": [{
              "matcher": "",
              "hooks": [{
                "type": "command",
                "command": "'/tmp/amf-shots/20260719-124607-366569/config/amf/thinking-stop.sh'"
              }]
            }]
          }
        }"#,
    )
    .unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibe);

    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "Stop");
    assert!(
        cmds.iter().all(|cmd| !cmd.contains("/tmp/amf-shots/")),
        "stale temp-XDG-config AMF hooks should be removed, got: {cmds:?}"
    );
    assert_eq!(
        cmds.iter()
            .filter(|cmd| cmd.contains("thinking-stop.sh"))
            .count(),
        1,
        "current quoted Stop hook should be the only thinking-stop hook: {cmds:?}"
    );
}

#[test]
fn cleanup_recognizes_arbitrary_xdg_config_home_managed_claude_hooks() {
    // Regression test: XDG_CONFIG_HOME is not required to end in `config` —
    // per the XDG spec it can point anywhere (e.g. XDG_CONFIG_HOME=/tmp/foo
    // resolves the AMF config dir to /tmp/foo/amf). Any such custom root
    // must still be recognized as AMF-managed, not just roots whose final
    // component happens to be literally `config`.
    let workdir = TempDir::new().unwrap();
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.local.json"),
        r#"{
          "hooks": {
            "Stop": [{
              "matcher": "",
              "hooks": [{
                "type": "command",
                "command": "'/tmp/foo/amf/thinking-stop.sh'"
              }]
            }]
          }
        }"#,
    )
    .unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibe);

    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "Stop");
    assert!(
        cmds.iter().all(|cmd| !cmd.contains("/tmp/foo/amf/")),
        "stale hooks from an arbitrary custom XDG_CONFIG_HOME should be removed, got: {cmds:?}"
    );
    assert_eq!(
        cmds.iter()
            .filter(|cmd| cmd.contains("thinking-stop.sh"))
            .count(),
        1,
        "current quoted Stop hook should be the only thinking-stop hook: {cmds:?}"
    );
}

#[test]
fn repair_unquoted_claude_hooks_refreshes_stale_stored_features() {
    let repo = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.local.json"),
        r#"{
          "hooks": {
            "Stop": [{
              "matcher": "",
              "hooks": [{
                "type": "command",
                "command": "/Users/me/Library/Application Support/amf/thinking-stop.sh"
              }]
            }]
          }
        }"#,
    )
    .unwrap();

    let now = Utc::now();
    let mut feature = Feature::new(
        "feat-1".to_string(),
        "feat-1".to_string(),
        workdir.path().to_path_buf(),
        true,
        VibeMode::Vibe,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );
    feature.status = ProjectStatus::Active;
    let store = ProjectStore {
        version: 5,
        projects: vec![Project {
            id: "proj-1".to_string(),
            name: "project".to_string(),
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

    let repaired = super::setup::repair_unquoted_claude_hooks_for_store(&store);

    assert_eq!(repaired, 1);
    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "Stop");
    assert!(
        cmds.iter()
            .all(|cmd| !cmd.starts_with("/Users/me/Library/Application Support/amf/")),
        "stale unquoted hook should be repaired, got: {cmds:?}"
    );
    assert!(
        cmds.iter().any(|cmd| cmd.starts_with('\'')
            && cmd.ends_with('\'')
            && cmd.contains("thinking-stop.sh")),
        "repaired hook should be quoted, got: {cmds:?}"
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
fn ensure_hooks_removes_stale_temp_home_amf_hooks() {
    let workdir = TempDir::new().unwrap();
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.local.json"),
        r#"{
          "hooks": {
            "PostToolUse": [
              {
                "matcher": "",
                "hooks": [{
                  "type": "command",
                  "command": "/tmp/claude-1000/worktree/scratchpad/amf_verify_home/.config/amf/tool-stop.sh"
                }]
              },
              {
                "matcher": "custom",
                "hooks": [{
                  "type": "command",
                  "command": "/tmp/user/tool-stop.sh"
                }]
              }
            ],
            "PreToolUse": [{
              "matcher": "",
              "hooks": [{
                "type": "command",
                "command": "/tmp/claude-1000/worktree/scratchpad/amf_verify_home/.config/amf/thinking-start.sh"
              }]
            }]
          }
        }"#,
    )
    .unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibe);

    let s = read_settings(&workdir);
    let post_cmds = hook_commands_for(&s, "PostToolUse");
    assert!(
        post_cmds
            .iter()
            .all(|cmd| !cmd.contains("/tmp/claude-1000/")),
        "stale temp-home AMF hooks should be removed, got: {post_cmds:?}"
    );
    assert!(
        post_cmds.iter().any(|cmd| cmd == "/tmp/user/tool-stop.sh"),
        "user hook with same basename outside .config/amf should be preserved"
    );
    assert_eq!(
        post_cmds
            .iter()
            .filter(|cmd| cmd
                .trim_matches('\'')
                .ends_with("/.config/amf/tool-stop.sh"))
            .count(),
        1,
        "only the current AMF PostToolUse hook should remain"
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
    let codex_notify =
        std::fs::read_to_string(workdir.path().join(".codex").join("amf-codex-notify.sh")).unwrap();
    assert!(
        codex_notify.contains("provider_session_id")
            && codex_notify.contains("amf_feature_session_id")
            && codex_notify.contains("AMF_FEATURE_SESSION_ID")
            && codex_notify.contains("AMF_ACTIVE"),
        "Codex notify hook should preserve AMF and provider session ids, got: {codex_notify}"
    );
    assert!(
        !workdir.path().join(".codex").join("config.toml").exists(),
        "repo-root codex feature should not write unsupported project-local config"
    );
    let screenshot_skill = workdir
        .path()
        .join(".agents/skills/amf-screenshot/SKILL.md");
    let screenshot_skill = std::fs::read_to_string(screenshot_skill)
        .expect("Codex features should get the AMF screenshot skill");
    assert!(
        screenshot_skill.contains("name: amf-screenshot")
            && screenshot_skill.contains("scripts/dev/screenshot/amf-capture.sh")
            && !screenshot_skill.contains("allowed-tools:")
            && !screenshot_skill.contains("Artifact tool"),
        "Codex should get its native screenshot workflow, got: {screenshot_skill}"
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
    assert!(
        !workdir
            .path()
            .join(".agents/skills/amf-screenshot")
            .exists(),
        "cleanup should remove the managed Codex screenshot skill"
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
        agent: agent.clone(),
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
        triage_source: None,
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
            token_usage: None,
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
            token_usage: None,
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
        token_usage: None,
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
        triage_source: None,
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
        token_usage: None,
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
        triage_source: None,
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

fn store_with_single_agent_session(
    workdir: &std::path::Path,
    agent: AgentKind,
    kind: SessionKind,
    session_id: &str,
    window: &str,
) -> ProjectStore {
    let now = Utc::now();
    let session = FeatureSession {
        id: session_id.to_string(),
        kind,
        label: window.to_string(),
        tmux_window: window.to_string(),
        claude_session_id: None,
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
        workdir: workdir.to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![session],
        collapsed: false,
        mode: VibeMode::default(),
        review: false,
        plan_mode: false,
        agent: agent.clone(),
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
        triage_source: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: workdir.to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: agent,
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
        token_usage: None,
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
        triage_source: None,
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
    let usage = app.store.projects[0].features[0].sessions[0]
        .token_usage
        .as_ref()
        .unwrap();
    assert_eq!(usage.input_tokens, 12);
    assert_eq!(usage.output_tokens, 4);
    assert_eq!(usage.total_tokens, 16);
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
        token_usage: None,
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
        triage_source: None,
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
fn sync_session_status_does_not_infer_stale_codex_usage_for_new_session() {
    let home = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let codex_dir = home.path().join(".codex");
    std::fs::create_dir_all(&codex_dir).unwrap();
    let rollout = codex_dir.join("old-rollout.jsonl");
    std::fs::write(
        &rollout,
        format!(
            concat!(
                "{{\"timestamp\":\"2026-03-13T14:00:00Z\",\"type\":\"session_meta\",\"payload\":{{\"id\":\"old-codex\",\"cwd\":\"{}\"}}}}\n",
                "{{\"timestamp\":\"2026-03-13T14:01:00Z\",\"type\":\"event_msg\",\"payload\":{{\"type\":\"token_count\",\"info\":{{\"total_token_usage\":{{\"input_tokens\":1000000,\"output_tokens\":1000000,\"total_tokens\":2000000}}}}}}}}\n"
            ),
            workdir.path().display()
        ),
    )
    .unwrap();
    let conn = rusqlite::Connection::open(codex_dir.join("state_5.sqlite")).unwrap();
    conn.execute_batch(
        "CREATE TABLE threads (
            id TEXT PRIMARY KEY,
            rollout_path TEXT NOT NULL,
            updated_at INTEGER NOT NULL,
            cwd TEXT NOT NULL,
            archived INTEGER NOT NULL DEFAULT 0
        );",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO threads (id, rollout_path, updated_at, cwd, archived)
         VALUES (?1, ?2, ?3, ?4, 0)",
        rusqlite::params![
            "old-codex",
            rollout.to_string_lossy(),
            Utc.with_ymd_and_hms(2026, 3, 13, 14, 0, 0)
                .unwrap()
                .timestamp(),
            workdir.path().to_string_lossy(),
        ],
    )
    .unwrap();

    let created_at = Utc.with_ymd_and_hms(2026, 3, 13, 14, 5, 0).unwrap();
    let session = FeatureSession {
        id: "codex-sess".to_string(),
        kind: SessionKind::Codex,
        label: "Codex".to_string(),
        tmux_window: "codex".to_string(),
        claude_session_id: None,
        token_usage_source: Some(TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "old-codex".to_string(),
        }),
        token_usage_source_match: Some(TokenUsageSourceMatch::Inferred),
        created_at,
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
        triage_source: None,
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

    let session = &app.store.projects[0].features[0].sessions[0];
    assert_eq!(session.token_usage_source, None);
    assert_eq!(session.token_usage_source_match, None);
    assert_eq!(session.status_text, None);
    assert_eq!(session.token_usage, None);
}

#[test]
fn sync_session_status_does_not_duplicate_inferred_sources_in_feature() {
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

    let created_at = Utc::now();
    let first = FeatureSession {
        id: "codex-sess-1".to_string(),
        kind: SessionKind::Codex,
        label: "Codex 1".to_string(),
        tmux_window: "codex".to_string(),
        claude_session_id: None,
        token_usage_source: None,
        token_usage_source_match: None,
        created_at,
        command: None,
        on_stop: None,
        pre_check: None,
        status_text: None,
        token_usage: None,
    };
    let second = FeatureSession {
        id: "codex-sess-2".to_string(),
        label: "Codex 2".to_string(),
        tmux_window: "codex-2".to_string(),
        created_at: created_at + chrono::Duration::milliseconds(100),
        ..first.clone()
    };
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "my-feat".to_string(),
        branch: "my-feat".to_string(),
        workdir: workdir.path().to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-my-feat".to_string(),
        sessions: vec![first, second],
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
        triage_source: None,
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

    let sessions = &app.store.projects[0].features[0].sessions;
    assert_eq!(
        sessions[0].token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "codex-1".to_string(),
        }),
    );
    assert_eq!(
        sessions[0].token_usage_source_match,
        Some(TokenUsageSourceMatch::Inferred),
    );
    assert_eq!(sessions[1].token_usage_source, None);
    assert_eq!(sessions[1].token_usage_source_match, None);
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
        token_usage: None,
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
        triage_source: None,
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
fn note_codex_prompt_submit_marks_codex_session_in_non_codex_feature_thinking() {
    let workdir = TempDir::new().unwrap();
    let mut store = store_with_codex_session(workdir.path(), false);
    store.projects[0].features[0].agent = AgentKind::Claude;
    store.projects[0].features[0].sessions[0].label = "PR triage".to_string();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.note_codex_prompt_submit("amf-my-feat", "codex");

    assert!(app.ipc_thinking_sessions.contains("amf-my-feat"));
    assert!(app.ipc_thinking_feature_sessions.contains("codex-sess"));
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
fn ipc_turn_end_archives_superseded_review_notes_for_review_features() {
    let workdir = TempDir::new().unwrap();
    let claude = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        claude.join("review-notes.md"),
        "## src/main.rs — first\n\nOld context.\n\n---\n\n\
         ## src/main.rs — current\n\nCurrent context.\n\n---\n",
    )
    .unwrap();

    let mut store = store_with_codex_session(workdir.path(), false);
    store.projects[0].features[0].review = true;
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
        "message": "Codex finished and is waiting for input"
    }));

    let live = std::fs::read_to_string(claude.join("review-notes.md")).unwrap();
    assert!(live.contains("Current context."));
    assert!(!live.contains("Old context."));
    let archive = std::fs::read_to_string(claude.join("review-notes-archive.md")).unwrap();
    assert!(archive.contains("Old context."));
    assert!(!archive.contains("Current context."));
}

#[test]
fn ipc_event_binds_exact_codex_usage_source_by_feature_session_id() {
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
        "provider_session_id": "codex-provider-123",
        "amf_feature_session_id": "codex-sess",
        "cwd": workdir.path().display().to_string(),
        "message": "Codex finished and is waiting for input"
    }));

    let session = &app.store.projects[0].features[0].sessions[0];
    assert_eq!(
        session.token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "codex-provider-123".to_string(),
        })
    );
    assert_eq!(
        session.token_usage_source_match,
        Some(TokenUsageSourceMatch::Exact)
    );
}

#[test]
fn ipc_event_binds_exact_claude_usage_source_by_feature_session_id() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_single_agent_session(
        workdir.path(),
        AgentKind::Claude,
        SessionKind::Claude,
        "claude-sess",
        "claude",
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": "amf-my-feat",
        "provider_session_id": "claude-provider-123",
        "amf_feature_session_id": "claude-sess",
        "cwd": workdir.path().display().to_string()
    }));

    let session = &app.store.projects[0].features[0].sessions[0];
    assert_eq!(
        session.token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "claude-provider-123".to_string(),
        })
    );
    assert_eq!(
        session.token_usage_source_match,
        Some(TokenUsageSourceMatch::Exact)
    );
}

#[test]
fn ipc_event_binds_exact_opencode_usage_source_by_feature_session_id() {
    let workdir = TempDir::new().unwrap();
    let store = store_with_single_agent_session(
        workdir.path(),
        AgentKind::Opencode,
        SessionKind::Opencode,
        "opencode-sess",
        "opencode",
    );
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "input-request",
        "session_id": "opencode-provider-123",
        "provider_session_id": "opencode-provider-123",
        "amf_feature_session_id": "opencode-sess",
        "cwd": workdir.path().display().to_string(),
        "message": "Agent finished and is waiting for input"
    }));

    let session = &app.store.projects[0].features[0].sessions[0];
    assert_eq!(
        session.token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Opencode,
            id: "opencode-provider-123".to_string(),
        })
    );
    assert_eq!(
        session.token_usage_source_match,
        Some(TokenUsageSourceMatch::Exact)
    );
}

#[test]
fn ipc_exact_usage_source_replaces_inferred_source() {
    let workdir = TempDir::new().unwrap();
    let mut store = store_with_codex_session(workdir.path(), false);
    let session = &mut store.projects[0].features[0].sessions[0];
    session.set_token_usage_source_inferred(TokenUsageSource {
        provider: TokenUsageProvider::Codex,
        id: "old-inferred-codex".to_string(),
    });
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-stop",
        "source": "codex-notify",
        "session_id": "amf-my-feat",
        "provider_session_id": "exact-codex-session",
        "amf_feature_session_id": "codex-sess",
        "cwd": workdir.path().display().to_string()
    }));

    let session = &app.store.projects[0].features[0].sessions[0];
    assert_eq!(
        session.token_usage_source,
        Some(TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "exact-codex-session".to_string(),
        })
    );
    assert_eq!(
        session.token_usage_source_match,
        Some(TokenUsageSourceMatch::Exact)
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
            token_usage: None,
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
        token_usage: None,
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
        triage_source: None,
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
        triage_source: None,
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

// ── Prompt library picker ─────────────────────────────────────

fn store_with_prompt_templates(names: &[&str]) -> ProjectStore {
    let templates = names
        .iter()
        .map(|name| {
            crate::prompt_library::PromptTemplate::new(name.to_string(), format!("body of {name}"))
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
    app.config.extension.prompt_templates = vec![crate::prompt_library::PromptTemplate::new(
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
    let key =
        crossterm::event::KeyEvent::new(KeyCode::Char('A'), crossterm::event::KeyModifiers::NONE);
    crate::handlers::handle_normal_key(&mut app, key).unwrap();
    assert!(matches!(app.mode, AppMode::HarnessSetup(_)));
}

fn pr_review_test_app() -> App {
    let store = ProjectStore {
        version: 5,
        projects: vec![],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    };
    App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    )
}

fn pr_review_with_comments(n: u64) -> crate::app::pr_review::PrReview {
    let comments: Vec<crate::github::ReviewComment> = (1..=n)
        .map(|id| crate::github::ReviewComment {
            id,
            path: Some(format!("src/file{id}.rs")),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("comment {id}"),
            user: crate::github::GhUser {
                login: "alice".to_string(),
                kind: "User".to_string(),
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    crate::app::pr_review::normalize(pr, comments, vec![], vec![], vec![])
}

fn enter_pr_review(app: &mut App, n: u64) {
    app.mode = AppMode::PrReview(PrReviewState {
        workdir: std::path::PathBuf::from("/tmp/wd"),
        review: pr_review_with_comments(n),
        selected: 0,
        detail_scroll: 0,
        detail_content_lines: 0,
        hide_resolved: false,
        sort_mode: crate::app::pr_review::PrSortMode::default(),
        fix_target: crate::app::pr_review::FixTarget::default(),
        fix_target_picked: false,
        usage_baselines: std::collections::HashMap::new(),
        review_harness: None,
        harness_pick: None,
        new_feature_setup: None,
        integrate: None,
        fix_confirm: None,
        fix_vim_enabled: false,
        mark_pick: None,
        reply_kind_pick: None,
        reply: None,
        memory_add: None,
        marked: std::collections::HashSet::new(),
        pending_batch: false,
        checked_out_branch: Some("main".to_string()),
        pending_ai_review_findings: 0,
    });
}

fn pr_review_selected(app: &App) -> usize {
    match &app.mode {
        AppMode::PrReview(state) => state.selected,
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn pr_review_navigation_clamps_at_both_ends() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);

    // Already at top: prev is a no-op.
    app.pr_review_select_prev();
    assert_eq!(pr_review_selected(&app), 0);

    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 1);
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 2);

    // At the bottom: next is a no-op (no wrap).
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 2);
}

#[test]
fn pr_review_close_returns_to_dashboard() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.close_pr_review();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.pr_review_bg.is_none());
}

#[test]
fn pr_review_toggle_to_session_requires_existing_session() {
    // `store_with_feature` has no sessions, so there's no dedicated triage
    // session to jump to yet — the toggle must hint rather than create one
    // as a side effect of a quick peek.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_toggle_to_session().unwrap();

    assert!(matches!(app.mode, AppMode::PrReview(_)));
    assert!(app.pr_review_return.is_none());
}

#[test]
fn pr_review_toggle_to_session_jumps_and_stashes_state() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
        state.detail_scroll = 3;
    }

    app.pr_review_toggle_to_session().unwrap();

    match &app.mode {
        AppMode::Viewing(view) => {
            assert_eq!(view.session, "amf-my-feat");
            assert_eq!(view.session_label, "PR Triage");
        }
        other => panic!("expected Viewing, got {:?}", std::mem::discriminant(other)),
    }
    let stash = app
        .pr_review_return
        .as_ref()
        .expect("pane state should be stashed");
    assert_eq!(stash.session, "amf-my-feat");
    assert_eq!(stash.state.selected, 1);
    assert_eq!(stash.state.detail_scroll, 3);
}

#[test]
fn pr_review_return_to_pane_restores_stashed_state() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
    }
    app.pr_review_toggle_to_session().unwrap();

    app.pr_review_return_to_pane();

    match &app.mode {
        AppMode::PrReview(state) => assert_eq!(state.selected, 1),
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(
        app.pr_review_return.is_none(),
        "stash should be consumed on restore"
    );
}

#[test]
fn pr_review_return_to_pane_ignores_mismatched_session() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_toggle_to_session().unwrap();

    // Simulate having navigated away to an unrelated session's view.
    if let AppMode::Viewing(view) = &mut app.mode {
        view.session = "amf-other-feat".to_string();
        view.window = "claude".to_string();
    }

    app.pr_review_return_to_pane();

    assert!(
        matches!(&app.mode, AppMode::Viewing(view) if view.session == "amf-other-feat"),
        "should not have jumped into the stashed pane from an unrelated session"
    );
    assert!(
        app.pr_review_return.is_some(),
        "the stash should be left alone, not dropped, in case the user navigates back"
    );
}

#[test]
fn pr_review_return_to_pane_without_stash_shows_message() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Normal;

    app.pr_review_return_to_pane();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.pr_review_return.is_none());
}

#[test]
fn pr_review_inject_fix_also_stashes_return_state() {
    // Regression: `f` (inject fix) used to drop the pane's state on the floor
    // when leaving for the fix session, so `leader+P` had nothing to restore
    // even though `f` is the far more common way into that session (`P` is
    // just a peek). `f` must stash exactly like `P` does.
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
    }
    // Go through the real confirm-dialog path (`f` opens it, then the user
    // confirms) rather than the no-dialog fallback — a prior version of this
    // fix left the confirm dialog attached to the stashed state, so returning
    // via leader+P reopened the same "inject fix" dialog instead of the plain
    // comment list.
    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.fix_confirm.is_some()),
        "confirm dialog should be open before injecting"
    );
    let request_id = match &app.mode {
        AppMode::PrReview(state) => state.fix_confirm.as_ref().unwrap().reply_draft_requests[0]
            .request_id
            .clone(),
        _ => unreachable!(),
    };

    app.pr_review_inject_fix().unwrap();

    // Confirming the injection activates this dialog's request id. The agent
    // can now return a draft through IPC; an id from an older dialog could not
    // overwrite it.
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .capture_pr_comment_reply_draft(7, 2, &request_id, "Fixed the selected path.")
            .unwrap()
    );

    // Compose intercept is on by default, so `f` lands in the compose box
    // (seeded with the fix prompt) rather than bare Viewing — the stash must
    // still be keyed to the same session/window either way.
    let compose_view = match &app.mode {
        AppMode::Compose(state) => state.view.clone(),
        other => panic!("expected Compose, got {:?}", std::mem::discriminant(other)),
    };
    let stash = app
        .pr_review_return
        .as_ref()
        .expect("f should stash the pane state just like the P toggle");
    assert_eq!(stash.session, compose_view.session);
    assert_eq!(stash.window, compose_view.window);
    assert_eq!(stash.state.selected, 1);
    let stash_session = stash.session.clone();

    // Ctrl+Space from the compose box cancels it back to the underlying
    // Viewing session (handlers/compose.rs) before handing off to the leader
    // chord — the real path `leader+P` is reached through after `f`.
    app.cancel_compose();
    assert!(matches!(&app.mode, AppMode::Viewing(view) if view.session == stash_session));

    app.pr_review_return_to_pane();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.selected, 1);
            assert!(
                state.fix_confirm.is_none(),
                "the already-actioned confirm dialog must not reappear"
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.pr_review_return.is_none());

    app.pr_review_open_reply_done();
    assert_eq!(reply_editor_text(&app), "Fixed the selected path.");
}

#[test]
fn pr_review_inject_fix_targets_dialogs_original_comment_after_selection_moves() {
    // Regression: a PR Triage refresh (e.g. the automatic one after posting
    // an AI review) can drop the comment a still-open fix-confirm dialog was
    // built for, falling the selection back onto a different comment while
    // the stale dialog stays open. Confirming must still mark — and route
    // the reply-draft handoff to — the dialog's original comment, never
    // whatever the refresh happened to leave selected.
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);
    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1; // comment id 2
    }
    app.pr_review_open_fix_confirm();
    let request_id = match &app.mode {
        AppMode::PrReview(state) => state.fix_confirm.as_ref().unwrap().reply_draft_requests[0]
            .request_id
            .clone(),
        _ => unreachable!(),
    };

    // Simulate the refresh: comment 2's thread resolved upstream and dropped
    // out of the fetched set, so selection fell back to comment 1. The dialog
    // (still targeting comment 2 via its own `reply_draft_requests`) is left
    // open, exactly as `apply_refreshed_pr_review_state` leaves it.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.comments.retain(|c| c.id != 2);
        state.selected = 0;
    }

    app.pr_review_inject_fix().unwrap();

    let triage = app.db.as_ref().unwrap().load_pr_comment_triage(7).unwrap();
    assert!(
        !triage.contains_key(&1),
        "the newly-selected comment must not be marked Fixing"
    );
    assert_eq!(
        triage.get(&2).map(|(state, _)| *state),
        Some(crate::app::pr_review::TriageState::Fixing),
        "the dialog's original comment must still be marked Fixing"
    );
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .capture_pr_comment_reply_draft(7, 2, &request_id, "Fixed the selected path.")
            .unwrap(),
        "the reply-draft handoff must still route to the dialog's original comment"
    );
}

#[test]
fn pr_review_i_opens_syntax_picker_for_selected_comment_file() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2); // comments have paths src/file{id}.rs (Rust)

    app.open_syntax_language_picker_for_selected_diff_file();

    match &app.mode {
        AppMode::SyntaxLanguagePicker(state) => {
            assert_eq!(
                state.languages[state.selected].language,
                crate::highlight::HighlightLanguage::Rust
            );
            // The picker returns to the same review pane on close.
            assert!(matches!(
                state.return_to.as_deref(),
                Some(AppMode::PrReview(_))
            ));
        }
        other => panic!(
            "expected syntax picker, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

/// Enter the review pane against the `store_with_feature` feature so the fix
/// flow can resolve a real feature/workdir (`/tmp/test-workdir`).
fn enter_pr_review_for_feature(app: &mut App, n: u64) {
    app.mode = AppMode::PrReview(PrReviewState {
        workdir: std::path::PathBuf::from("/tmp/test-workdir"),
        review: pr_review_with_comments(n),
        selected: 0,
        detail_scroll: 0,
        detail_content_lines: 0,
        hide_resolved: false,
        sort_mode: crate::app::pr_review::PrSortMode::default(),
        fix_target: crate::app::pr_review::FixTarget::default(),
        fix_target_picked: false,
        usage_baselines: std::collections::HashMap::new(),
        review_harness: None,
        harness_pick: None,
        new_feature_setup: None,
        integrate: None,
        fix_confirm: None,
        fix_vim_enabled: false,
        mark_pick: None,
        reply_kind_pick: None,
        reply: None,
        memory_add: None,
        marked: std::collections::HashSet::new(),
        pending_batch: false,
        checked_out_branch: Some("main".to_string()),
        pending_ai_review_findings: 0,
    });
}

/// A minimal, freshly-opened `AiReviewState` for `workdir`/`pr` — no findings,
/// no picker/dialog overlays open.
fn sample_ai_review_state(
    workdir: std::path::PathBuf,
    pr: crate::github::PrRef,
) -> crate::app::AiReviewState {
    crate::app::AiReviewState {
        workdir,
        pr,
        findings: Vec::new(),
        summary: None,
        selected: 0,
        detail_scroll: 0,
        detail_content_lines: 0,
        last_run: None,
        harness: None,
        harness_pick: None,
        harness_pick_origin: None,
        model: None,
        model_picked: false,
        model_pick: None,
        finding_editor: None,
        post_confirm: None,
    }
}

fn sample_ai_review_finding(body: &str) -> crate::app::ai_review::AiReviewFinding {
    crate::app::ai_review::AiReviewFinding {
        path: None,
        line: None,
        body: body.to_string(),
        diff_hunk: None,
        skipped: false,
        published: false,
    }
}

/// Enter the AI Review pane against the `store_with_feature` feature so tests
/// can exercise its lifecycle without a real `gh` call.
fn enter_ai_review_for_feature(app: &mut App) {
    let pr = crate::github::PrRef {
        number: 321,
        head_sha: "abc123".to_string(),
        url: "https://github.com/o/r/pull/321".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    app.mode = AppMode::AiReview(sample_ai_review_state(
        std::path::PathBuf::from("/tmp/test-workdir"),
        pr,
    ));
}

#[test]
fn ai_review_model_picker_opens_for_pi() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Pi);
    }

    app.start_ai_pr_review();

    match &app.mode {
        AppMode::AiReview(state) => {
            // Pi's headless CLI takes `--model`, so it must get the picker
            // rather than being force-skipped to the harness default.
            assert!(!state.model_picked);
            let pick = state
                .model_pick
                .as_ref()
                .expect("Pi should open the model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
        }
        _ => panic!("expected AI Review pane"),
    }
    assert!(app.ai_review_bg.is_none(), "review should not have started");
}

#[test]
fn ai_review_model_picker_backs_through_custom_editor_and_rebuilds_for_new_harness() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .times(1)
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.config.review_model = Some("sonnet".to_string());
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Claude);
        state.model = Some("sonnet".to_string());
        state.model_pick = Some(AiModelPickState {
            rows: vec![
                ModelPickRow::Default,
                ModelPickRow::Preset("sonnet"),
                ModelPickRow::Custom,
            ],
            selected: 2,
            custom_input: "claude-custom".to_string(),
            editing_custom: true,
        });
    }

    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::AiReview(state) => {
            let pick = state.model_pick.as_ref().expect("model list should remain");
            assert!(!pick.editing_custom);
            assert_eq!(pick.custom_input, "claude-custom");
            assert!(state.harness_pick.is_none());
        }
        _ => panic!("expected AI Review pane"),
    }

    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::AiReview(state) => {
            let pick = state
                .harness_pick
                .as_ref()
                .expect("model list should return to harness picker");
            assert_eq!(pick.agents[pick.selected], AgentKind::Claude);
            assert!(state.harness.is_none());
            assert!(state.model.is_none());
            assert!(!state.model_picked);
            assert!(state.model_pick.is_none());
        }
        _ => panic!("expected AI Review pane"),
    }

    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();

    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            assert!(state.harness_pick.is_none());
            assert!(state.model.is_none());
            assert!(!state.model_picked);
            let pick = state
                .model_pick
                .as_ref()
                .expect("new harness should open a rebuilt model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
            assert_eq!(pick.selected, 0);
            assert!(pick.custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }
}

/// Regression for the gap `ai_review_model_picker_backs_through_custom_editor_and_rebuilds_for_new_harness`
/// doesn't cover: backing out *again* after switching harness once, and
/// reconfirming the *same* (already-switched-to) harness a second time.
/// `harness_changed` used to compare only against the immediately preceding
/// screen, so this second confirm looked unchanged and fell through to
/// `start_ai_pr_review`'s `AppConfig::review_model` fallback — reseeding the
/// Claude-only "sonnet" preset as a custom Opencode model, exactly what the
/// first switch had correctly cleared.
#[test]
fn ai_review_harness_pick_reconfirm_after_backing_out_does_not_reseed_stale_model() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .times(2)
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.config.review_model = Some("sonnet".to_string());
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Claude);
        state.model_pick = Some(AiModelPickState {
            rows: vec![
                ModelPickRow::Default,
                ModelPickRow::Preset("sonnet"),
                ModelPickRow::Custom,
            ],
            selected: 0,
            custom_input: String::new(),
            editing_custom: false,
        });
    }

    // Back out of the model picker (Claude -> harness picker) and switch to
    // Opencode: this is the already-covered case, so the model resets to
    // fresh defaults instead of "sonnet".
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            let pick = state.model_pick.as_ref().expect("rebuilt model picker");
            assert!(pick.custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Back out a second time without picking a model, and reconfirm the same
    // (already-switched-to) Opencode harness.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::AiReview(state) => {
            let pick = state
                .harness_pick
                .as_ref()
                .expect("should return to harness picker");
            assert_eq!(pick.agents[pick.selected], AgentKind::Opencode);
        }
        _ => panic!("expected AI Review pane"),
    }
    app.accept_selected_ai_review_harness_for_test();

    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            let pick = state
                .model_pick
                .as_ref()
                .expect("reconfirming should still open a fresh model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
            assert_eq!(pick.selected, 0);
            assert!(
                pick.custom_input.is_empty(),
                "must not reseed the Claude-only \"sonnet\" default as an Opencode custom model"
            );
        }
        _ => panic!("expected AI Review pane"),
    }
}

/// Broader regression covering the reviewer's general "repeated navigation"
/// concern rather than just a single back-and-forth: hops through three
/// harnesses (Claude -> Opencode -> Codex -> Opencode -> Claude), backing out
/// to the harness picker between every hop. Every switch away from the
/// original Claude harness must land on fresh `Default`/empty model defaults
/// (never a stale non-Claude value), and switching *back* to the original
/// Claude harness must still restore the meaningful `AppConfig::review_model`
/// default ("sonnet") rather than being needlessly cleared just because a
/// picker round trip happened.
#[test]
fn ai_review_harness_pick_survives_multi_hop_navigation() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .times(4)
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.config.review_model = Some("sonnet".to_string());
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Claude);
        state.model_pick = Some(AiModelPickState {
            rows: vec![
                ModelPickRow::Default,
                ModelPickRow::Preset("sonnet"),
                ModelPickRow::Custom,
            ],
            selected: 1,
            custom_input: String::new(),
            editing_custom: false,
        });
    }

    // Claude -> Opencode: first switch away from the original harness.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            assert!(state.model_pick.as_ref().unwrap().custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Opencode -> Codex: still away from the original, still fresh defaults.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Codex));
            let pick = state.model_pick.as_ref().expect("rebuilt model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
            assert!(pick.custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Codex -> Opencode: back to a previously-visited harness, still not the
    // original — must not resurrect anything stale.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Up)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            assert!(state.model_pick.as_ref().unwrap().custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Opencode -> Claude: switching back to the *original* harness should
    // restore the configured "sonnet" default, not force an empty Custom.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Up)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Claude));
            let pick = state.model_pick.as_ref().expect("rebuilt model picker");
            assert_eq!(
                pick.rows.get(pick.selected),
                Some(&ModelPickRow::Preset("sonnet")),
                "returning to the original harness should still honor the configured default"
            );
        }
        _ => panic!("expected AI Review pane"),
    }
}

/// Enter the PR picker directly (bypassing the real `gh pr list` call
/// `open_pr_picker` would make) so bootstrap-pick tests can exercise the
/// overlay's state transitions without hitting the network.
fn enter_pr_picker_for_test(app: &mut App) {
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: std::path::PathBuf::from("/tmp/test-workdir"),
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        compact_confirm: None,
        current_user: None,
    });
}

#[test]
fn review_memory_bootstrap_pick_opens_defaulting_to_fifty() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);

    assert!(!app.review_memory_bootstrap_picking());
    app.open_review_memory_bootstrap_pick();
    assert!(app.review_memory_bootstrap_picking());

    match &app.mode {
        AppMode::PrPicker(state) => {
            let pick = state.bootstrap_pick.as_ref().unwrap();
            assert_eq!(
                crate::app::pr_review::BootstrapDepth::ALL[pick.selected],
                crate::app::pr_review::BootstrapDepth::default()
            );
        }
        other => panic!("expected PrPicker, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn review_memory_bootstrap_pick_defaults_to_project_scope_and_toggles() {
    use crate::app::review_memory::MemoryScope;

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    app.open_review_memory_bootstrap_pick();

    let scope = |app: &App| match &app.mode {
        AppMode::PrPicker(state) => state.bootstrap_pick.as_ref().unwrap().scope,
        _ => panic!("expected PrPicker"),
    };
    assert_eq!(
        scope(&app),
        MemoryScope::Project,
        "a bootstrap learns from this repo's history, so it defaults to this repo's doc"
    );

    app.review_memory_bootstrap_toggle_scope();
    assert_eq!(scope(&app), MemoryScope::Global);
    app.review_memory_bootstrap_toggle_scope();
    assert_eq!(scope(&app), MemoryScope::Project);
}

#[test]
fn review_memory_bootstrap_pick_move_wraps() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    app.open_review_memory_bootstrap_pick();

    let selected = |app: &App| match &app.mode {
        AppMode::PrPicker(state) => state.bootstrap_pick.as_ref().unwrap().selected,
        _ => panic!("expected PrPicker"),
    };

    // Starts on the default (Fifty, index 1).
    assert_eq!(selected(&app), 1);
    app.review_memory_bootstrap_pick_move(1);
    assert_eq!(selected(&app), 2);
    app.review_memory_bootstrap_pick_move(1);
    assert_eq!(selected(&app), 3);
    // Wraps forward past the last entry.
    app.review_memory_bootstrap_pick_move(1);
    assert_eq!(selected(&app), 0);
    // Wraps backward past the first entry.
    app.review_memory_bootstrap_pick_move(-1);
    assert_eq!(selected(&app), 3);
}

#[test]
fn review_memory_bootstrap_pick_cancel_closes_the_overlay() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    app.open_review_memory_bootstrap_pick();
    assert!(app.review_memory_bootstrap_picking());

    app.review_memory_bootstrap_pick_cancel();

    assert!(!app.review_memory_bootstrap_picking());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
}

#[test]
fn poll_review_memory_bootstrap_bg_surfaces_result_and_returns_to_picker() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_bootstrap_bg = Some(rx);
    app.mode = AppMode::ReviewMemoryBootstrapRunning(crate::app::BootstrapRunState {
        scope: crate::app::review_memory::MemoryScope::Project,
        origin,
        depth: crate::app::pr_review::BootstrapDepth::default(),
        stage: crate::app::pr_review::BootstrapStage::FetchingComments,
    });

    tx.send(crate::app::pr_review::BootstrapProgress::Distilling {
        pr_count: 3,
        token_estimate: 42,
    })
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    match &app.mode {
        AppMode::ReviewMemoryBootstrapRunning(state) => assert_eq!(
            state.stage,
            crate::app::pr_review::BootstrapStage::Distilling {
                pr_count: 3,
                token_estimate: 42,
            }
        ),
        other => panic!(
            "expected ReviewMemoryBootstrapRunning, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    tx.send(crate::app::pr_review::BootstrapProgress::Done(Ok(
        crate::app::pr_review::BootstrapOutcome {
            pr_count: 3,
            appended: 2,
        },
    )))
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert!(app.review_memory_bootstrap_bg.is_none());
}

#[test]
fn poll_review_memory_bootstrap_bg_error_still_returns_to_picker() {
    // Regression: `show_error` unconditionally resets `self.mode` to `Normal`
    // for any non-Normal/Help/Viewing mode. The `Done(Err(_))` branch used to
    // call it *before* restoring the origin picker, so a failed distill (e.g.
    // the headless `claude` call erroring) dumped the user onto the bare
    // dashboard instead of back to the PR picker like the success path does.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_bootstrap_bg = Some(rx);
    app.mode = AppMode::ReviewMemoryBootstrapRunning(crate::app::BootstrapRunState {
        scope: crate::app::review_memory::MemoryScope::Project,
        origin,
        depth: crate::app::pr_review::BootstrapDepth::default(),
        stage: crate::app::pr_review::BootstrapStage::FetchingComments,
    });

    tx.send(crate::app::pr_review::BootstrapProgress::Done(Err(
        anyhow::anyhow!("claude headless command failed"),
    )))
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    match &app.mode {
        AppMode::PrPicker(state) => {
            assert!(
                state
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("claude headless command failed")),
                "expected the picker's inline error to surface the failure, got {:?}",
                state.error
            );
        }
        other => panic!("expected PrPicker, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.review_memory_bootstrap_bg.is_none());
}

#[test]
fn cancel_review_memory_bootstrap_returns_to_picker_without_dropping_the_bg_result() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_bootstrap_bg = Some(rx);
    app.mode = AppMode::ReviewMemoryBootstrapRunning(crate::app::BootstrapRunState {
        scope: crate::app::review_memory::MemoryScope::Project,
        origin,
        depth: crate::app::pr_review::BootstrapDepth::default(),
        stage: crate::app::pr_review::BootstrapStage::FetchingComments,
    });

    // The user gives up watching...
    app.cancel_review_memory_bootstrap();
    assert!(matches!(app.mode, AppMode::PrPicker(_)));

    // ...but the background run still finishes and reports its result (a real
    // side effect: it wrote findings and spent tokens), even though the user
    // isn't looking at the running screen anymore.
    tx.send(crate::app::pr_review::BootstrapProgress::Done(Ok(
        crate::app::pr_review::BootstrapOutcome {
            pr_count: 5,
            appended: 1,
        },
    )))
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    assert!(app.review_memory_bootstrap_bg.is_none());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
}

#[test]
fn open_review_memory_compact_confirm_reads_doc_and_opens() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- One\n- Two\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        compact_confirm: None,
        current_user: None,
    });

    assert!(!app.review_memory_compact_confirming());
    app.open_review_memory_compact_confirm();
    assert!(app.review_memory_compact_confirming());

    match &app.mode {
        AppMode::PrPicker(state) => {
            let confirm = state.compact_confirm.as_ref().unwrap();
            assert_eq!(confirm.existing_findings, 2);
            assert_eq!(
                confirm.scope,
                crate::app::review_memory::MemoryScope::Project
            );
        }
        other => panic!("expected PrPicker, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn open_review_memory_compact_confirm_uses_global_when_only_global_has_findings() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let global_doc = tmp.path().join("global-review-memory.md");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        &global_doc,
        "# Review memory (cross-project)\n\n## Tests\n- One\n- Two\n- Three\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        compact_confirm: None,
        current_user: None,
    });

    app.open_review_memory_compact_confirm();

    match &app.mode {
        AppMode::PrPicker(state) => {
            let confirm = state.compact_confirm.as_ref().unwrap();
            assert_eq!(confirm.existing_findings, 3);
            assert_eq!(
                confirm.scope,
                crate::app::review_memory::MemoryScope::Global
            );
        }
        other => panic!("expected PrPicker, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn review_memory_compact_toggle_scope_rereads_the_selected_doc() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let global_doc = tmp.path().join("global-review-memory.md");
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- Project one\n- Project two\n",
    )
    .unwrap();
    std::fs::write(
        &global_doc,
        "# Review memory (cross-project)\n\n## Tests\n- Global one\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        compact_confirm: None,
        current_user: None,
    });

    let compact_selection = |app: &App| match &app.mode {
        AppMode::PrPicker(state) => {
            let confirm = state.compact_confirm.as_ref().unwrap();
            (confirm.scope, confirm.existing_findings)
        }
        _ => panic!("expected PrPicker"),
    };

    app.open_review_memory_compact_confirm();
    assert_eq!(
        compact_selection(&app),
        (crate::app::review_memory::MemoryScope::Project, 2)
    );

    app.review_memory_compact_toggle_scope();
    assert_eq!(
        compact_selection(&app),
        (crate::app::review_memory::MemoryScope::Global, 1)
    );

    app.review_memory_compact_toggle_scope();
    assert_eq!(
        compact_selection(&app),
        (crate::app::review_memory::MemoryScope::Project, 2)
    );
}

#[test]
fn open_review_memory_compact_confirm_bails_when_doc_missing() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        compact_confirm: None,
        current_user: None,
    });

    app.open_review_memory_compact_confirm();

    assert!(!app.review_memory_compact_confirming());
    assert!(app.message.is_some());
}

#[test]
fn review_memory_compact_confirm_cancel_closes_the_overlay() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    if let AppMode::PrPicker(state) = &mut app.mode {
        state.compact_confirm = Some(crate::app::CompactConfirmState {
            existing_findings: 3,
            scope: crate::app::review_memory::MemoryScope::Project,
        });
    }
    assert!(app.review_memory_compact_confirming());

    app.review_memory_compact_confirm_cancel();

    assert!(!app.review_memory_compact_confirming());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
}

#[test]
fn review_memory_compact_confirm_run_targets_the_selected_global_doc() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let global_doc = tmp.path().join("global-review-memory.md");
    std::fs::create_dir_all(&repo).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        compact_confirm: Some(crate::app::CompactConfirmState {
            // The missing file makes the spawned background read return
            // immediately, while this cached count still lets us exercise the
            // selected-path handoff without invoking an agent in the test.
            existing_findings: 1,
            scope: crate::app::review_memory::MemoryScope::Global,
        }),
        current_user: None,
    });

    app.review_memory_compact_confirm_run();

    match &app.mode {
        AppMode::ReviewMemoryCompactRunning(state) => {
            assert_eq!(state.scope, crate::app::review_memory::MemoryScope::Global);
            assert_eq!(state.path, global_doc);
            assert!(state.origin.compact_confirm.is_none());
        }
        other => panic!(
            "expected ReviewMemoryCompactRunning, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    assert_eq!(
        app.review_memory_compact_pending
            .as_ref()
            .map(|state| (&state.path, state.scope)),
        Some((&global_doc, crate::app::review_memory::MemoryScope::Global))
    );
}

#[test]
fn poll_review_memory_compact_bg_success_opens_review_dialog() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    let run_state = crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Global,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    };
    app.review_memory_compact_pending = Some(run_state.clone());
    app.mode = AppMode::ReviewMemoryCompactRunning(run_state);

    tx.send(crate::app::pr_review::CompactProgress::Compacting { token_estimate: 99 })
        .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    match &app.mode {
        AppMode::ReviewMemoryCompactRunning(state) => assert_eq!(
            state.stage,
            crate::app::pr_review::CompactStage::Compacting { token_estimate: 99 }
        ),
        other => panic!(
            "expected ReviewMemoryCompactRunning, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    tx.send(crate::app::pr_review::CompactProgress::Done(Ok(Some(
        crate::app::pr_review::CompactOutcome {
            original_findings: 5,
            proposed_findings: 3,
            proposed_content: "# Review memory\n\n## Tests\n- Merged finding\n".to_string(),
            original_content: "# Review memory\n\n## Tests\n- One\n- Two\n".to_string(),
        },
    ))))
    .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    assert!(app.review_memory_compact_bg.is_none());
    match &app.mode {
        AppMode::ReviewMemoryCompactReview(state) => {
            assert_eq!(state.scope, crate::app::review_memory::MemoryScope::Global);
            assert_eq!(state.original_findings, 5);
            assert_eq!(state.proposed_findings, 3);
            assert_eq!(
                state.editor.text(),
                "# Review memory\n\n## Tests\n- Merged finding\n"
            );
        }
        other => panic!(
            "expected ReviewMemoryCompactReview, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn poll_review_memory_compact_bg_nothing_to_compact_returns_to_picker_with_message() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    let run_state = crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Project,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    };
    app.review_memory_compact_pending = Some(run_state.clone());
    app.mode = AppMode::ReviewMemoryCompactRunning(run_state);

    tx.send(crate::app::pr_review::CompactProgress::Done(Ok(None)))
        .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert!(app.message.is_some());
}

#[test]
fn poll_review_memory_compact_bg_error_still_returns_to_picker() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    let run_state = crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Project,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    };
    app.review_memory_compact_pending = Some(run_state.clone());
    app.mode = AppMode::ReviewMemoryCompactRunning(run_state);

    tx.send(crate::app::pr_review::CompactProgress::Done(Err(
        anyhow::anyhow!("claude headless command failed"),
    )))
    .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    match &app.mode {
        AppMode::PrPicker(state) => {
            assert!(
                state
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("claude headless command failed"))
            );
        }
        other => panic!("expected PrPicker, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.review_memory_compact_bg.is_none());
}

#[test]
fn cancel_review_memory_compact_does_not_reopen_review_dialog_over_the_user() {
    // Regression guard for the analogous AI-review bug (`ai_review_pending`):
    // a late `Done` after the user already backed out (`esc`) must not force
    // them into a full-screen dialog they didn't ask to see, and must not
    // panic when `self.mode` is no longer `ReviewMemoryCompactRunning`.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    let run_state = crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Project,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    };
    app.review_memory_compact_pending = Some(run_state.clone());
    app.mode = AppMode::ReviewMemoryCompactRunning(run_state);

    app.cancel_review_memory_compact();
    assert!(matches!(app.mode, AppMode::PrPicker(_)));

    tx.send(crate::app::pr_review::CompactProgress::Done(Ok(Some(
        crate::app::pr_review::CompactOutcome {
            original_findings: 2,
            proposed_findings: 1,
            proposed_content: "# Review memory\n".to_string(),
            original_content: "# Review memory\n\n## Tests\n- One\n- Two\n".to_string(),
        },
    ))))
    .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    assert!(app.review_memory_compact_bg.is_none());
    assert!(app.review_memory_compact_pending.is_none());
    // Stays on the picker the user cancelled back to — not yanked into the
    // review dialog, and not silently dropped either (a toast explains it).
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert!(
        app.toasts
            .last()
            .is_some_and(|t| t.message.contains("navigated away"))
    );
}

/// Enter the compact review dialog with `content` proposed for `path`, taking
/// the conflict baseline from whatever is on disk at `path` right now — the
/// same snapshot the background pass would have read.
fn enter_compact_review_for_test(app: &mut App, path: std::path::PathBuf, content: &str) {
    let original_content = std::fs::read_to_string(&path).unwrap_or_default();
    let origin = crate::app::PrPickerState {
        workdir: std::path::PathBuf::from("/tmp/test-workdir"),
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        compact_confirm: None,
        current_user: None,
    };
    app.mode = AppMode::ReviewMemoryCompactReview(crate::app::CompactReviewState {
        origin,
        path,
        scope: crate::app::review_memory::MemoryScope::Project,
        original_findings: 3,
        proposed_findings: 2,
        editor: crate::editor::TextEditor::new(content.to_string()),
        original_content,
        overwrite_confirmed: false,
        editing: false,
        scroll: 0,
        sync_to_cursor: false,
        error: None,
    });
}

#[test]
fn pr_review_compact_write_overwrites_file_and_returns_to_picker() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    std::fs::write(
        &path,
        "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n",
    )
    .unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(
        &mut app,
        path.clone(),
        "# Review memory\n\n## Tests\n- Merged finding\n",
    );

    app.pr_review_compact_write().unwrap();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "# Review memory\n\n## Tests\n- Merged finding\n");
    assert!(
        app.toasts
            .last()
            .is_some_and(|t| t.message.contains("3") && t.message.contains("2"))
    );
}

#[test]
fn pr_review_compact_write_keeps_findings_appended_while_the_dialog_was_open() {
    // The compact proposal rewrites a snapshot taken before the agent pass. Any
    // AMF session can append to the same doc in that window (every session on
    // the machine shares the global one), so those findings have to survive the
    // rewrite instead of being clobbered by a stale snapshot.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    std::fs::write(
        &path,
        "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n",
    )
    .unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(
        &mut app,
        path.clone(),
        "# Review memory\n\n## Tests\n- Merged finding\n",
    );

    // Another session appends after the snapshot was taken.
    crate::app::review_memory::append_finding(
        &path,
        crate::app::review_memory::MemoryScope::Project,
        "Concurrency",
        "Guard the shared doc",
    )
    .unwrap();

    app.pr_review_compact_write().unwrap();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("- Merged finding"));
    assert!(contents.contains("- Guard the shared doc"));
    assert!(
        app.toasts
            .last()
            .is_some_and(|t| t.message.contains("kept 1 finding added elsewhere"))
    );
}

#[test]
fn pr_review_compact_write_refuses_once_when_the_doc_diverged_then_overwrites() {
    // A change no append can explain (prose hand-edited, findings deleted)
    // can't be replayed onto the rewrite, so the first confirm reports it
    // inline and writes nothing; confirming again is a deliberate overwrite.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    std::fs::write(
        &path,
        "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n",
    )
    .unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(
        &mut app,
        path.clone(),
        "# Review memory\n\n## Tests\n- Merged finding\n",
    );

    let hand_edited = "# Review memory\n\nHand-written note.\n\n## Tests\n- Stale one\n";
    std::fs::write(&path, hand_edited).unwrap();

    app.pr_review_compact_write().unwrap();

    match &app.mode {
        AppMode::ReviewMemoryCompactReview(state) => {
            assert!(
                state
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("changed on disk"))
            );
            assert!(state.overwrite_confirmed);
        }
        other => panic!(
            "expected the dialog to stay open, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), hand_edited);

    app.pr_review_compact_write().unwrap();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# Review memory\n\n## Tests\n- Merged finding\n"
    );
}

#[test]
fn pr_review_compact_discard_leaves_file_untouched() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    let original = "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n";
    std::fs::write(&path, original).unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(&mut app, path.clone(), "# Review memory\n\n- edited away\n");

    app.pr_review_compact_discard();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents, original);
}

#[test]
fn pr_review_fix_session_usage_reads_the_target_sessions_tokens() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    session.token_usage = Some(SessionTokenUsage {
        source: TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "s1".to_string(),
        },
        input_tokens: 1000,
        output_tokens: 500,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1500,
    });

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    let usage = app.pr_review_fix_session_usage();
    assert_eq!(usage.map(|u| u.total_tokens), Some(1500));
}

#[test]
fn pr_review_fix_session_usage_none_before_session_exists() {
    // Before the first `f`, the dedicated review session doesn't exist yet —
    // the header shouldn't show a stale or fabricated number.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert!(app.pr_review_fix_session_usage().is_none());
}

#[test]
fn pr_review_dedicated_session_status_is_scoped_to_its_session() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let other_session_id = store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Working session".to_string())
        .id
        .clone();
    let dedicated_session_id = store.projects[0].features[0]
        .add_session_named(
            SessionKind::Claude,
            crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
        )
        .id
        .clone();
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), Some(false));

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": tmux_session,
        "amf_feature_session_id": other_session_id,
    }));
    assert_eq!(
        app.pr_review_dedicated_session_working(),
        Some(false),
        "another agent window in the feature must not light the badge"
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));

    // Either activity signal independently keeps the exact session working.
    app.handle_ipc_message_value(serde_json::json!({
        "type": "tool-start",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-stop",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));

    app.handle_ipc_message_value(serde_json::json!({
        "type": "tool-stop",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(app.pr_review_dedicated_session_working(), Some(false));
}

#[test]
fn pr_review_dedicated_opencode_session_uses_sidebar_activity() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Opencode,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    session.set_token_usage_source_exact(TokenUsageSource {
        provider: TokenUsageProvider::Opencode,
        id: "opencode-triage".to_string(),
    });
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.opencode_sidebar_cache.insert(
        tmux_session,
        crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "opencode-triage".to_string(),
            status: Some("busy".to_string()),
            ..Default::default()
        },
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));
}

#[test]
fn pr_review_schedules_sidebar_load_for_dedicated_opencode_session() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Opencode,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    session.set_token_usage_source_exact(TokenUsageSource {
        provider: TokenUsageProvider::Opencode,
        id: "opencode-triage".to_string(),
    });
    let si = store.projects[0].features[0].sessions.len() - 1;
    let expected_signature = SidebarLoadRequest::from_opencode_session(
        &store.projects[0].features[0],
        &store.projects[0].features[0].sessions[si],
    )
    .signature();
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    app.schedule_sidebar_load_for_feature(0, 0);
    for _ in 0..20 {
        app.poll_sidebar_load_results();
        if !app.pending_sidebar_loads.contains(&tmux_session) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert_eq!(
        app.sidebar_load_signatures.get(&tmux_session),
        Some(&expected_signature)
    );
}

#[test]
fn pr_review_dedicated_pi_session_uses_thinking_marker() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(
        SessionKind::Pi,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    let tmux_session = format!("amf-pi-triage-test-{}", uuid::Uuid::new_v4());
    store.projects[0].features[0].tmux_session = tmux_session.clone();
    let marker = PathBuf::from("/tmp/amf-thinking").join(&tmux_session);
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, "").unwrap();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));

    std::fs::remove_file(marker).unwrap();
}

#[test]
fn pr_review_dedicated_session_status_is_absent_before_creation() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), None);
}

#[test]
fn pr_review_triage_session_usage_reports_only_growth_since_baseline() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    let source = TokenUsageSource {
        provider: TokenUsageProvider::Claude,
        id: "triage-session".to_string(),
    };
    session.token_usage = Some(SessionTokenUsage {
        source: source.clone(),
        input_tokens: 1400,
        output_tokens: 300,
        cache_read_tokens: 200,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1900,
    });

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);
    let AppMode::PrReview(state) = &mut app.mode else {
        panic!("expected PR review pane");
    };
    state.usage_baselines.insert(
        source.clone(),
        SessionTokenUsage {
            source,
            input_tokens: 1000,
            output_tokens: 200,
            cache_read_tokens: 100,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            total_tokens: 1300,
        },
    );

    let usage = app.pr_review_triage_session_usage().unwrap();
    assert_eq!(usage.input_tokens, 400);
    assert_eq!(usage.output_tokens, 100);
    assert_eq!(usage.cache_read_tokens, 100);
    assert_eq!(usage.total_tokens, 600);
}

#[test]
fn pr_review_triage_session_usage_hides_an_unchanged_baseline() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    let usage = SessionTokenUsage {
        source: TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "unchanged".to_string(),
        },
        input_tokens: 1000,
        output_tokens: 200,
        cache_read_tokens: 100,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1300,
    };
    session.token_usage = Some(usage.clone());

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);
    let AppMode::PrReview(state) = &mut app.mode else {
        panic!("expected PR review pane");
    };
    state.usage_baselines.insert(usage.source.clone(), usage);

    assert!(app.pr_review_triage_session_usage().is_none());
}

#[test]
fn pr_review_switching_to_live_target_ignores_its_earlier_usage() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let usage = SessionTokenUsage {
        source: TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "live-session".to_string(),
        },
        input_tokens: 800,
        output_tokens: 200,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1000,
    };
    store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Working".to_string())
        .token_usage = Some(usage.clone());

    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);

    // Pick "existing live session" from the fix-target picker (replaces the
    // old standalone `t` toggle).
    app.pr_review_open_fix_confirm();
    if let AppMode::PrReview(state) = &mut app.mode {
        let pick = state
            .harness_pick
            .as_mut()
            .expect("fix-target picker should be open");
        pick.selected = pick
            .rows
            .iter()
            .position(|r| matches!(r, crate::app::pr_review::FixTargetPickRow::ExistingLive(_)))
            .expect("existing-live row should be present");
    }
    app.pr_review_harness_pick_confirm();

    assert!(app.pr_review_triage_session_usage().is_none());

    let session = &mut app.store.projects[0].features[0].sessions[0];
    session.token_usage.as_mut().unwrap().input_tokens += 250;
    session.token_usage.as_mut().unwrap().total_tokens += 250;
    let delta = app.pr_review_triage_session_usage().unwrap();
    assert_eq!(delta.input_tokens, 250);
    assert_eq!(delta.total_tokens, 250);
}

#[test]
fn pr_review_first_fix_opens_harness_picker_then_confirm() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);

    // First `f`: no dedicated session and no harness chosen yet → the picker
    // opens; the fix confirm has NOT opened yet.
    app.pr_review_open_fix_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(
                state.harness_pick.is_some(),
                "harness picker should be open"
            );
            assert!(state.fix_confirm.is_none(), "fix confirm should wait");
            assert!(state.review_harness.is_none());
            // First row is always "existing live session"; default highlight
            // is the dedicated row for the project's preferred agent.
            let pick = state.harness_pick.as_ref().unwrap();
            assert_eq!(
                pick.rows[0],
                crate::app::pr_review::FixTargetPickRow::ExistingLive(None)
            );
            assert_eq!(
                pick.rows[pick.selected],
                crate::app::pr_review::FixTargetPickRow::Dedicated(AgentKind::default())
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    // Choosing the harness remembers it and continues into the fix confirm.
    app.pr_review_harness_pick_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.harness_pick.is_none());
            assert_eq!(state.review_harness, Some(AgentKind::default()));
            assert!(
                state.fix_confirm.is_some(),
                "fix confirm should now be open"
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_second_fix_skips_harness_picker() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);

    // Harness already chosen for this PR: `f` goes straight to the fix confirm.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review_harness = Some(AgentKind::Codex);
    }
    app.pr_review_open_fix_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(
                state.harness_pick.is_none(),
                "no picker on subsequent fixes"
            );
            assert!(state.fix_confirm.is_some());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_toggle_mark_adds_and_removes() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);

    // Mark the first comment, move down, mark the second.
    app.pr_review_toggle_mark();
    app.pr_review_select_next();
    app.pr_review_toggle_mark();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.marked.len(), 2);
            assert!(state.marked.contains(&1));
            assert!(state.marked.contains(&2));
        }
        _ => unreachable!(),
    }

    // Toggling the same comment again unmarks it.
    app.pr_review_toggle_mark();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.marked.len(), 1);
            assert!(!state.marked.contains(&2));
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_with_nothing_marked_hints() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);

    app.pr_review_open_batch_confirm();
    assert!(
        app.message.as_deref().unwrap_or("").contains("press space"),
        "expected a hint to mark comments, got {:?}",
        app.message
    );
    // No dialog opened.
    match &app.mode {
        AppMode::PrReview(state) => assert!(state.fix_confirm.is_none()),
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_opens_combined_dialog_for_marked() {
    // `pr_review_test_app` has no feature at /tmp/wd, so no harness pick is
    // needed — `B` builds the combined dialog directly.
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.marked.insert(1);
        state.marked.insert(3);
    }

    app.pr_review_open_batch_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("batch dialog should open");
            // The dialog carries both marked ids (list order).
            let mut ids = confirm.batch.clone().expect("dialog is a batch");
            ids.sort();
            assert_eq!(ids, vec![1, 3]);
            // One combined prompt, numbered, covering both files, unmarked one absent.
            let text = confirm.editor.text();
            assert!(text.starts_with("Address these PR review comments."));
            assert!(text.contains("Comment 1:") && text.contains("Comment 2:"));
            assert!(text.contains("File: src/file1.rs:1"));
            assert!(text.contains("File: src/file3.rs:3"));
            assert!(!text.contains("File: src/file2.rs"));
            assert_eq!(confirm.reply_draft_requests.len(), 2);
            assert_eq!(
                confirm
                    .reply_draft_requests
                    .iter()
                    .map(|request| request.comment_id)
                    .collect::<Vec<_>>(),
                vec![1, 3]
            );
            assert!(
                confirm
                    .reply_draft_requests
                    .iter()
                    .all(|request| request.base_head_sha == "sha")
            );
            assert!(text.contains("--comment-id 1 --request-id"));
            assert!(text.contains("--comment-id 3 --request-id"));
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_excludes_resolved_marks() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        // Mark both, but one is already resolved on GitHub.
        state.marked.insert(1);
        state.marked.insert(2);
        state.review.comments[0].is_resolved = true;
    }

    app.pr_review_open_batch_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("batch dialog should open");
            // Only the unresolved comment is included (token principle #6).
            assert_eq!(confirm.batch.clone().unwrap(), vec![2]);
            assert!(!confirm.editor.text().contains("File: src/file1.rs"));
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_all_resolved_hints() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.marked.insert(1);
        state.review.comments[0].is_resolved = true;
    }

    app.pr_review_open_batch_confirm();
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("all resolved"),
        "expected an all-resolved hint, got {:?}",
        app.message
    );
    match &app.mode {
        AppMode::PrReview(state) => assert!(state.fix_confirm.is_none()),
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_routes_through_harness_pick_then_opens_combined() {
    // A dedicated-review PR with no session yet picks the harness before the
    // first fix — the batch flow must route the picker's continuation back to
    // the combined dialog (not the single-comment one).
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.marked.insert(1);
        state.marked.insert(2);
    }

    // `B`: picker opens, no dialog yet, and the pending action is the batch.
    app.pr_review_open_batch_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(
                state.harness_pick.is_some(),
                "harness picker should be open"
            );
            assert!(state.fix_confirm.is_none());
            assert!(state.pending_batch, "pending action should be the batch");
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    // Choosing the harness opens the *combined* dialog and clears the flag.
    app.pr_review_harness_pick_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(!state.pending_batch);
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("batch dialog should open");
            let mut ids = confirm.batch.clone().expect("dialog is a batch");
            ids.sort();
            assert_eq!(ids, vec![1, 2]);
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_cancel_harness_picker_aborts_fix() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_open_fix_confirm();
    assert!(app.pr_review_harness_picking());

    // Cancel: picker closes, nothing injected, harness stays unset so the next
    // `f` asks again.
    app.pr_review_harness_pick_cancel();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.harness_pick.is_none());
            assert!(state.fix_confirm.is_none());
            assert!(state.review_harness.is_none());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_i_is_noop_for_comment_without_file_path() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    // Conversation/summary comments carry no path — nothing to highlight.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.comments[0].path = None;
    }

    app.open_syntax_language_picker_for_selected_diff_file();

    assert!(
        matches!(app.mode, AppMode::PrReview(_)),
        "expected to stay in the review pane, got {:?}",
        std::mem::discriminant(&app.mode)
    );
}

/// Enter the review pane with comments built from `(id, path, author, is_bot)`
/// tuples, in that fetch order, for exercising `sort_mode`.
fn enter_pr_review_with_authors(app: &mut App, entries: &[(u64, &str, &str, bool)]) {
    let comments: Vec<crate::github::ReviewComment> = entries
        .iter()
        .map(|&(id, path, author, is_bot)| crate::github::ReviewComment {
            id,
            path: Some(path.to_string()),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("comment {id}"),
            user: crate::github::GhUser {
                login: author.to_string(),
                kind: if is_bot {
                    "Bot".to_string()
                } else {
                    "User".to_string()
                },
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let review = crate::app::pr_review::normalize(pr, comments, vec![], vec![], vec![]);
    app.mode = AppMode::PrReview(PrReviewState {
        workdir: std::path::PathBuf::from("/tmp/wd"),
        review,
        selected: 0,
        detail_scroll: 0,
        detail_content_lines: 0,
        hide_resolved: false,
        sort_mode: crate::app::pr_review::PrSortMode::default(),
        fix_target: crate::app::pr_review::FixTarget::default(),
        fix_target_picked: false,
        usage_baselines: std::collections::HashMap::new(),
        review_harness: None,
        harness_pick: None,
        new_feature_setup: None,
        integrate: None,
        fix_confirm: None,
        fix_vim_enabled: false,
        mark_pick: None,
        reply_kind_pick: None,
        reply: None,
        memory_add: None,
        marked: std::collections::HashSet::new(),
        pending_batch: false,
        checked_out_branch: Some("main".to_string()),
        pending_ai_review_findings: 0,
    });
}

/// Enter the review pane with a mix of inline (code-anchored) comments and
/// top-level conversation comments, in fetch order: every id in `inline_ids`
/// first, then every id in `conversation_ids` — matching how `normalize`
/// orders review comments ahead of issue comments. For exercising
/// `PrSortMode::Conversations`.
fn enter_pr_review_with_conversation(app: &mut App, inline_ids: &[u64], conversation_ids: &[u64]) {
    let review_comments: Vec<crate::github::ReviewComment> = inline_ids
        .iter()
        .map(|&id| crate::github::ReviewComment {
            id,
            path: Some(format!("f{id}.rs")),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("inline {id}"),
            user: crate::github::GhUser {
                login: "alice".into(),
                kind: "User".into(),
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let issue_comments: Vec<crate::github::IssueComment> = conversation_ids
        .iter()
        .map(|&id| crate::github::IssueComment {
            id,
            body: format!("conversation {id}"),
            user: crate::github::GhUser {
                login: "bob".into(),
                kind: "User".into(),
            },
        })
        .collect();
    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let review =
        crate::app::pr_review::normalize(pr, review_comments, vec![], issue_comments, vec![]);
    app.mode = AppMode::PrReview(PrReviewState {
        workdir: std::path::PathBuf::from("/tmp/wd"),
        review,
        selected: 0,
        detail_scroll: 0,
        detail_content_lines: 0,
        hide_resolved: false,
        sort_mode: crate::app::pr_review::PrSortMode::default(),
        fix_target: crate::app::pr_review::FixTarget::default(),
        fix_target_picked: false,
        usage_baselines: std::collections::HashMap::new(),
        review_harness: None,
        harness_pick: None,
        new_feature_setup: None,
        integrate: None,
        fix_confirm: None,
        fix_vim_enabled: false,
        mark_pick: None,
        reply_kind_pick: None,
        reply: None,
        memory_add: None,
        marked: std::collections::HashSet::new(),
        pending_batch: false,
        checked_out_branch: Some("main".to_string()),
        pending_ai_review_findings: 0,
    });
}

fn pr_review_ids_in_visible_order(app: &App) -> Vec<u64> {
    match &app.mode {
        AppMode::PrReview(state) => state
            .visible_indices()
            .iter()
            .map(|&i| state.review.comments[i].id)
            .collect(),
        _ => panic!("not in PrReview mode"),
    }
}

fn install_amf_outbound_comment_mix(app: &mut App) {
    use crate::app::pr_review::{
        AI_REVIEW_ATTRIBUTION_FOOTER, AMF_ATTRIBUTION_FOOTER, CommentKind,
    };

    enter_pr_review_with_conversation(app, &[], &[]);
    if let AppMode::PrReview(state) = &mut app.mode {
        let mut root = pr_comment_of_kind(1, CommentKind::Inline);
        root.author = "reviewer".into();

        let mut amf_reply = pr_comment_of_kind(2, CommentKind::Inline);
        amf_reply.author = "author".into();
        amf_reply.in_reply_to = Some(1);
        amf_reply.body = format!("Done in `abc123`.\n\n{AMF_ATTRIBUTION_FOOTER}");

        let mut ai_finding = pr_comment_of_kind(3, CommentKind::Inline);
        ai_finding.author = "author".into();
        ai_finding.body = format!("AI finding.\n\n{AI_REVIEW_ATTRIBUTION_FOOTER}");

        let mut human_reply = pr_comment_of_kind(4, CommentKind::Inline);
        human_reply.author = "second-reviewer".into();
        human_reply.in_reply_to = Some(1);
        human_reply.body = "This still needs a regression test.".into();

        let mut orphaned_amf_reply = pr_comment_of_kind(5, CommentKind::Inline);
        orphaned_amf_reply.author = "author".into();
        orphaned_amf_reply.in_reply_to = Some(999);
        orphaned_amf_reply.body = format!("Done elsewhere.\n\n{AMF_ATTRIBUTION_FOOTER}");

        state.review.comments = vec![root, amf_reply, ai_finding, human_reply, orphaned_amf_reply];
    }
}

#[test]
fn pr_review_collates_amf_followup_but_keeps_standalone_findings_actionable() {
    let mut app = pr_review_test_app();
    install_amf_outbound_comment_mix(&mut app);

    // The attributed inline reply is represented under comment 1's Replies
    // section, not as a duplicate row. A top-level AI finding stays fully
    // actionable; an orphaned follow-up stays visible for context; and the
    // unrelated human reply remains actionable.
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![1, 3, 4, 5]);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("not in PrReview mode");
    };
    assert_eq!(state.review.open_count(), 3);
    assert!(state.review.comments[3].is_actionable());
    assert!(state.review.comments[2].is_actionable());
    assert!(!state.review.comments[4].is_actionable());
}

#[test]
fn pr_review_fixes_standalone_amf_findings_but_not_followup_replies() {
    let mut app = pr_review_test_app();
    install_amf_outbound_comment_mix(&mut app);

    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 2; // top-level AI Review finding
    }
    app.pr_review_open_fix_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("standalone AI Review finding is fixable");
            assert!(confirm.editor.text().contains("AI finding."));
        }
        _ => panic!("not in PrReview mode"),
    }

    // Even a stale mark set assembled before refresh filters the orphaned
    // AMF follow-up out of the combined prompt, but retains the finding.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_confirm = None;
        state.marked.extend([3, 4, 5]);
    }
    app.pr_review_open_batch_confirm();
    let AppMode::PrReview(state) = &app.mode else {
        panic!("not in PrReview mode");
    };
    let confirm = state
        .fix_confirm
        .as_ref()
        .expect("standalone finding and human reply are batchable");
    assert_eq!(confirm.batch.as_deref(), Some(&[3, 4][..]));
    assert!(
        confirm
            .editor
            .text()
            .contains("This still needs a regression test.")
    );
    assert!(confirm.editor.text().contains("AI finding."));
    assert!(!confirm.editor.text().contains("Done elsewhere."));
}

#[test]
fn pr_review_cycle_sort_wraps_through_all_modes() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);

    use crate::app::pr_review::PrSortMode;
    let sort_mode = |app: &App| match &app.mode {
        AppMode::PrReview(state) => state.sort_mode,
        _ => panic!("not in PrReview mode"),
    };

    assert_eq!(sort_mode(&app), PrSortMode::FetchOrder);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::ByFile);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::ByAuthor);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::HumansFirst);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::Conversations);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::FetchOrder);
}

#[test]
fn pr_review_sort_by_file_groups_and_orders_paths() {
    let mut app = pr_review_test_app();
    // Fetch order: 1 (z.rs), 2 (a.rs), 3 (m.rs).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "z.rs", "alice", false),
            (2, "a.rs", "alice", false),
            (3, "m.rs", "alice", false),
        ],
    );
    app.pr_review_cycle_sort(); // FetchOrder -> ByFile
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 3, 1]);
}

#[test]
fn pr_review_sort_by_author_is_stable_within_ties() {
    let mut app = pr_review_test_app();
    // Fetch order: 1 (bob), 2 (alice), 3 (alice).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "a.rs", "bob", false),
            (2, "b.rs", "alice", false),
            (3, "c.rs", "alice", false),
        ],
    );
    app.pr_review_cycle_sort(); // FetchOrder -> ByFile
    app.pr_review_cycle_sort(); // ByFile -> ByAuthor
    // alice's two comments keep their fetch-order relative to each other.
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 3, 1]);
}

#[test]
fn pr_review_sort_humans_first_keeps_bots_last() {
    let mut app = pr_review_test_app();
    // Fetch order: 1 (bot), 2 (human), 3 (bot), 4 (human).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "a.rs", "coderabbit", true),
            (2, "b.rs", "alice", false),
            (3, "c.rs", "copilot", true),
            (4, "d.rs", "bob", false),
        ],
    );
    for _ in 0..3 {
        app.pr_review_cycle_sort(); // FetchOrder -> ByFile -> ByAuthor -> HumansFirst
    }
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 4, 1, 3]);
}

#[test]
fn pr_review_sort_conversations_groups_them_after_code_anchored_comments() {
    let mut app = pr_review_test_app();
    // Fetch order: inline 1, 2 then conversation 3, 4 — already matches the
    // desired grouping, so this proves the mode is a no-op here, not just
    // coincidentally correct because fetch order already separates them
    // (the interleaved case below is the real test).
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[3, 4]);
    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> ByFile -> ByAuthor -> HumansFirst -> Conversations
    }
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![1, 2, 3, 4]);
}

fn pr_comment_of_kind(
    id: u64,
    kind: crate::app::pr_review::CommentKind,
) -> crate::app::pr_review::PrComment {
    crate::app::pr_review::PrComment {
        id,
        kind,
        author: "someone".into(),
        is_bot: false,
        path: None,
        line: None,
        side: None,
        outdated: false,
        file_level: false,
        diff_hunk: None,
        body: format!("comment {id}"),
        snippet: format!("comment {id}"),
        in_reply_to: None,
        thread_id: None,
        is_resolved: false,
        triage: crate::app::pr_review::TriageState::Untriaged,
        local_note: None,
        github_id: None,
        github_review_id: None,
    }
}

#[test]
fn pr_review_sort_conversations_reorders_interleaved_comments() {
    use crate::app::pr_review::CommentKind;

    let mut app = pr_review_test_app();
    enter_pr_review_with_conversation(&mut app, &[], &[]);
    if let AppMode::PrReview(state) = &mut app.mode {
        // Fetch order: 1 (inline), 3 (conversation), 2 (inline), 4 (conversation).
        state.review.comments = vec![
            pr_comment_of_kind(1, CommentKind::Inline),
            pr_comment_of_kind(3, CommentKind::Conversation),
            pr_comment_of_kind(2, CommentKind::Inline),
            pr_comment_of_kind(4, CommentKind::Conversation),
        ];
    }

    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> ByFile -> ByAuthor -> HumansFirst -> Conversations
    }
    // Every conversation comment moves after every inline one; relative order
    // within each group is preserved (stable sort).
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![1, 2, 3, 4]);
}

#[test]
fn pr_review_conversation_section_start_marks_where_the_group_begins() {
    let mut app = pr_review_test_app();
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[3, 4]);
    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> Conversations
    }
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PrReview");
    };
    assert_eq!(state.conversation_section_start(), Some(2));
}

#[test]
fn pr_review_conversation_section_start_is_none_outside_conversations_mode() {
    let mut app = pr_review_test_app();
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[3, 4]);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PrReview");
    };
    assert_eq!(state.conversation_section_start(), None);
}

#[test]
fn pr_review_conversation_section_start_is_none_without_both_groups() {
    let mut app = pr_review_test_app();
    // All inline, no conversation comments — nothing to separate.
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[]);
    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> Conversations
    }
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PrReview");
    };
    assert_eq!(state.conversation_section_start(), None);
}

#[test]
fn pr_review_toggle_resolved_snaps_using_current_sort_order() {
    let mut app = pr_review_test_app();
    // Fetch order 1,2,3 by id; sorted by file: 2 (a.rs), 3 (m.rs), 1 (z.rs).
    // Resolve comment 3 (the middle one in file-sort order).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "z.rs", "alice", false),
            (2, "a.rs", "alice", false),
            (3, "m.rs", "alice", false),
        ],
    );
    if let AppMode::PrReview(state) = &mut app.mode {
        // Mark comment 3 resolved directly on the model.
        state
            .review
            .comments
            .iter_mut()
            .find(|c| c.id == 3)
            .unwrap()
            .is_resolved = true;
    }
    app.pr_review_cycle_sort(); // FetchOrder -> ByFile: order is [2, 3, 1]
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = state
            .review
            .comments
            .iter()
            .position(|c| c.id == 3)
            .unwrap();
    }

    app.pr_review_toggle_resolved();
    // Comment 3 is hidden; the visible file-sort order is now [2, 1].
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 1]);
    match &app.mode {
        AppMode::PrReview(state) => {
            let selected_id = state.review.comments[state.selected].id;
            assert_eq!(selected_id, 1);
        }
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn pr_review_open_without_selection_shows_message() {
    // No projects/features selected, so opening should not enter a PR mode.
    let mut app = pr_review_test_app();
    app.open_pr_review();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.message.is_some());
}

fn view_state_for(project_name: &str, feature_name: &str) -> ViewState {
    ViewState::new(
        project_name.to_string(),
        feature_name.to_string(),
        "amf-my-feat".to_string(),
        "claude".to_string(),
        "Claude".to_string(),
        SessionKind::Claude,
        VibeMode::default(),
        false,
    )
}

#[test]
fn feature_for_view_resolves_by_project_and_feature_name() {
    let store = store_with_feature(ProjectStatus::Active);
    let app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let view = view_state_for("my-project", "my-feat");

    let feature = app.feature_for_view(&view).expect("feature should resolve");
    assert_eq!(feature.id, "feat-1");
}

#[test]
fn feature_for_view_returns_none_for_unknown_feature() {
    let store = store_with_feature(ProjectStatus::Active);
    let app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let view = view_state_for("my-project", "does-not-exist");

    assert!(app.feature_for_view(&view).is_none());
}

#[test]
fn open_pr_review_from_view_is_noop_outside_viewing_mode() {
    // Leader commands only fire while `Viewing`, but guard the entry point
    // itself in case something else ever calls it from another mode.
    let mut app = pr_review_test_app();
    app.mode = AppMode::Normal;

    app.open_pr_review_from_view();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.message.is_none());
}

#[test]
fn open_pr_review_from_view_shows_message_for_unresolvable_feature() {
    // The session view names a project/feature that no longer exists in the
    // store — resolve must fail gracefully rather than panicking or spending
    // a `gh` call on a bogus workdir.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(view_state_for("my-project", "does-not-exist"));

    app.open_pr_review_from_view();

    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert_eq!(app.message.as_deref(), Some("No active feature to review"));
}

#[test]
fn dedicated_review_session_working_for_workdir_reads_outside_pr_review_mode() {
    // The ambient Viewing-mode badge calls this while `self.mode` is
    // `Viewing`, not `PrReview` — unlike `pr_review_dedicated_session_working`,
    // it must not depend on the pane being open.
    let mut store = store_with_feature(ProjectStatus::Active);
    let dedicated_session_id = store.projects[0].features[0]
        .add_session_named(
            SessionKind::Claude,
            crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
        )
        .id
        .clone();
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let workdir = store.projects[0].features[0].workdir.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(view_state_for("my-project", "my-feat"));

    assert_eq!(
        app.dedicated_review_session_working_for_workdir(&workdir),
        Some(false)
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(
        app.dedicated_review_session_working_for_workdir(&workdir),
        Some(true)
    );
}

#[test]
fn ai_review_running_for_workdir_matches_the_pending_reviews_workdir() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let workdir = origin.workdir.clone();
    let other_workdir = PathBuf::from("/tmp/other-workdir");

    // No background job yet.
    assert!(!app.ai_review_running_for_workdir(&workdir));

    let (_tx, rx) = std::sync::mpsc::channel();
    app.ai_review_bg = Some(rx);
    app.ai_review_pending = Some(origin);

    assert!(app.ai_review_running_for_workdir(&workdir));
    assert!(!app.ai_review_running_for_workdir(&other_workdir));
}

/// Enter the review pane with comments 1..=n where the given ids are resolved.
fn enter_pr_review_with_resolved(app: &mut App, n: u64, resolved: &[u64]) {
    let comments: Vec<crate::github::ReviewComment> = (1..=n)
        .map(|id| crate::github::ReviewComment {
            id,
            path: Some(format!("src/file{id}.rs")),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("comment {id}"),
            user: crate::github::GhUser {
                login: "alice".to_string(),
                kind: "User".to_string(),
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let threads: Vec<crate::github::ReviewThread> = resolved
        .iter()
        .map(|&id| crate::github::ReviewThread {
            id: format!("T{id}"),
            is_resolved: true,
            comment_ids: vec![id],
        })
        .collect();
    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let review = crate::app::pr_review::normalize(pr, comments, vec![], vec![], threads);
    app.mode = AppMode::PrReview(PrReviewState {
        workdir: std::path::PathBuf::from("/tmp/wd"),
        review,
        selected: 0,
        detail_scroll: 0,
        detail_content_lines: 0,
        hide_resolved: false,
        sort_mode: crate::app::pr_review::PrSortMode::default(),
        fix_target: crate::app::pr_review::FixTarget::default(),
        fix_target_picked: false,
        usage_baselines: std::collections::HashMap::new(),
        review_harness: None,
        harness_pick: None,
        new_feature_setup: None,
        integrate: None,
        fix_confirm: None,
        fix_vim_enabled: false,
        mark_pick: None,
        reply_kind_pick: None,
        reply: None,
        memory_add: None,
        marked: std::collections::HashSet::new(),
        pending_batch: false,
        checked_out_branch: Some("main".to_string()),
        pending_ai_review_findings: 0,
    });
}

#[test]
fn pr_review_hide_resolved_skips_resolved_comments() {
    // Comments 1,2,3; comment 2 is resolved on GitHub.
    let mut app = pr_review_test_app();
    enter_pr_review_with_resolved(&mut app, 3, &[2]);

    app.pr_review_toggle_resolved();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.hide_resolved);
            assert_eq!(state.visible_indices(), vec![0, 2]);
            assert_eq!(state.hidden_resolved_count(), 1);
        }
        _ => panic!("not in PrReview mode"),
    }

    // Navigation jumps over the hidden (resolved) comment at index 1.
    assert_eq!(pr_review_selected(&app), 0);
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 2);
    app.pr_review_select_prev();
    assert_eq!(pr_review_selected(&app), 0);
}

#[test]
fn pr_review_toggle_snaps_selection_off_hidden_comment() {
    let mut app = pr_review_test_app();
    enter_pr_review_with_resolved(&mut app, 3, &[2]);

    // Park the selection on the resolved comment, then hide resolved.
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 1);
    app.pr_review_toggle_resolved();

    // Index 1 is now hidden, so selection snaps to the next visible comment.
    assert_eq!(pr_review_selected(&app), 2);
}

#[test]
fn pr_review_detail_scroll_clamps() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1); // single short comment

    // Scrolling up at the top is a no-op.
    app.pr_review_scroll_detail_up(5);
    match &app.mode {
        AppMode::PrReview(state) => assert_eq!(state.detail_scroll, 0),
        _ => panic!("not in PrReview mode"),
    }

    // Render once so the detail pane records how many lines it actually drew;
    // the scroll-down clamp bounds against that recorded count.
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    // Scrolling far down clamps to the rendered line count, never past it.
    app.pr_review_scroll_detail_down(1000);
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.detail_content_lines > 0);
            assert_eq!(state.detail_scroll, state.detail_content_lines - 1);
        }
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn pr_review_selection_change_resets_detail_scroll() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);

    app.pr_review_scroll_detail_down(2);
    app.pr_review_select_next();
    match &app.mode {
        AppMode::PrReview(state) => assert_eq!(state.detail_scroll, 0),
        _ => panic!("not in PrReview mode"),
    }
}

fn pr_review_fix_confirm(app: &App) -> &crate::app::FixConfirmState {
    match &app.mode {
        AppMode::PrReview(state) => state
            .fix_confirm
            .as_ref()
            .expect("fix confirm dialog should be open"),
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn pr_review_open_fix_confirm_seeds_editor_from_selection() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);

    assert_eq!(app.pr_review_fix_editing(), None);
    app.pr_review_open_fix_confirm();

    // Dialog opens in confirm (not edit) mode, seeded with the comment's prompt.
    assert_eq!(app.pr_review_fix_editing(), Some(false));
    let confirm = pr_review_fix_confirm(&app);
    let expected = match &app.mode {
        AppMode::PrReview(state) => state.selected_comment().unwrap().fix_prompt(),
        _ => unreachable!(),
    };
    assert!(confirm.editor.text().starts_with(&expected));
    assert!(
        confirm
            .editor
            .text()
            .contains("Address this PR review comment.")
    );
    assert!(
        confirm
            .editor
            .text()
            .contains("Do not post replies to GitHub")
    );
    assert!(
        confirm
            .editor
            .text()
            .contains("amf reply-draft --pr-number 7 --comment-id 1 --request-id")
    );
    assert_eq!(confirm.reply_draft_requests.len(), 1);
    assert_eq!(confirm.reply_draft_requests[0].comment_id, 1);
    assert_eq!(confirm.reply_draft_requests[0].base_head_sha, "sha");
    assert!(
        confirm
            .editor
            .text()
            .contains(&confirm.reply_draft_requests[0].request_id)
    );
}

#[test]
fn pr_review_fix_edit_mode_forwards_keys_and_cancel_closes() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_fix_confirm();

    // Keys are ignored until edit mode is entered.
    assert!(!app.pr_review_fix_editor_key(KeyEvent::from(KeyCode::Char('Z'))));
    let before = pr_review_fix_confirm(&app).editor.text().to_string();

    app.pr_review_fix_edit();
    assert_eq!(app.pr_review_fix_editing(), Some(true));
    assert!(app.pr_review_fix_editor_key(KeyEvent::from(KeyCode::Char('Z'))));
    assert_eq!(
        pr_review_fix_confirm(&app).editor.text(),
        format!("{before}Z")
    );

    // Leaving edit mode keeps the edited text; cancel closes the dialog.
    app.pr_review_fix_stop_edit();
    assert_eq!(app.pr_review_fix_editing(), Some(false));
    app.pr_review_cancel_fix();
    assert_eq!(app.pr_review_fix_editing(), None);
}

#[test]
fn pr_review_fix_vim_toggle_persists_across_reopen() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    // Default keymap is plain.
    app.pr_review_open_fix_confirm();
    assert!(app.pr_review_fix_vim_mode().is_none());

    // Toggling vim flips the editor and the remembered pane preference.
    app.pr_review_fix_toggle_vim();
    assert!(app.pr_review_fix_vim_mode().is_some());
    match &app.mode {
        AppMode::PrReview(state) => assert!(state.fix_vim_enabled),
        _ => unreachable!(),
    }

    // Closing and reopening the dialog (e.g. for another comment) keeps vim on.
    app.pr_review_cancel_fix();
    app.pr_review_open_fix_confirm();
    assert!(
        app.pr_review_fix_vim_mode().is_some(),
        "reopened dialog should remember the vim choice"
    );

    // Toggling back off is likewise remembered.
    app.pr_review_fix_toggle_vim();
    app.pr_review_cancel_fix();
    app.pr_review_open_fix_confirm();
    assert!(app.pr_review_fix_vim_mode().is_none());
}

#[test]
fn pr_review_fix_scroll_moves_offset_and_clears_cursor_follow() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_fix_confirm();

    app.pr_review_fix_scroll(3);
    let confirm = pr_review_fix_confirm(&app);
    assert_eq!(confirm.scroll, 3);
    assert!(
        !confirm.sync_to_cursor,
        "an explicit scroll should stop following the cursor"
    );

    // Scrolling up saturates at zero rather than underflowing.
    app.pr_review_fix_scroll(-10);
    assert_eq!(pr_review_fix_confirm(&app).scroll, 0);
}

fn reply_editor_text(app: &App) -> String {
    match &app.mode {
        AppMode::PrReview(state) => state.reply.as_ref().unwrap().editor.text().to_string(),
        _ => unreachable!(),
    }
}

fn reply_is_agent_drafted(app: &App) -> bool {
    match &app.mode {
        AppMode::PrReview(state) => state.reply.as_ref().unwrap().agent_drafted,
        _ => unreachable!(),
    }
}

fn reply_effective_agent_drafted(app: &App) -> bool {
    match &app.mode {
        AppMode::PrReview(state) => {
            crate::app::pr_review::reply_effective_agent_drafted(state.reply.as_ref().unwrap())
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_open_reply_not_needed_starts_in_edit_mode() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_reply_not_needed();
    // The not-needed reply needs a typed reason, so it opens straight into edit
    // mode with an empty buffer.
    assert_eq!(app.pr_review_reply_view(), Some(true));
    assert_eq!(reply_editor_text(&app), "");
}

#[test]
fn pr_review_open_reply_done_seeds_template_in_confirm_view() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_reply_done();
    // The done reply is post-ready (a template), so it opens in the confirm
    // view. The workdir isn't a git repo in tests, so it falls back to "Done.".
    assert_eq!(app.pr_review_reply_view(), Some(false));
    assert_eq!(reply_editor_text(&app), "Done.");
}

#[test]
fn pr_review_reply_done_prefers_the_agent_draft() {
    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review(&mut app, 1);

    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", "sha")
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Updated the lock scope and added a regression test."
    }));

    app.pr_review_open_reply_done();
    assert_eq!(
        reply_editor_text(&app),
        "Updated the lock scope and added a regression test."
    );
    assert!(reply_is_agent_drafted(&app));
    assert_eq!(app.pr_review_reply_view(), Some(false));
}

#[test]
fn pr_review_reply_not_needed_ignores_a_captured_fix_draft() {
    // A captured draft is the fixing agent's description of what it *did* —
    // not a rationale for why a fix isn't needed. NotNeeded must never seed
    // from it, or a single confirm keystroke would post the fix summary as
    // the not-needed explanation.
    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review(&mut app, 1);

    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", "sha")
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Updated the lock scope and added a regression test."
    }));

    app.pr_review_open_reply_not_needed();
    assert_eq!(reply_editor_text(&app), "");
    assert!(!reply_is_agent_drafted(&app));
    assert_eq!(
        app.pr_review_reply_view(),
        Some(true),
        "not-needed always starts in edit mode so the user types their own reason"
    );
}

#[test]
fn reply_draft_toast_context_uses_the_cached_comments_path_and_snippet() {
    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());

    let review = pr_review_with_comments(1);
    app.db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&review)
        .unwrap();
    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", "sha")
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Fixed it."
    }));

    // A toast fired while the user is elsewhere (dashboard, unrelated tmux
    // view) needs more than bare numbers to mean anything — pull the file
    // path from the review cached at fix-injection time.
    let context = app.reply_draft_toast_context(7, 1).unwrap();
    assert!(context.contains("src/file1.rs"));
}

#[test]
fn reply_draft_toast_context_none_without_a_cached_review() {
    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", "sha")
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Fixed it."
    }));

    // No review was ever cached for PR #7 / head "sha" — falls back to
    // `None` so the caller uses the bare-numbers message instead of panicking
    // or showing a broken context string.
    assert_eq!(app.reply_draft_toast_context(7, 1), None);
}

#[test]
fn pr_review_reply_draft_ipc_ignores_an_expired_request() {
    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "current", "sha")
        .unwrap();

    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "expired",
        "body": "Stale reply"
    }));

    assert_eq!(
        app.db
            .as_ref()
            .unwrap()
            .load_pr_comment_reply_draft(7, 1)
            .unwrap(),
        None
    );
    assert!(app.toasts.is_empty(), "stale drafts are ignored quietly");
}

#[test]
fn pr_review_reply_pick_opens_defaulting_to_done_and_confirm_routes_to_it() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_reply_pick();
    assert!(app.pr_review_reply_pick_picking());

    // Confirming without moving picks the default (first) row, `Done`.
    app.pr_review_reply_pick_confirm();
    assert!(!app.pr_review_reply_pick_picking());
    assert_eq!(app.pr_review_reply_view(), Some(false));
    assert_eq!(reply_editor_text(&app), "Done.");
}

#[test]
fn pr_review_reply_pick_move_and_confirm_routes_to_not_needed() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_reply_pick();
    app.pr_review_reply_pick_move(1);
    app.pr_review_reply_pick_confirm();

    assert!(!app.pr_review_reply_pick_picking());
    // Not-needed opens straight into edit mode with an empty buffer.
    assert_eq!(app.pr_review_reply_view(), Some(true));
    assert_eq!(reply_editor_text(&app), "");
}

#[test]
fn pr_review_reply_pick_cancel_closes_without_opening_a_reply() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_reply_pick();
    app.pr_review_reply_pick_cancel();

    assert!(!app.pr_review_reply_pick_picking());
    assert_eq!(app.pr_review_reply_view(), None);
}

#[test]
fn pr_review_mark_pick_confirm_applies_the_chosen_action() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    // Row 0 is `Done`.
    app.pr_review_open_mark_pick();
    assert!(app.pr_review_mark_pick_picking());
    app.pr_review_mark_pick_confirm();
    assert!(!app.pr_review_mark_pick_picking());
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.selected_comment().unwrap().triage,
                crate::app::pr_review::TriageState::Done
            );
        }
        _ => panic!("expected PrReview"),
    }

    // Row 1 (`Skip`) toggles it back off Done and onto Skipped.
    app.pr_review_open_mark_pick();
    app.pr_review_mark_pick_move(1);
    app.pr_review_mark_pick_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.selected_comment().unwrap().triage,
                crate::app::pr_review::TriageState::Skipped
            );
        }
        _ => panic!("expected PrReview"),
    }
}

#[test]
fn pr_review_mark_pick_cancel_leaves_triage_untouched() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_mark_pick();
    app.pr_review_mark_pick_cancel();

    assert!(!app.pr_review_mark_pick_picking());
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.selected_comment().unwrap().triage,
                crate::app::pr_review::TriageState::Untriaged
            );
        }
        _ => panic!("expected PrReview"),
    }
}

#[test]
fn pr_review_open_reply_done_seeds_the_commit_that_touched_the_comments_line() {
    let repo = TempDir::new().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    let file = repo.path().join("src/file1.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "line1\nline2\nline3\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "first"]);
    std::fs::write(&file, "line1\nFIXED\nline3\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "second"]);
    let fix_sha = git(&["rev-parse", "--short", "HEAD"]);

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo.path().to_path_buf();
        // `pr_review_with_comments` seeds comment 1 at src/file1.rs:1; point it
        // at the line the second commit actually changed.
        state.review.comments[0].line = Some(2);
    }

    app.pr_review_open_reply_done();
    assert_eq!(reply_editor_text(&app), format!("Done in `{fix_sha}`."));
    assert!(!reply_is_agent_drafted(&app));
}

#[test]
fn pr_review_agent_draft_includes_the_post_injection_commit_that_touched_the_file() {
    let repo = TempDir::new().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    let file = repo.path().join("src/file1.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "line1\nreturn value\nline3\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "first"]);
    let base_sha = git(&["rev-parse", "HEAD"]);
    // The fix inserts a guard beside the unchanged commented line. Plain
    // line history would cite the first commit; the recorded injection head
    // makes the later file-touching commit unambiguous.
    std::fs::write(&file, "line1\nguard\nreturn value\nline3\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "second"]);
    let fix_sha = git(&["rev-parse", "--short", "HEAD"]);

    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo.path().to_path_buf();
        state.review.comments[0].line = Some(3);
    }
    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", &base_sha)
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Guarded zero divisors and added regression coverage."
    }));

    app.pr_review_open_reply_done();

    assert_eq!(
        reply_editor_text(&app),
        format!("Guarded zero divisors and added regression coverage.\n\nDone in `{fix_sha}`.")
    );
    assert!(reply_is_agent_drafted(&app));
}

#[test]
fn pr_review_open_reply_done_flags_a_bare_head_fallback() {
    let repo = TempDir::new().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(repo.path().join("README.md"), "hello\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    let head_sha = git(&["rev-parse", "--short", "HEAD"]);

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo.path().to_path_buf();
        // Comment's path was never committed here — no line/file history to
        // find, so the reply falls back to bare HEAD with a caveat.
    }

    app.pr_review_open_reply_done();
    assert_eq!(
        reply_editor_text(&app),
        format!("Done in `{head_sha}` (latest commit).")
    );
}

#[test]
fn pr_review_reply_edit_forwards_keys_and_cancel_closes() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_reply_done();

    // Keys are ignored until edit mode is entered.
    let before = reply_editor_text(&app);
    app.pr_review_reply_editor_key(KeyEvent::from(KeyCode::Char('Z')));
    assert_eq!(reply_editor_text(&app), before);

    app.pr_review_reply_edit();
    assert_eq!(app.pr_review_reply_view(), Some(true));
    app.pr_review_reply_editor_key(KeyEvent::from(KeyCode::Char('!')));
    assert_eq!(reply_editor_text(&app), format!("{before}!"));

    app.pr_review_reply_stop_edit();
    assert_eq!(app.pr_review_reply_view(), Some(false));
    app.pr_review_cancel_reply();
    assert_eq!(app.pr_review_reply_view(), None);
}

#[test]
fn pr_review_editing_a_captured_draft_drops_ai_attribution() {
    use crossterm::event::{KeyCode, KeyEvent};

    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review(&mut app, 1);

    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", "sha")
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Updated the lock scope and added a regression test."
    }));

    app.pr_review_open_reply_done();
    assert!(reply_is_agent_drafted(&app));
    assert!(reply_effective_agent_drafted(&app));

    app.pr_review_reply_edit();
    app.pr_review_reply_editor_key(KeyEvent::from(KeyCode::Char('!')));
    app.pr_review_reply_stop_edit();

    // `agent_drafted` stays true — it's a historical fact about how the reply
    // was seeded — but the *effective* attribution (what actually gets
    // posted) drops to AMF's channel-only footer now that the user has
    // changed the agent's words.
    assert!(reply_is_agent_drafted(&app));
    assert!(!reply_effective_agent_drafted(&app));
}

#[test]
fn pr_review_post_empty_reply_is_rejected() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    // Not-needed opens empty; posting without a reason is rejected (no write).
    app.pr_review_open_reply_not_needed();

    app.pr_review_post_reply().unwrap();
    assert_eq!(app.pr_review_reply_view(), Some(true));
    assert!(app.message.is_some());
}

fn memory_add_editor_text(app: &App) -> String {
    match &app.mode {
        AppMode::PrReview(state) => state.memory_add.as_ref().unwrap().editor.text().to_string(),
        _ => unreachable!(),
    }
}

fn memory_add_category(app: &App) -> usize {
    match &app.mode {
        AppMode::PrReview(state) => state.memory_add.as_ref().unwrap().category,
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_open_memory_add_seeds_finding_with_file_hint_and_default_category() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    // Post-ready like the "done" reply template: opens in the confirm view,
    // not straight into editing.
    assert_eq!(app.pr_review_memory_add_view(), Some(false));
    assert_eq!(memory_add_editor_text(&app), "comment 1 (src/file1.rs:1)");
    assert_eq!(memory_add_category(&app), 0, "defaults to General");
}

#[test]
fn pr_review_open_memory_add_without_comments_shows_message() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 0);

    app.pr_review_open_memory_add();
    assert_eq!(app.pr_review_memory_add_view(), None);
    assert!(app.message.is_some());
}

#[test]
fn pr_review_memory_add_edit_forwards_keys_and_cancel_closes() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    // Keys are ignored until edit mode is entered.
    let before = memory_add_editor_text(&app);
    app.pr_review_memory_add_editor_key(KeyEvent::from(KeyCode::Char('Z')));
    assert_eq!(memory_add_editor_text(&app), before);

    app.pr_review_memory_add_edit();
    assert_eq!(app.pr_review_memory_add_view(), Some(true));
    app.pr_review_memory_add_editor_key(KeyEvent::from(KeyCode::Char('!')));
    assert_eq!(memory_add_editor_text(&app), format!("{before}!"));

    app.pr_review_memory_add_stop_edit();
    assert_eq!(app.pr_review_memory_add_view(), Some(false));
    app.pr_review_cancel_memory_add();
    assert_eq!(app.pr_review_memory_add_view(), None);
}

#[test]
fn pr_review_cycle_memory_category_wraps() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    let n = crate::app::pr_review::MEMORY_CATEGORIES.len();
    for i in 1..=n {
        app.pr_review_cycle_memory_category();
        assert_eq!(memory_add_category(&app), i % n);
    }
}

#[test]
fn pr_review_append_empty_finding_is_rejected() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().editor = crate::editor::TextEditor::new(String::new());
    }

    // No `expect_repo_root()` was set on the mock: the empty check short-
    // circuits before the doc path is ever resolved.
    app.pr_review_append_memory().unwrap();
    assert_eq!(
        app.pr_review_memory_add_view(),
        Some(false),
        "dialog stays open"
    );
    assert!(app.message.is_some());
}

#[test]
fn pr_review_append_memory_writes_and_dedups() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(2)
        .returning(move |_| Ok(repo_clone.clone()));

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
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    app.pr_review_append_memory().unwrap();
    assert_eq!(
        app.pr_review_memory_add_view(),
        None,
        "dialog closes once appended"
    );

    let doc_path = repo.join(".amf").join("review-memory.md");
    let contents = std::fs::read_to_string(&doc_path).unwrap();
    assert!(contents.contains("## General"));
    assert!(contents.contains("- comment 1 (src/file1.rs:1)"));

    // Re-adding the same finding is a dedup no-op, not a duplicate bullet.
    app.pr_review_open_memory_add();
    app.pr_review_append_memory().unwrap();
    let contents_after = std::fs::read_to_string(&doc_path).unwrap();
    assert_eq!(
        contents_after.matches("comment 1 (src/file1.rs:1)").count(),
        1,
        "dedup should skip the second append"
    );
}

#[test]
fn pr_review_append_memory_honors_project_review_memory_path_override() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();

    // A project-level override in `{repo}/.amf/config.json` should redirect
    // the append away from the default `.amf/review-memory.md`.
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("config.json"),
        r#"{"review_memory_path": ".amf/team-review-memory.md"}"#,
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(1)
        .returning(move |_| Ok(repo_clone.clone()));

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
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    app.pr_review_append_memory().unwrap();

    let default_path = repo.join(".amf").join("review-memory.md");
    assert!(
        !default_path.exists(),
        "the default path should be untouched when a project override is set"
    );
    let overridden_path = repo.join(".amf").join("team-review-memory.md");
    let contents = std::fs::read_to_string(&overridden_path).unwrap();
    assert!(contents.contains("- comment 1 (src/file1.rs:1)"));
}

#[test]
fn pr_review_memory_add_defaults_to_project_scope_and_toggles() {
    use crate::app::review_memory::MemoryScope;

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    let scope = |app: &App| match &app.mode {
        AppMode::PrReview(state) => state.memory_add.as_ref().unwrap().scope,
        _ => panic!("expected PrReview"),
    };
    assert_eq!(
        scope(&app),
        MemoryScope::Project,
        "a finding from this PR is about this repo until the user says otherwise"
    );

    app.pr_review_toggle_memory_scope();
    assert_eq!(scope(&app), MemoryScope::Global);
    app.pr_review_toggle_memory_scope();
    assert_eq!(scope(&app), MemoryScope::Project);
}

#[test]
fn pr_review_append_memory_global_scope_writes_the_cross_project_doc() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let global_doc = tmp.path().join("global").join("review-memory.md");

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(1)
        .returning(move |_| Ok(repo_clone.clone()));

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
        Box::new(worktree),
    );
    // An absolute override keeps the test off the developer's real
    // `~/.config/amf/review-memory.md`.
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    app.pr_review_toggle_memory_scope();
    app.pr_review_append_memory().unwrap();

    assert_eq!(app.pr_review_memory_add_view(), None, "dialog closes");
    let contents = std::fs::read_to_string(&global_doc).unwrap();
    assert!(
        contents.starts_with("# Review memory (cross-project)"),
        "a freshly created global doc gets the cross-project header, got: {contents}"
    );
    assert!(contents.contains("- comment 1 (src/file1.rs:1)"));
    assert!(
        !repo.join(".amf").join("review-memory.md").exists(),
        "the project doc should be untouched when the global scope is picked"
    );
}

#[test]
fn review_memory_paths_resolves_project_override_and_global_together() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("config.json"),
        r#"{"review_memory_path": ".amf/team-review-memory.md"}"#,
    )
    .unwrap();
    let global_doc = tmp.path().join("global-lessons.md");

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
    app.config.global_review_memory_path = Some(global_doc.display().to_string());

    let paths = app.review_memory_paths(&repo);
    assert_eq!(
        paths.project,
        repo.join(".amf").join("team-review-memory.md")
    );
    assert_eq!(paths.global, global_doc);
}

#[test]
fn pr_review_open_fix_confirm_without_comments_shows_message() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 0);

    app.pr_review_open_fix_confirm();
    assert_eq!(app.pr_review_fix_editing(), None);
    assert!(app.message.is_some());
}

/// Build a fresh final-review `DiffViewerState` over two modified files for the
/// given workdir, install it as the current mode, and return nothing — the test
/// drives `app` afterwards.
#[cfg(test)]
fn enter_review_with_two_files(app: &mut App, workdir: &std::path::Path) {
    let mut state = DiffViewerState::new(
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
        workdir.to_path_buf(),
    );
    state.review = true;
    state.files = vec![
        crate::diff::DiffFile {
            old_path: Some("src/a.rs".into()),
            path: "src/a.rs".into(),
            status: crate::diff::DiffFileStatus::Modified,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        },
        crate::diff::DiffFile {
            old_path: Some("src/b.rs".into()),
            path: "src/b.rs".into(),
            status: crate::diff::DiffFileStatus::Modified,
            additions: 1,
            deletions: 1,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        },
    ];
    app.mode = AppMode::DiffViewer(state);
}

/// A `ProjectStore` with one project/feature named to match
/// `enter_review_with_two_files`'s `ViewState` ("proj"/"feat"), so
/// `finish_final_review`'s per-project `final_review_check_command` lookup
/// resolves to the project rooted at `repo`.
#[cfg(test)]
fn store_with_review_project(repo: &std::path::Path) -> ProjectStore {
    let now = Utc::now();
    let feature = Feature {
        id: "feat-1".to_string(),
        name: "feat".to_string(),
        branch: "feat".to_string(),
        workdir: repo.to_path_buf(),
        is_worktree: false,
        tmux_session: "amf-feat".to_string(),
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
        status: ProjectStatus::Active,
        created_at: now,
        last_accessed: now,
        summary: None,
        summary_updated_at: None,
        nickname: None,
        triage_source: None,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "proj".to_string(),
        repo: repo.to_path_buf(),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        preferred_agent: AgentKind::default(),
        is_git: false,
    };
    ProjectStore {
        version: 5,
        projects: vec![project],
        session_bookmarks: vec![],
        available_harnesses: vec![],
        prompt_templates: Vec::new(),
        extra: HashMap::new(),
    }
}

/// Poll `poll_final_review_check` until the spawned check process finishes
/// (or a generous cap is hit), so tests don't race the child process.
#[cfg(test)]
fn drain_final_review_check(app: &mut App) {
    for _ in 0..200 {
        app.poll_final_review_check().unwrap();
        let still_running =
            matches!(&app.mode, AppMode::DiffViewer(state) if state.finish_check_child.is_some());
        if !still_running {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("final review check did not complete in time");
}

#[test]
fn final_review_check_command_pass_reported_after_all_approved() {
    let repo = TempDir::new().unwrap();
    std::fs::create_dir_all(repo.path().join(".amf")).unwrap();
    std::fs::write(
        repo.path().join(".amf").join("config.json"),
        r#"{"final_review_check_command": "exit 0"}"#,
    )
    .unwrap();

    let mut app = App::new_for_test(
        store_with_review_project(repo.path()),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    enter_review_with_two_files(&mut app, repo.path());
    app.diff_review_approve_current();
    app.diff_review_approve_current();

    app.finish_final_review().unwrap();
    assert!(
        matches!(&app.mode, AppMode::DiffViewer(state) if state.finish_check_child.is_some()),
        "the check should be spawned in the background rather than run inline"
    );

    drain_final_review_check(&mut app);

    assert!(matches!(app.mode, AppMode::Viewing(_)));
    let msg = app.message.clone().unwrap_or_default();
    assert!(
        msg.contains("check `exit 0` passed"),
        "message should report the passing check: {msg}"
    );
}

#[test]
fn final_review_check_command_failure_blocks_all_approved_fast_path() {
    let repo = TempDir::new().unwrap();
    std::fs::create_dir_all(repo.path().join(".amf")).unwrap();
    std::fs::write(
        repo.path().join(".amf").join("config.json"),
        r#"{"final_review_check_command": "exit 1"}"#,
    )
    .unwrap();

    let mut app = App::new_for_test(
        store_with_review_project(repo.path()),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );

    enter_review_with_two_files(&mut app, repo.path());
    app.diff_review_approve_current();
    app.diff_review_approve_current();

    app.finish_final_review().unwrap();
    drain_final_review_check(&mut app);

    let msg = app.message.clone().unwrap_or_default();
    assert!(
        msg.contains("check `exit 1` FAILED"),
        "message should report the failing check: {msg}"
    );

    let feedback =
        std::fs::read_to_string(repo.path().join(".claude").join("final-review-feedback.md"))
            .expect("a failing check must still write the feedback file, even with 0 rejections");
    assert!(feedback.contains("**Check:** `exit 1` — FAILED"));
}

#[test]
fn load_prior_agent_responses_populates_state_for_known_files() {
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

    // A prior round's feedback file: replies on a known file (src/a.rs) and on a
    // file no longer in the diff (src/gone.rs), which must be filtered out.
    let claude = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(
        claude.join("final-review-feedback.md"),
        "# Final Review Feedback\n\n## Review — 2026-07-02T00:00:00Z\n\n\
         ### Line Comments\n\n#### src/a.rs:3 — [suggestion]\n\nRename it.\n\n\
         **Agent:** done, renamed.\n\n#### src/gone.rs:9 — [blocker]\n\nFix it.\n\n\
         **Agent:** removed the file.\n",
    )
    .unwrap();

    enter_review_with_two_files(&mut app, workdir.path());
    app.load_prior_agent_responses();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(
                state.prior_agent_responses.len(),
                1,
                "only files still in the diff are kept"
            );
            let a = state.prior_agent_responses.get("src/a.rs").expect("a.rs");
            assert_eq!(a[0].anchor, "src/a.rs:3");
            assert!(a[0].response.contains("renamed"));
            assert!(
                !state.prior_agent_responses.contains_key("src/gone.rs"),
                "a file no longer in the diff is dropped"
            );
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn load_prior_agent_responses_no_feedback_file_is_empty() {
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

    enter_review_with_two_files(&mut app, workdir.path());
    app.load_prior_agent_responses();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.prior_agent_responses.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn final_review_progress_persists_and_resumes() {
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

    enter_review_with_two_files(&mut app, workdir.path());

    // Approve file 0 (advances to file 1), reject file 1 with feedback, and add
    // general feedback. Each action persists progress to disk.
    app.diff_review_approve_current();
    app.diff_review_start_feedback();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new("needs work".into());
    }
    app.diff_review_submit_feedback();
    app.diff_review_start_general_feedback();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new("overall looks ok".into());
    }
    app.diff_review_submit_general_feedback();

    let progress_path = workdir
        .path()
        .join(".claude")
        .join("final-review-progress.json");
    assert!(progress_path.exists(), "progress file should be written");

    // Simulate closing and reopening the review: a brand-new state for the same
    // workdir, then restore.
    enter_review_with_two_files(&mut app, workdir.path());
    app.restore_review_progress();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(
                state.decisions.get("src/a.rs"),
                Some(&ReviewDecision::Approve)
            );
            assert_eq!(
                state.decisions.get("src/b.rs"),
                Some(&ReviewDecision::Reject {
                    feedback: "needs work".into(),
                    // An explicit rejection defaults to Blocker severity.
                    severity: crate::app::Severity::Blocker,
                })
            );
            assert_eq!(state.general_feedback, "overall looks ok");
        }
        _ => panic!("expected diff viewer after restore"),
    }

    // Finishing the review clears the saved progress.
    app.finish_final_review().unwrap();
    assert!(
        !progress_path.exists(),
        "progress file should be removed after finishing"
    );
}

#[test]
fn comment_auto_reject_survives_pause_and_resume() {
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

    // One commentable hunk on src/a.rs: a context line and an added line.
    let hunk = crate::diff::DiffHunk {
        header: "@@ -1,1 +1,2 @@".into(),
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 2,
        lines: vec![
            crate::diff::DiffLine {
                kind: crate::diff::DiffLineKind::Context,
                text: " ctx".into(),
            },
            crate::diff::DiffLine {
                kind: crate::diff::DiffLineKind::Added,
                text: "+added".into(),
            },
        ],
    };

    // Comment the added line: the file auto-rejects and progress persists.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![hunk.clone()];
        state.comment_cursor = Some(1);
        state.editing_line_comment = true;
        state.feedback_editor = crate::editor::TextEditor::new("bug".into());
    }
    app.diff_review_submit_line_comment();

    // Reopen the review fresh and restore: the verdict comes back still marked
    // as comment-implied, not explicit.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![hunk];
    }
    app.restore_review_progress();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert_eq!(
                state.decisions.get("src/a.rs"),
                Some(&ReviewDecision::Reject {
                    feedback: String::new(),
                    // An auto-rejection carries the neutral default severity;
                    // the real severity lives on the line comment.
                    severity: crate::app::Severity::Suggestion,
                })
            );
            assert!(state.auto_rejected.contains("src/a.rs"));
        }
        _ => panic!("expected diff viewer after restore"),
    }

    // Deleting the restored comment (empty re-submit) still clears the
    // implicit verdict — the distinction survived the round-trip.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.comment_cursor = Some(1);
        state.editing_line_comment = true;
        state.feedback_editor = crate::editor::TextEditor::new(String::new());
    }
    app.diff_review_submit_line_comment();
    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.decisions.is_empty());
            assert!(state.auto_rejected.is_empty());
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn restore_review_progress_skips_when_decisions_present() {
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

    // Write a progress file that, if applied, would approve src/a.rs.
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("final-review-progress.json"),
        r#"{"decisions":{"src/a.rs":"Approve"},"line_comments":{},"general_feedback":"stale","selected_file":0}"#,
    )
    .unwrap();

    // Enter a review that already has an in-memory decision (mimicking an
    // in-review refresh); restore must not clobber it with the disk copy.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state
            .decisions
            .insert("src/b.rs".into(), ReviewDecision::Approve);
    }
    app.restore_review_progress();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(
                !state.decisions.contains_key("src/a.rs"),
                "should not have merged disk progress over in-memory state"
            );
            assert_eq!(state.general_feedback, "");
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn re_review_flags_changed_files_and_applies_filter() {
    use crate::app::FileFilter;
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

    // First round: approve both files and finish. This records the snapshot.
    enter_review_with_two_files(&mut app, workdir.path());
    app.diff_review_approve_current();
    app.diff_review_approve_current();
    app.finish_final_review().unwrap();

    let snapshot_path = workdir
        .path()
        .join(".claude")
        .join("final-review-snapshot.json");
    assert!(
        snapshot_path.exists(),
        "snapshot should be written on finish"
    );

    // Re-open with src/b.rs's diff changed (different patch); src/a.rs is
    // untouched.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[1].patch = "@@ -1 +1 @@\n-old\n+new".into();
    }
    app.apply_review_snapshot_diff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.has_prior_review, "a prior snapshot exists");
            assert_eq!(
                state.changed_since_last.iter().cloned().collect::<Vec<_>>(),
                vec!["src/b.rs".to_string()],
                "only the changed file is flagged"
            );
            assert_eq!(
                state.file_filter,
                FileFilter::Changed,
                "a partial re-review auto-applies the changed filter"
            );
            assert_eq!(
                state.selected_file, 1,
                "selection snaps onto the changed file"
            );
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn re_review_no_changes_keeps_all_filter() {
    use crate::app::FileFilter;
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

    enter_review_with_two_files(&mut app, workdir.path());
    app.diff_review_approve_current();
    app.diff_review_approve_current();
    app.finish_final_review().unwrap();

    // Re-open with the identical diff: nothing changed since the last round.
    enter_review_with_two_files(&mut app, workdir.path());
    app.apply_review_snapshot_diff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.has_prior_review);
            assert!(
                state.changed_since_last.is_empty(),
                "no files changed since the last review"
            );
            assert_eq!(
                state.file_filter,
                FileFilter::All,
                "an unchanged re-review leaves the filter untouched"
            );
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn re_review_carries_approvals_for_unchanged_files() {
    use crate::app::FileFilter;
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

    // Round 1: approve src/a.rs, reject src/b.rs, and finish. The snapshot now
    // records a.rs=Approve and b.rs=Reject.
    enter_review_with_two_files(&mut app, workdir.path());
    app.diff_review_approve_current();
    app.diff_review_start_feedback();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.feedback_editor = crate::editor::TextEditor::new("fix this".into());
    }
    app.diff_review_submit_feedback();
    app.finish_final_review().unwrap();

    // Round 2: the fix landed, so b.rs's diff changed; a.rs is untouched.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[1].patch = "@@ -1 +1 @@\n-old\n+new".into();
    }
    app.apply_review_snapshot_diff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            // The unchanged, previously-approved file keeps its verdict — the
            // reviewer resumes from the saved approved state.
            assert_eq!(
                state.decisions.get("src/a.rs"),
                Some(&ReviewDecision::Approve),
                "an unchanged approved file carries its approval into the re-review"
            );
            // The changed (and previously rejected) file is reset for a fresh
            // look, and the review narrows onto it.
            assert!(
                !state.decisions.contains_key("src/b.rs"),
                "a changed file does not carry a stale verdict"
            );
            assert_eq!(state.file_filter, FileFilter::Changed);
            assert_eq!(state.selected_file, 1);
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn re_review_carries_an_unresolved_thread_and_reports_its_count() {
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

    let hunk = crate::diff::DiffHunk {
        header: "@@ -1,1 +1,2 @@".into(),
        old_start: 1,
        old_lines: 1,
        new_start: 1,
        new_lines: 2,
        lines: vec![
            crate::diff::DiffLine {
                kind: crate::diff::DiffLineKind::Context,
                text: " ctx".into(),
            },
            crate::diff::DiffLine {
                kind: crate::diff::DiffLineKind::Added,
                text: "+added".into(),
            },
        ],
    };

    // Round 1: comment on src/a.rs (auto-rejects it), approve src/b.rs, finish.
    // The snapshot now carries the open thread on src/a.rs.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![hunk.clone()];
        state.comment_cursor = Some(1);
        state.editing_line_comment = true;
        state.feedback_editor = crate::editor::TextEditor::new("still needs work".into());
    }
    app.diff_review_submit_line_comment();
    app.diff_review_approve_current();
    app.finish_final_review().unwrap();

    // Round 2: reopen against the same (untouched) diff and restore.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].hunks = vec![hunk];
    }
    app.restore_review_progress();
    app.apply_review_snapshot_diff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            let comments = state.line_comments.get("src/a.rs").expect("thread carried");
            assert_eq!(comments.len(), 1);
            assert!(comments[0].carried, "restored thread is marked carried");
            assert!(!comments[0].resolved, "the thread is still open");
            assert_eq!(comments[0].text, "still needs work");
            // The open thread means src/a.rs must not inherit any stale
            // approval, even though nothing about its diff changed.
            assert!(!state.decisions.contains_key("src/a.rs"));
        }
        _ => panic!("expected diff viewer"),
    }
    let message = app.message.as_deref().unwrap_or_default();
    assert!(
        message.contains("no files changed since the last review"),
        "{message}"
    );
    assert!(
        message.contains("1 unresolved thread carried over"),
        "{message}"
    );
}

#[test]
fn interdiff_shows_diff_since_last_reviewed_content() {
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

    // Round 1: src/a.rs's worktree content at the time the round finishes is
    // captured into the snapshot.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].new_content = Some("fn a() {}\n".into());
    }
    app.diff_review_approve_current();
    app.diff_review_approve_current();
    app.finish_final_review().unwrap();

    // Round 2: the agent edited src/a.rs since. Its base-ref patch is left
    // empty (as in the fixture) — the interdiff must not depend on it.
    enter_review_with_two_files(&mut app, workdir.path());
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files[0].new_content = Some("fn a() {\n    println!(\"hi\");\n}\n".into());
    }

    app.open_interdiff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(state.interdiff_open, "modal opens when content changed");
            let diff_file = state
                .interdiff_file
                .as_ref()
                .expect("interdiff computed against last round's content");
            assert_eq!(diff_file.path, "src/a.rs");
            assert!(
                !diff_file.hunks.is_empty(),
                "the content actually changed between rounds"
            );
            assert!(diff_file.patch.contains("println"), "{}", diff_file.patch);
        }
        _ => panic!("expected diff viewer"),
    }
}

#[test]
fn interdiff_noop_without_prior_review() {
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

    // No prior finished review, so no snapshot exists on disk yet.
    enter_review_with_two_files(&mut app, workdir.path());
    app.open_interdiff();

    match &app.mode {
        AppMode::DiffViewer(state) => {
            assert!(!state.interdiff_open, "nothing to diff against yet");
            assert!(state.interdiff_file.is_none());
        }
        _ => panic!("expected diff viewer"),
    }
    assert_eq!(
        app.message.as_deref(),
        Some("No prior review to diff against")
    );
}

#[test]
fn session_picker_offers_todos_when_project_has_none() {
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
                    .any(|session| session.kind == SessionKind::Todos),
                "TODOs should be offered when the project has none"
            );
        }
        _ => panic!("expected SessionPicker mode"),
    }
}

#[test]
fn session_picker_hides_todos_when_project_already_has_one() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();

    app.selection = Selection::Feature(0, 0);
    app.open_session_picker().unwrap();

    match &app.mode {
        AppMode::SessionPicker(state) => {
            assert!(
                !state
                    .builtin_sessions
                    .iter()
                    .any(|session| session.kind == SessionKind::Todos),
                "TODOs should be hidden once the project already has one"
            );
        }
        _ => panic!("expected SessionPicker mode"),
    }
}

#[test]
fn add_builtin_session_blocks_second_todos_per_project() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);

    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();
    assert!(app.store.projects[0].has_todos_session());
    let todos_count = |app: &App| {
        app.store.projects[0]
            .features
            .iter()
            .flat_map(|f| &f.sessions)
            .filter(|s| s.kind == SessionKind::Todos)
            .count()
    };
    assert_eq!(todos_count(&app), 1);

    // A second attempt is rejected; still exactly one TODOs session.
    app.selection = Selection::Feature(0, 0);
    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();
    assert_eq!(
        todos_count(&app),
        1,
        "a second TODOs session must be blocked"
    );
    assert_eq!(
        app.message.as_deref(),
        Some("This project already has a TODOs session")
    );
}

#[test]
fn todos_session_does_not_create_a_tmux_window() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);

    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();

    let session = app.store.projects[0].features[0]
        .sessions
        .iter()
        .find(|s| s.kind == SessionKind::Todos)
        .expect("TODOs session should exist");
    assert!(!session.kind.is_tmux_backed());
    assert_eq!(session.label, "TODOs");
}

/// Build a DB-backed app whose project `proj-1` has two features (`feat-1`,
/// `feat-2`) and a TODO list hosted by `feat-1` with one item. Returns the app
/// with `feat-1` already removed from the store (as after a delete).
fn app_with_todo_host_deleted(remove_all_features: bool) -> App {
    let mut store = store_with_feature(ProjectStatus::Active);
    if !remove_all_features {
        let mut second = store.projects[0].features[0].clone();
        second.id = "feat-2".to_string();
        second.name = "other-feat".to_string();
        store.projects[0].features.push(second);
    }

    let db_dir = TempDir::new().unwrap();
    // Keep the tempdir alive for the app's lifetime by leaking it; tests are
    // short-lived processes.
    let db_path = db_dir.keep().join("amf.db");
    let db = crate::db::AmfDb::open(&db_path).unwrap();
    db.save_store(&store).unwrap();
    let list = db.create_todo_list("proj-1", "feat-1").unwrap();
    db.add_todo(
        &list.id,
        "do a thing",
        None,
        crate::db::todos::TodoPriority::Med,
    )
    .unwrap();

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(db);
    // Simulate the host feature already having been removed from the store.
    app.store.projects[0].features.retain(|f| f.id != "feat-1");
    app
}

#[test]
fn host_feature_delete_prompts_rehome_onto_surviving_feature() {
    let mut app = app_with_todo_host_deleted(false);

    let opened = app.handle_todos_host_feature_deleted("my-project", "my-feat", Some("feat-1"));
    assert!(
        opened,
        "deleting the host feature should open the re-home prompt"
    );
    match &app.mode {
        AppMode::TodosHostReassign(state) => {
            assert_eq!(
                state.candidates,
                vec![("other-feat".to_string(), "feat-2".to_string())]
            );
            assert_eq!(state.todo_count, 1);
            assert_eq!(state.selected, 0);
        }
        _ => panic!("expected TodosHostReassign mode"),
    }

    // Confirm the (default) re-home onto the surviving feature.
    app.confirm_todos_host_reassign().unwrap();
    let reloaded = app
        .db
        .as_ref()
        .unwrap()
        .todo_list("proj-1")
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.feature_id, "feat-2");
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn host_feature_delete_can_delete_the_list() {
    let mut app = app_with_todo_host_deleted(false);
    app.handle_todos_host_feature_deleted("my-project", "my-feat", Some("feat-1"));

    // Move selection onto the trailing "Delete" option and confirm.
    if let AppMode::TodosHostReassign(state) = &mut app.mode {
        state.selected = state.candidates.len();
    }
    app.confirm_todos_host_reassign().unwrap();

    assert!(
        app.db
            .as_ref()
            .unwrap()
            .todo_list("proj-1")
            .unwrap()
            .is_none()
    );
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn host_feature_delete_drops_list_when_no_features_remain() {
    let mut app = app_with_todo_host_deleted(true);

    let opened = app.handle_todos_host_feature_deleted("my-project", "my-feat", Some("feat-1"));
    assert!(!opened, "no surviving features → no prompt");
    // The orphaned list is dropped, not left dangling.
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .todo_list("proj-1")
            .unwrap()
            .is_none()
    );
}

#[test]
fn deleting_non_host_feature_leaves_todo_list_untouched() {
    let mut app = app_with_todo_host_deleted(false);

    // A different (non-host) feature id is passed in.
    let opened = app.handle_todos_host_feature_deleted("my-project", "other-feat", Some("feat-2"));
    assert!(!opened);
    let list = app
        .db
        .as_ref()
        .unwrap()
        .todo_list("proj-1")
        .unwrap()
        .unwrap();
    assert_eq!(list.feature_id, "feat-1", "list host should be unchanged");
}

fn sample_todo(title: &str, done: bool) -> crate::db::todos::Todo {
    crate::db::todos::Todo {
        id: format!("todo-{title}"),
        list_id: "list-1".to_string(),
        title: title.to_string(),
        body: None,
        priority: crate::db::todos::TodoPriority::Med,
        done,
        sort_order: 0,
        spawned_session_id: None,
        created_at: String::new(),
        updated_at: String::new(),
    }
}

#[test]
fn entering_todos_session_opens_native_overlay() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();

    // Select the TODOs session and open it.
    let si = app.store.projects[0].features[0]
        .sessions
        .iter()
        .position(|s| s.kind == SessionKind::Todos)
        .unwrap();
    app.selection = Selection::Session(0, 0, si);
    app.enter_view().unwrap();

    match &app.mode {
        AppMode::Todos(state) => {
            assert_eq!(state.project_name, "my-project");
            assert_eq!(state.feature_name, "my-feat");
            assert_eq!(state.pi, 0);
            assert_eq!(state.fi, 0);
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn closing_todos_overlay_returns_to_session_selection() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);
    app.add_builtin_session(0, 0, SessionKind::Todos).unwrap();
    let si = app.store.projects[0].features[0]
        .sessions
        .iter()
        .position(|s| s.kind == SessionKind::Todos)
        .unwrap();
    app.selection = Selection::Session(0, 0, si);
    app.enter_view().unwrap();

    app.close_todos_view();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(matches!(app.selection, Selection::Session(0, 0, s) if s == si));
}

#[test]
fn todos_navigation_wraps_around() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Todos(TodoViewState {
        todos: vec![
            sample_todo("a", false),
            sample_todo("b", false),
            sample_todo("c", true),
        ],
        ..empty_todo_view()
    });

    let selected = |app: &App| match &app.mode {
        AppMode::Todos(state) => state.selected,
        _ => panic!("expected Todos overlay"),
    };

    app.todos_select_next();
    assert_eq!(selected(&app), 1);
    app.todos_select_next();
    app.todos_select_next();
    assert_eq!(selected(&app), 0, "next wraps from last back to first");
    app.todos_select_prev();
    assert_eq!(selected(&app), 2, "prev wraps from first to last");
}

#[test]
fn todos_navigation_no_op_when_empty() {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Todos(empty_todo_view());

    app.todos_select_next();
    app.todos_select_prev();
    match &app.mode {
        AppMode::Todos(state) => assert_eq!(state.selected, 0),
        _ => panic!("expected Todos overlay"),
    }
}

fn empty_todo_view() -> TodoViewState {
    TodoViewState {
        project_id: "proj-1".to_string(),
        pi: 0,
        fi: 0,
        project_name: "my-project".to_string(),
        feature_name: "my-feat".to_string(),
        list: None,
        todos: vec![],
        selected: 0,
        scroll_offset: 0,
        editor: None,
        pending_delete: false,
    }
}

fn todos_app() -> App {
    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Todos(empty_todo_view());
    app
}

fn ke(code: KeyCode) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
}

fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        crate::handlers::handle_todos_key(app, ke(KeyCode::Char(c))).unwrap();
    }
}

fn todo_titles(app: &App) -> Vec<String> {
    match &app.mode {
        AppMode::Todos(state) => state.todos.iter().map(|t| t.title.clone()).collect(),
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_add_via_handler_appends_and_selects() {
    let mut app = todos_app();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('a'))).unwrap();
    type_str(&mut app, "buy milk");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert_eq!(todo_titles(&app), vec!["buy milk"]);
    match &app.mode {
        AppMode::Todos(state) => {
            assert_eq!(state.selected, 0);
            assert!(state.editor.is_none(), "editor closes after commit");
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_empty_title_is_not_added() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "   ");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();
    assert!(todo_titles(&app).is_empty());
}

#[test]
fn todos_edit_title_replaces_text() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "old");
    app.todos_commit_edit().unwrap();

    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('e'))).unwrap();
    // Clear the seeded "old" then type "new".
    for _ in 0..3 {
        crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Backspace)).unwrap();
    }
    type_str(&mut app, "new");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    assert_eq!(todo_titles(&app), vec!["new"]);
}

#[test]
fn todos_toggle_done_sinks_item_below_open() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "a");
    app.todos_commit_edit().unwrap();
    app.todos_begin_add();
    type_str(&mut app, "b");
    app.todos_commit_edit().unwrap();

    // Select "a" (index 0) and mark it done.
    if let AppMode::Todos(state) = &mut app.mode {
        state.selected = 0;
    }
    app.todos_toggle_done().unwrap();

    // Open "b" now sorts before done "a"; cursor follows "a".
    assert_eq!(todo_titles(&app), vec!["b", "a"]);
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.todos[1].done);
            assert_eq!(state.selected, 1);
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_cycle_priority_rotates() {
    use crate::db::todos::TodoPriority;
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "a");
    app.todos_commit_edit().unwrap();

    let prio = |app: &App| match &app.mode {
        AppMode::Todos(state) => state.todos[0].priority,
        _ => panic!("expected Todos overlay"),
    };
    assert_eq!(prio(&app), TodoPriority::Med);
    app.todos_cycle_priority().unwrap();
    assert_eq!(prio(&app), TodoPriority::Low);
    app.todos_cycle_priority().unwrap();
    assert_eq!(prio(&app), TodoPriority::High);
    app.todos_cycle_priority().unwrap();
    assert_eq!(prio(&app), TodoPriority::Med);
}

#[test]
fn todos_reorder_moves_item() {
    let mut app = todos_app();
    for t in ["a", "b", "c"] {
        app.todos_begin_add();
        type_str(&mut app, t);
        app.todos_commit_edit().unwrap();
    }
    // Select "a" (top) and move it down.
    if let AppMode::Todos(state) = &mut app.mode {
        state.selected = 0;
    }
    app.todos_reorder(1).unwrap();

    assert_eq!(todo_titles(&app), vec!["b", "a", "c"]);
    match &app.mode {
        AppMode::Todos(state) => assert_eq!(state.selected, 1, "cursor follows moved item"),
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_delete_requires_confirmation() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "doomed");
    app.todos_commit_edit().unwrap();

    // 'd' arms the confirm; 'n' cancels and keeps the item.
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('d'))).unwrap();
    assert!(matches!(&app.mode, AppMode::Todos(s) if s.pending_delete));
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('n'))).unwrap();
    assert_eq!(todo_titles(&app), vec!["doomed"]);

    // 'd' then 'y' deletes it.
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('d'))).unwrap();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('y'))).unwrap();
    assert!(todo_titles(&app).is_empty());
}

#[test]
fn todos_edit_scratchpad_banner() {
    let mut app = todos_app();
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Char('b'))).unwrap();
    type_str(&mut app, "finishing the parser");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Enter)).unwrap();

    match &app.mode {
        AppMode::Todos(state) => {
            assert_eq!(
                state.list.as_ref().and_then(|l| l.carry_over.as_deref()),
                Some("finishing the parser")
            );
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn todos_edit_cancel_discards_changes() {
    let mut app = todos_app();
    app.todos_begin_add();
    type_str(&mut app, "scratch");
    crate::handlers::handle_todos_key(&mut app, ke(KeyCode::Esc)).unwrap();
    assert!(todo_titles(&app).is_empty());
    assert!(matches!(&app.mode, AppMode::Todos(s) if s.editor.is_none()));
}

#[test]
fn todo_spawn_prompt_includes_title_and_body() {
    let mut todo = sample_todo("Wire up the parser", false);
    let title_only = App::todo_spawn_prompt(&todo);
    assert_eq!(
        title_only,
        "Please address this TODO item for this feature:\n\nWire up the parser"
    );

    todo.body = Some("  handle nested groups\nand escapes  ".to_string());
    let with_body = App::todo_spawn_prompt(&todo);
    assert_eq!(
        with_body,
        "Please address this TODO item for this feature:\n\nWire up the parser\n\nhandle nested groups\nand escapes"
    );

    // A whitespace-only body is treated as absent.
    todo.body = Some("   ".to_string());
    assert_eq!(App::todo_spawn_prompt(&todo), title_only);
}

#[test]
fn todo_session_label_truncates_long_titles() {
    assert_eq!(App::todo_session_label("short"), "TODO: short");
    let long = "a really long todo title that keeps going and going";
    let label = App::todo_session_label(long);
    assert!(label.starts_with("TODO: "));
    assert!(label.ends_with('…'), "long titles are elided: {label}");
    assert!(label.chars().count() <= "TODO: ".len() + 24 + 1);
}

#[test]
fn resolve_todo_host_feature_prefers_list_feature_id() {
    let app = todos_app();
    // Known feature id resolves to its index (0), regardless of the fallback.
    assert_eq!(app.resolve_todo_host_feature(0, Some("feat-1"), 3), 0);
    // Unknown / missing id falls back.
    assert_eq!(app.resolve_todo_host_feature(0, Some("nope"), 7), 7);
    assert_eq!(app.resolve_todo_host_feature(0, None, 5), 5);
}

#[test]
fn todos_record_spawned_session_updates_in_memory() {
    let mut app = todos_app();
    if let AppMode::Todos(state) = &mut app.mode {
        state.todos = vec![sample_todo("a", false), sample_todo("b", false)];
    }
    app.todos_record_spawned_session("todo-b", "sess-42")
        .unwrap();
    match &app.mode {
        AppMode::Todos(state) => {
            assert!(state.todos[0].spawned_session_id.is_none());
            assert_eq!(
                state.todos[1].spawned_session_id.as_deref(),
                Some("sess-42")
            );
        }
        _ => panic!("expected Todos overlay"),
    }
}

#[test]
fn poll_ai_pr_review_bg_warns_when_reviewing_and_done_arrive_together() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_bg = Some(rx);
    app.ai_review_pending = Some(origin.clone());
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    tx.send(crate::app::ai_review::AiReviewProgress::Reviewing {
        token_estimate: 50_000,
    })
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![],
            summary: Some("No actionable issues found.".to_string()),
            raw_output: String::new(),
        },
    )))
    .unwrap();
    assert!(app.poll_ai_pr_review_bg());
    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("Large diff") && t.message.contains("50000")),
        "toasts: {:?}",
        app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
    assert!(
        !app.toasts
            .iter()
            .any(|toast| toast.message.contains("check the debug log")),
        "a valid summary-only response is a clean zero-finding review"
    );
}

#[test]
fn completed_ai_review_updates_stashed_triage_pending_count_and_summary() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    app.open_ai_review_from_triage();
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_bg = Some(rx);
    app.ai_review_pending = Some(origin.clone());
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![
                sample_ai_review_finding("first"),
                sample_ai_review_finding("second"),
            ],
            summary: Some("Two correctness risks need attention.".to_string()),
            raw_output: "review output".to_string(),
        },
    )))
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(
                state.summary.as_deref(),
                Some("Two correctness risks need attention.")
            );
            assert_eq!(state.findings.len(), 2);
        }
        _ => panic!("expected AI Review pane"),
    }
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => assert_eq!(state.pending_ai_review_findings, 2),
        _ => panic!("expected stashed PR Triage pane"),
    }

    app.ai_review_toggle_skip();
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => assert_eq!(state.pending_ai_review_findings, 1),
        _ => panic!("expected stashed PR Triage pane"),
    }
}

#[test]
fn poll_ai_pr_review_bg_surfaces_streamed_activity_and_usage() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_bg = Some(rx);
    app.ai_review_pending = Some(origin.clone());
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::Reviewing {
                token_estimate: 42_000,
            },
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    tx.send(crate::app::ai_review::AiReviewProgress::Activity(
        "Inspecting the repository".to_string(),
    ))
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Usage {
        input_tokens: 41_000,
        output_tokens: 900,
    })
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::AiReviewRunning(state) => {
            assert_eq!(
                state.progress.activity.as_deref(),
                Some("Inspecting the repository")
            );
            assert_eq!(state.progress.usage, Some((41_000, 900)));
        }
        _ => panic!("expected running AI review"),
    }
}

#[test]
fn running_ai_review_can_reopen_preserved_progress_after_escape() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let started_at = std::time::Instant::now() - std::time::Duration::from_secs(75);
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_bg = Some(rx);
    app.ai_review_pending = Some(origin.clone());
    app.ai_review_progress = Some(crate::app::AiReviewRunProgress {
        stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
        started_at,
        activity: None,
        usage: None,
    });

    // Progress that lands after leaving the full-screen view is retained in
    // the app-level run state rather than discarded with the old mode.
    tx.send(crate::app::ai_review::AiReviewProgress::Reviewing {
        token_estimate: 95_000,
    })
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Activity(
        "Inspecting the repository".to_string(),
    ))
    .unwrap();
    assert!(app.poll_ai_pr_review_bg());

    app.start_ai_pr_review();
    match &app.mode {
        AppMode::AiReviewRunning(state) => {
            assert_eq!(state.progress.started_at, started_at);
            assert_eq!(
                state.progress.stage,
                crate::app::ai_review::AiReviewStage::Reviewing {
                    token_estimate: 95_000
                }
            );
            assert_eq!(
                state.progress.activity.as_deref(),
                Some("Inspecting the repository")
            );
        }
        _ => panic!("expected preserved AI review progress"),
    }
}

#[test]
fn open_ai_review_for_pr_starts_empty_with_no_stashed_return() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    // A stash left over from a previous PR Triage-originated visit must not
    // leak into a fresh dashboard/session/picker entry.
    let pr = crate::github::PrRef {
        number: 1,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/1".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    app.ai_review_return_to = Some(Box::new(AppMode::Normal));

    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), pr.clone());

    assert!(app.ai_review_return_to.is_none());
    match &app.mode {
        AppMode::AiReview(state) => {
            assert!(state.findings.is_empty());
            assert_eq!(state.pr.number, 1);
            assert_eq!(state.workdir, PathBuf::from("/tmp/test-workdir"));
        }
        other => panic!("expected AiReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn open_ai_review_for_pr_reopens_cached_findings_and_summary() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    let pr = pr_review_with_comments(1).pr;
    app.db
        .as_ref()
        .unwrap()
        .save_ai_review_cache(
            pr.number,
            &pr.head_sha,
            &crate::app::ai_review::AiReviewCacheEntry {
                findings: vec![sample_ai_review_finding("cached finding")],
                last_run: Some(crate::app::ai_review::AiReviewRun {
                    ran_at: chrono::Local::now(),
                    outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(1),
                }),
                summary: Some("Cached review summary.".to_string()),
            },
        )
        .unwrap();
    assert_eq!(app.pending_ai_review_count(&pr), 1);

    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), pr);

    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.findings[0].body, "cached finding");
            assert_eq!(state.summary.as_deref(), Some("Cached review summary."));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn ai_review_post_dialog_is_seeded_with_generated_summary_and_attribution() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.findings = vec![sample_ai_review_finding("A general finding")];
        state.summary = Some("The patch has one correctness risk.".to_string());
        state.last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(1),
        });
    }

    app.ai_review_open_post_confirm();

    match &app.mode {
        AppMode::AiReview(state) => {
            let body = state.post_confirm.as_ref().unwrap().editor.text();
            assert!(body.starts_with("The patch has one correctness risk."));
            assert!(body.contains("A general finding"));
            assert!(body.ends_with("— AI review via AMF"));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn ai_review_post_dialog_drops_generated_summary_when_a_finding_is_skipped() {
    // Regression: `state.summary` is model prose written over the *complete*
    // finding set, so it can still describe a finding the user has since
    // skipped (a false positive, or one too sensitive to post) even though
    // that finding is excluded from the posted findings below it. Once
    // anything's skipped, the dialog must fall back to the generic
    // placeholder rather than risk republishing what `skipped` was meant to
    // suppress.
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        let mut skipped = sample_ai_review_finding("A sensitive finding");
        skipped.skipped = true;
        state.findings = vec![skipped, sample_ai_review_finding("A kept finding")];
        state.summary = Some("The patch has a sensitive issue plus a kept finding.".to_string());
        state.last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(2),
        });
    }

    app.ai_review_open_post_confirm();

    match &app.mode {
        AppMode::AiReview(state) => {
            let body = state.post_confirm.as_ref().unwrap().editor.text();
            assert!(
                !body.contains("sensitive issue"),
                "the model summary describing the skipped finding must not be posted: {body}"
            );
            assert!(body.starts_with("AI review, via AMF."));
            assert!(body.contains("A kept finding"));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn open_ai_review_from_triage_stashes_the_pane_and_close_restores_it() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let triage_workdir = match &app.mode {
        AppMode::PrReview(state) => state.workdir.clone(),
        _ => unreachable!(),
    };

    app.open_ai_review_from_triage();

    match &app.mode {
        AppMode::AiReview(state) => assert_eq!(state.workdir, triage_workdir),
        other => panic!("expected AiReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.ai_review_return_to.is_some());

    app.close_ai_review();
    assert!(matches!(&app.mode, AppMode::PrReview(_)));
    assert!(app.ai_review_return_to.is_none());
}

#[test]
fn post_success_refresh_updates_stashed_triage_without_leaving_ai_review() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
        state.hide_resolved = true;
        state.marked.insert(2);
    }
    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh { workdir, pr });
    tx.send(Ok(pr_review_with_comments(3))).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_eq!(state.review.comments.len(), 3);
            assert_eq!(state.selected_comment().map(|comment| comment.id), Some(2));
            assert!(state.hide_resolved);
            assert!(state.marked.contains(&2));
            assert_eq!(state.pending_ai_review_findings, 0);
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_snaps_selection_off_a_newly_resolved_comment() {
    // Regression: restoring the selection by id after a refresh can land it
    // on a comment that `hide_resolved` now excludes (its thread resolved
    // upstream since the last fetch), leaving `selected` pointing at a row
    // `visible_indices()` doesn't include.
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 3);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1; // comment id 2, not yet resolved
        state.hide_resolved = true;
    }
    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh { workdir, pr });

    // The refresh comes back with comment 2's thread now resolved.
    let comments: Vec<crate::github::ReviewComment> = (1..=3u64)
        .map(|id| crate::github::ReviewComment {
            id,
            path: Some(format!("src/file{id}.rs")),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("comment {id}"),
            user: crate::github::GhUser {
                login: "alice".to_string(),
                kind: "User".to_string(),
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let threads = vec![crate::github::ReviewThread {
        id: "T2".to_string(),
        is_resolved: true,
        comment_ids: vec![2],
    }];
    let refreshed_pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let refreshed =
        crate::app::pr_review::normalize(refreshed_pr, comments, vec![], vec![], threads);
    tx.send(Ok(refreshed)).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_ne!(
                state.selected_comment().map(|comment| comment.id),
                Some(2),
                "selection must not stay on a comment hide_resolved now excludes"
            );
            assert!(
                state.visible_indices().contains(&state.selected),
                "selection must land on a row the current filter shows"
            );
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_snaps_selection_off_a_newly_collated_amf_reply() {
    use crate::app::pr_review::AMF_ATTRIBUTION_FOOTER;

    // Regression: `selected` can itself be an orphaned AMF follow-up reply
    // (its root wasn't fetched, so it stood as its own row). If a refresh
    // fetches the root, the reply becomes collated under it and vanishes
    // from `visible_indices()`. `all_sorted_indices()` used to also drop
    // collated replies, so `position()` couldn't find `selected` there
    // either and both neighbor searches short-circuited to `visible[0]`
    // instead of the nearest neighbor (the reply's own root).
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);

    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let user = |login: &str| crate::github::GhUser {
        login: login.to_string(),
        kind: "User".to_string(),
    };

    // An unrelated comment that sorts before the reply's root in fetch order.
    // Its presence is what distinguishes "snap to the nearest neighbor" from
    // "snap to the first visible row" — with only the reply and its root,
    // the root would also happen to be `visible[0]`, masking the bug.
    let unrelated = crate::github::ReviewComment {
        id: 3,
        path: Some("src/other.rs".into()),
        line: Some(4),
        original_line: Some(4),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: "Unrelated finding.".into(),
        user: user("reviewer"),
        in_reply_to_id: None,
        pull_request_review_id: None,
    };

    // Initial fetch: the AMF reply came back but its root did not, so it's
    // orphaned and shown as its own row. It's selected.
    let orphan_reply = crate::github::ReviewComment {
        id: 2,
        path: Some("src/lib.rs".into()),
        line: Some(10),
        original_line: Some(10),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: format!("Done in `abc123`.\n\n{AMF_ATTRIBUTION_FOOTER}"),
        user: user("author"),
        in_reply_to_id: Some(1),
        pull_request_review_id: Some(91),
    };
    app.mode = AppMode::PrReview(PrReviewState {
        selected: 1,
        review: crate::app::pr_review::normalize(
            pr.clone(),
            vec![unrelated.clone(), orphan_reply.clone()],
            vec![],
            vec![],
            vec![],
        ),
        ..match app.mode {
            AppMode::PrReview(state) => state,
            _ => unreachable!(),
        }
    });

    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh {
        workdir,
        pr: pr.clone(),
    });

    // The refresh comes back with the reply's root now present, so the
    // reply collates under it and drops out of `visible_indices()`.
    let root = crate::github::ReviewComment {
        id: 1,
        path: Some("src/lib.rs".into()),
        line: Some(10),
        original_line: Some(10),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: "Please add a test here.".into(),
        user: user("reviewer"),
        in_reply_to_id: None,
        pull_request_review_id: Some(90),
    };
    let refreshed = crate::app::pr_review::normalize(
        pr,
        vec![unrelated, root, orphan_reply],
        vec![],
        vec![],
        vec![],
    );
    tx.send(Ok(refreshed)).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_ne!(
                state.selected_comment().map(|comment| comment.id),
                Some(2),
                "selection must not stay on the now-collated AMF reply"
            );
            assert_eq!(
                state.selected_comment().map(|comment| comment.id),
                Some(1),
                "selection should snap to the reply's own root, its nearest visible neighbor"
            );
            assert!(
                state.visible_indices().contains(&state.selected),
                "selection must land on a row the current filter shows"
            );
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_caches_fresh_triage_when_ai_review_has_no_return_pane() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    let stale = pr_review_with_comments(1);
    app.db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&stale)
        .unwrap();
    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), stale.pr.clone());

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh {
        workdir: PathBuf::from("/tmp/test-workdir"),
        pr: stale.pr.clone(),
    });
    tx.send(Ok(pr_review_with_comments(3))).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_pr_review_cache(stale.pr.number, &stale.pr.head_sha)
        .unwrap()
        .unwrap();
    assert_eq!(cached.comments.len(), 3);
}

#[test]
fn close_ai_review_with_no_stash_returns_to_the_dashboard() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    assert!(app.ai_review_return_to.is_none());

    app.close_ai_review();

    assert!(matches!(&app.mode, AppMode::Normal));
}

#[test]
fn pr_picker_choose_ai_review_is_noop_outside_picker_mode() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.pr_picker_choose_ai_review();
    assert!(matches!(&app.mode, AppMode::Normal));
}

/// Build a final-review `DiffViewerState` over `paths` (already in the
/// path-sorted order the diff loader produces) and install it as the mode, so
/// tree navigation can be driven through the same `App` methods the handlers
/// call.
#[cfg(test)]
fn enter_review_with_paths(app: &mut App, paths: &[&str]) {
    let mut state = DiffViewerState::new(
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
        std::path::PathBuf::from("/tmp"),
    );
    state.review = true;
    state.files = paths
        .iter()
        .map(|path| crate::diff::DiffFile {
            old_path: Some((*path).into()),
            path: (*path).into(),
            status: crate::diff::DiffFileStatus::Modified,
            additions: 1,
            deletions: 0,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        })
        .collect();
    app.mode = AppMode::DiffViewer(state);
}

#[cfg(test)]
fn tree_rows(app: &App) -> Vec<FileTreeRow> {
    match &app.mode {
        AppMode::DiffViewer(state) => state.file_tree_rows(),
        _ => panic!("not in the diff viewer"),
    }
}

#[cfg(test)]
fn viewer_state(app: &App) -> &DiffViewerState {
    match &app.mode {
        AppMode::DiffViewer(state) => state,
        _ => panic!("not in the diff viewer"),
    }
}

/// A 30-line file with line 15 rewritten, hydrated with the blobs context
/// expansion reads from — the same fixture `crate::diff`'s expansion tests use,
/// built here so the viewer can be driven end to end.
#[cfg(test)]
fn expandable_diff_file() -> crate::diff::DiffFile {
    let old: String = (1..=30).map(|i| format!("l{i}\n")).collect();
    let new = old.replace("l15\n", "l15 changed\n");
    let mut file = crate::diff::parse_unified_diff(
        "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -12,7 +12,7 @@
 l12
 l13
 l14
-l15
+l15 changed
 l16
 l17
 l18
",
    )
    .unwrap()
    .pop()
    .unwrap();
    file.old_content = Some(old);
    file.new_content = Some(new);
    file
}

/// Install a final-review viewer over a single expandable file.
#[cfg(test)]
fn enter_review_with_expandable_file(app: &mut App) {
    enter_review_with_paths(app, &["src/lib.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = vec![expandable_diff_file()];
    }
}

#[test]
fn expanding_context_widens_the_hunk_and_records_the_level() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    let before = viewer_state(&app).files[0].hunks[0].old_lines;
    assert_eq!(app.diff_viewer_context_level(), Some(3));

    app.diff_viewer_expand_context();

    assert_eq!(app.diff_viewer_context_level(), Some(10));
    let state = viewer_state(&app);
    assert!(state.files[0].hunks[0].old_lines > before);
    assert_eq!(state.context_expansion.get("src/lib.rs").copied(), Some(10));
    assert_eq!(app.message.as_deref(), Some("Context: 10 lines"));
}

#[test]
fn the_context_ladder_tops_out_at_the_whole_file_and_returns_to_the_default() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    for _ in 0..4 {
        app.diff_viewer_expand_context();
    }
    assert_eq!(app.diff_viewer_context_level(), Some(usize::MAX));
    assert_eq!(viewer_state(&app).files[0].hunks[0].old_lines, 30);

    // Already at the top: the level holds and the reviewer is told why.
    app.diff_viewer_expand_context();
    assert_eq!(app.diff_viewer_context_level(), Some(usize::MAX));
    assert_eq!(
        app.message.as_deref(),
        Some("Already showing the whole file")
    );

    for _ in 0..4 {
        app.diff_viewer_collapse_context();
    }
    assert_eq!(app.diff_viewer_context_level(), Some(3));
    // Back at the default, the per-file entry is dropped rather than pinned.
    assert!(viewer_state(&app).context_expansion.is_empty());
    assert_eq!(viewer_state(&app).files[0].hunks[0].old_lines, 7);
}

#[test]
fn whole_file_toggle_jumps_both_ways() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    app.diff_viewer_toggle_whole_file_context();
    assert_eq!(app.diff_viewer_context_level(), Some(usize::MAX));

    app.diff_viewer_toggle_whole_file_context();
    assert_eq!(app.diff_viewer_context_level(), Some(3));
}

#[test]
fn expanding_context_keeps_the_line_cursor_on_the_same_diff_line() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    // Park the cursor on the removed line and start a range selection there.
    let removed = viewer_state(&app).files[0]
        .addressable_lines()
        .iter()
        .position(|loc| loc.old_line == Some(15) && loc.new_line.is_none())
        .unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.comment_cursor = Some(removed);
        state.comment_anchor = Some(removed);
    }

    app.diff_viewer_expand_context();

    // The index moved (7 context lines were prepended) but it still points at
    // the same line, which is what a comment would anchor to.
    let state = viewer_state(&app);
    let cursor = state.comment_cursor.unwrap();
    assert_ne!(cursor, removed);
    assert_eq!(
        state.files[0].addressable_lines()[cursor],
        crate::diff::DiffLineLocation {
            old_line: Some(15),
            new_line: None
        }
    );
    assert_eq!(state.comment_anchor, Some(cursor));
    assert!(state.cursor_sync_to_view);
}

#[test]
fn expanding_context_raises_the_patch_scroll_bound() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    app.diff_viewer_scroll_patch_bottom();
    let before = viewer_state(&app).patch_scroll;

    app.diff_viewer_toggle_whole_file_context();
    app.diff_viewer_scroll_patch_bottom();

    // The scroll ceiling is derived from the rendered hunks, not the raw
    // `patch` string, so the expanded rows are actually reachable.
    assert!(
        viewer_state(&app).patch_scroll > before,
        "expanded content must be scrollable"
    );
}

/// The index of the whole-file-only context line `l1`, which narrowing the
/// context back to the default hides again.
#[cfg(test)]
fn whole_file_only_line(app: &App) -> usize {
    viewer_state(app).files[0]
        .addressable_lines()
        .iter()
        .position(|loc| loc.old_line == Some(1))
        .expect("the whole file starts at old line 1")
}

#[test]
fn narrowing_context_refuses_while_the_selected_range_would_be_hidden() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);
    app.diff_viewer_toggle_whole_file_context();

    // A range selection whose start only exists at whole-file context.
    let top = whole_file_only_line(&app);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.comment_cursor = Some(top + 4);
        state.comment_anchor = Some(top);
    }

    app.diff_viewer_toggle_whole_file_context();

    // Refused outright: dropping an endpoint would silently re-point a comment
    // made afterwards at lines the reviewer never selected.
    assert_eq!(app.diff_viewer_context_level(), Some(usize::MAX));
    let state = viewer_state(&app);
    assert_eq!(state.comment_cursor, Some(top + 4));
    assert_eq!(state.comment_anchor, Some(top));
    assert_eq!(
        app.message.as_deref(),
        Some(
            "Selected lines would be hidden at that context level — clear the selection (Esc) first"
        )
    );
}

#[test]
fn narrowing_context_keeps_a_selection_that_stays_visible() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);
    app.diff_viewer_toggle_whole_file_context();

    // The changed line survives at any context level, so the range travels.
    let changed = viewer_state(&app).files[0]
        .addressable_lines()
        .iter()
        .position(|loc| loc.old_line == Some(15) && loc.new_line.is_none())
        .unwrap();
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.comment_cursor = Some(changed);
        state.comment_anchor = Some(changed);
    }

    app.diff_viewer_toggle_whole_file_context();

    assert_eq!(app.diff_viewer_context_level(), Some(3));
    let state = viewer_state(&app);
    let cursor = state.comment_cursor.unwrap();
    assert_eq!(
        state.files[0].addressable_lines()[cursor],
        crate::diff::DiffLineLocation {
            old_line: Some(15),
            new_line: None
        }
    );
    assert_eq!(state.comment_anchor, Some(cursor));
}

#[test]
fn narrowing_context_moves_a_hidden_cursor_to_the_nearest_line_and_reports_it() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);
    app.diff_viewer_toggle_whole_file_context();

    let top = whole_file_only_line(&app);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.comment_cursor = Some(top);
        state.comment_anchor = None;
    }

    app.diff_viewer_toggle_whole_file_context();

    // No selection to protect, so the change goes ahead — but the cursor lands
    // on the nearest line still rendered (old line 12, the hunk's first
    // context line) rather than silently snapping to index 0 of a file whose
    // first rendered line has moved.
    assert_eq!(app.diff_viewer_context_level(), Some(3));
    let state = viewer_state(&app);
    let cursor = state.comment_cursor.unwrap();
    assert_eq!(
        state.files[0].addressable_lines()[cursor],
        crate::diff::DiffLineLocation {
            old_line: Some(12),
            new_line: Some(12)
        }
    );
    assert!(state.cursor_sync_to_view);
    let message = app.message.clone().unwrap();
    assert!(
        message.contains("cursor moved to line 12"),
        "the move must be reported: {message}"
    );
}

#[test]
fn narrowing_context_clamps_the_patch_scroll_into_the_shorter_patch() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    app.diff_viewer_toggle_whole_file_context();
    app.diff_viewer_scroll_patch_bottom();
    let expanded_scroll = viewer_state(&app).patch_scroll;

    app.diff_viewer_toggle_whole_file_context();

    // The patch is short again; leaving the old offset would render a blank
    // panel until the reviewer scrolled back up.
    let max_scroll = app.diff_viewer_patch_line_count().saturating_sub(1);
    let scroll = viewer_state(&app).patch_scroll;
    assert!(
        scroll <= max_scroll,
        "scroll {scroll} is past the new last line {max_scroll}"
    );
    assert!(
        expanded_scroll > max_scroll,
        "the test must actually start out of range: {expanded_scroll} vs {max_scroll}"
    );
}

#[test]
fn context_expansion_survives_a_diff_reload() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);
    app.diff_viewer_expand_context();

    // A refresh replaces `files` with freshly parsed (unexpanded) hunks.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files = vec![expandable_diff_file()];
        assert_eq!(state.files[0].hunks[0].old_lines, 7);
        state.reapply_context_expansion();
    }

    let state = viewer_state(&app);
    assert_eq!(state.files[0].hunks[0].old_lines, 21);
    assert_eq!(state.context_expansion.get("src/lib.rs").copied(), Some(10));
}

#[test]
fn context_expansion_is_dropped_for_a_file_that_left_the_changeset() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);
    app.diff_viewer_expand_context();

    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.files.clear();
        state.reapply_context_expansion();
        assert!(state.context_expansion.is_empty());
    }
}

#[test]
fn toggling_ignore_whitespace_flips_the_flag_and_reloads() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_expandable_file(&mut app);

    app.diff_viewer_toggle_ignore_whitespace();

    // `-w` changes what git emits, so the toggle must go through the loader
    // rather than just re-rendering what's already in memory.
    assert!(
        matches!(&app.mode, AppMode::DiffViewerLoading(s) if s.ignore_whitespace),
        "toggle should flip the flag and enter the loading state"
    );
    assert_eq!(
        app.message.as_deref(),
        Some("Ignoring whitespace-only changes (git diff -w)")
    );
}

#[test]
fn a_file_with_no_surrounding_context_reports_why_instead_of_no_opping() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    // `enter_review_with_paths` builds hunk-less, blob-less files — exactly the
    // added/binary shape that can't be expanded.
    enter_review_with_paths(&mut app, &["src/new.rs"]);

    app.diff_viewer_expand_context();

    assert_eq!(app.diff_viewer_context_level(), Some(3));
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("no surrounding context"),
        "got {:?}",
        app.message
    );
}

#[test]
fn ancestor_dirs_lists_each_level_shallowest_first() {
    assert_eq!(ancestor_dirs("README.md"), Vec::<String>::new());
    assert_eq!(ancestor_dirs("src/main.rs"), vec!["src".to_string()]);
    assert_eq!(
        ancestor_dirs("src/app/dialogs/diff.rs"),
        vec![
            "src".to_string(),
            "src/app".to_string(),
            "src/app/dialogs".to_string()
        ]
    );
}

#[test]
fn file_tree_rows_group_by_directory_without_reordering_the_file_list() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    // Exactly the order `crate::diff` produces (files sorted by full path).
    enter_review_with_paths(
        &mut app,
        &[
            "README.md",
            "src/a.rs",
            "src/app/mod.rs",
            "src/app/state.rs",
        ],
    );

    let rows = tree_rows(&app);
    let described: Vec<String> = rows
        .iter()
        .map(|row| match row {
            FileTreeRow::Dir { path, depth, .. } => format!("dir {path} @{depth}"),
            FileTreeRow::File { index, depth, name } => format!("file {name} #{index} @{depth}"),
        })
        .collect();
    assert_eq!(
        described,
        vec![
            "file README.md #0 @0",
            "dir src @0",
            "file a.rs #1 @1",
            "dir src/app @1",
            "file mod.rs #2 @2",
            "file state.rs #3 @2",
        ]
    );
    // File rows keep the original `files` order, so n/p and the tree agree.
    let file_indices: Vec<usize> = rows
        .iter()
        .filter_map(|row| match row {
            FileTreeRow::File { index, .. } => Some(*index),
            FileTreeRow::Dir { .. } => None,
        })
        .collect();
    assert_eq!(file_indices, vec![0, 1, 2, 3]);
}

#[test]
fn collapsing_a_directory_hides_its_files_but_not_from_filters() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(
        &mut app,
        &["src/a.rs", "src/app/mod.rs", "src/app/state.rs"],
    );

    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.toggle_dir_collapsed("src/app");
    }

    let rows = tree_rows(&app);
    assert!(
        matches!(&rows[2], FileTreeRow::Dir { path, collapsed, files, .. }
            if path == "src/app" && *collapsed && *files == 2),
        "collapsed directory row should summarise the two files it hides: {rows:?}"
    );
    assert_eq!(
        rows.iter()
            .filter(|row| matches!(row, FileTreeRow::File { .. }))
            .count(),
        1,
        "only the file outside the collapsed directory should have a row"
    );
    // Folding is a view concern only: filters and counts still see every file.
    assert_eq!(viewer_state(&app).visible_file_indices(), vec![0, 1, 2]);
}

#[test]
fn file_order_navigation_reveals_a_file_inside_a_collapsed_directory() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["src/a.rs", "src/app/mod.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.toggle_dir_collapsed("src/app");
    }

    // n (file-order navigation) must never strand the selection behind a fold.
    app.diff_viewer_select_next_file();

    let state = viewer_state(&app);
    assert_eq!(state.selected_file, 1);
    assert!(state.collapsed_dirs.is_empty());
    assert!(state.tree_cursor_dir.is_none());
}

#[test]
fn tree_move_walks_directory_rows_and_leaves_the_patch_alone() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["README.md", "src/a.rs"]);

    // Row 0 is README.md; row 1 is the `src` directory header.
    app.diff_viewer_tree_move(1);
    let state = viewer_state(&app);
    assert_eq!(state.tree_cursor_dir.as_deref(), Some("src"));
    assert_eq!(
        state.selected_file, 0,
        "parking on a directory must not change which file the patch shows"
    );

    // Folding from the directory row keeps the cursor there.
    app.diff_viewer_tree_toggle_collapsed();
    let state = viewer_state(&app);
    assert!(state.collapsed_dirs.contains("src"));
    assert_eq!(state.tree_cursor_dir.as_deref(), Some("src"));

    // Stepping down onto a file selects it and drops the directory cursor.
    app.diff_viewer_tree_expand();
    app.diff_viewer_tree_move(1);
    let state = viewer_state(&app);
    assert_eq!(state.selected_file, 1);
    assert!(state.tree_cursor_dir.is_none());
}

#[test]
fn toggle_all_folds_every_directory_then_unfolds_them() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["src/a.rs", "src/app/mod.rs"]);

    app.diff_viewer_tree_toggle_all();
    let state = viewer_state(&app);
    assert!(state.collapsed_dirs.contains("src"));
    assert!(state.collapsed_dirs.contains("src/app"));
    // The selection is folded away, so the cursor parks on its outermost dir
    // and the list still highlights a row.
    assert_eq!(state.tree_cursor_dir.as_deref(), Some("src"));
    let rows = state.file_tree_rows();
    assert_eq!(
        rows.len(),
        1,
        "everything should fold into one row: {rows:?}"
    );
    assert!(state.tree_cursor_row(&rows).is_some());

    app.diff_viewer_tree_toggle_all();
    assert!(viewer_state(&app).collapsed_dirs.is_empty());
}

#[test]
fn tree_cursor_row_falls_back_to_the_deepest_visible_ancestor() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["src/app/mod.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        // Fold the inner directory directly, without moving the selection.
        state.toggle_dir_collapsed("src/app");
    }

    let state = viewer_state(&app);
    let rows = state.file_tree_rows();
    let cursor = state
        .tree_cursor_row(&rows)
        .expect("a row stays highlighted");
    assert!(
        matches!(&rows[cursor], FileTreeRow::Dir { path, .. } if path == "src/app"),
        "the highlight should fall back to the fold hiding the selected file"
    );
}

#[test]
fn tree_move_enters_the_list_when_the_filter_hides_the_selection() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["README.md", "src/a.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        // Select a file the Undecided filter then drops, leaving a single
        // root-level row and nothing highlighted.
        state.selected_file = 1;
        state
            .decisions
            .insert("src/a.rs".to_string(), crate::app::ReviewDecision::Approve);
        state.file_filter = crate::app::FileFilter::Undecided;
    }
    assert!(
        viewer_state(&app)
            .tree_cursor_row(&tree_rows(&app))
            .is_none(),
        "the test needs a state with no highlighted row"
    );

    // j must reach the only row rather than stepping past it.
    app.diff_viewer_tree_move(1);
    assert_eq!(viewer_state(&app).selected_file, 0);

    // ...and so must k, from the other end.
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 1;
    }
    app.diff_viewer_tree_move(-1);
    assert_eq!(viewer_state(&app).selected_file, 0);
}

#[test]
fn tree_toggle_folds_the_highlighted_row_not_the_hidden_selections_directory() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_review_with_paths(&mut app, &["src/a.rs", "src/app/mod.rs"]);
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.selected_file = 1;
        state.decisions.insert(
            "src/app/mod.rs".to_string(),
            crate::app::ReviewDecision::Approve,
        );
        state.file_filter = crate::app::FileFilter::Undecided;
    }
    // With src/app/mod.rs filtered out, `src/app` has no row and the highlight
    // falls back to `src`.
    let rows = tree_rows(&app);
    let cursor = viewer_state(&app)
        .tree_cursor_row(&rows)
        .expect("a row stays highlighted");
    assert!(matches!(&rows[cursor], FileTreeRow::Dir { path, .. } if path == "src"));

    app.diff_viewer_tree_toggle_collapsed();

    let state = viewer_state(&app);
    assert!(
        state.collapsed_dirs.contains("src"),
        "z should fold the directory the list highlights"
    );
    assert!(
        !state.collapsed_dirs.contains("src/app"),
        "z must not fold a directory that has no row: {:?}",
        state.collapsed_dirs
    );
    assert_eq!(state.tree_cursor_dir.as_deref(), Some("src"));
}

// ---------------------------------------------------------------------------
// PR Triage: the `New feature…` fix target (companion triage feature)
// ---------------------------------------------------------------------------

/// An app whose single feature lives at `/tmp/test-workdir`, with a worktree
/// mock that resolves the repo root — enough for the fix-target picker and the
/// triage-feature setup overlay, neither of which touches git.
fn triage_target_app(store: ProjectStore) -> App {
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree))
}

fn triage_setup(app: &App) -> &crate::app::TriageFeatureSetupState {
    match &app.mode {
        AppMode::PrReview(state) => state
            .new_feature_setup
            .as_ref()
            .expect("triage-feature setup overlay should be open"),
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

/// Move the fix-target picker's highlight onto the `New feature…` row.
fn select_new_feature_row(app: &mut App) {
    if let AppMode::PrReview(state) = &mut app.mode {
        let pick = state
            .harness_pick
            .as_mut()
            .expect("fix-target picker should be open");
        pick.selected = pick
            .rows
            .iter()
            .position(|r| matches!(r, crate::app::pr_review::FixTargetPickRow::NewFeature))
            .expect("New feature… row should be present");
    }
}

#[test]
fn pr_review_fix_target_picker_offers_new_feature_last() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 2);

    app.pr_review_open_fix_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            let pick = state.harness_pick.as_ref().expect("picker should be open");
            assert_eq!(
                pick.rows.last(),
                Some(&crate::app::pr_review::FixTargetPickRow::NewFeature),
                "the isolated option is last so it reads as the deliberate choice"
            );
            // …and it is not what the cursor lands on by default.
            assert_ne!(pick.selected, pick.rows.len() - 1);
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn choosing_new_feature_opens_setup_instead_of_the_fix_confirm() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 2);

    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.harness_pick.is_none(), "picker closes");
            assert!(
                state.fix_confirm.is_none(),
                "nothing is injected until the feature exists"
            );
            assert!(state.new_feature_setup.is_some(), "setup overlay opens");
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    // The branch is pre-filled from the PR's head branch, and is deliberately
    // NOT that branch — git can't check one branch out in two worktrees.
    let setup = triage_setup(&app);
    assert_eq!(setup.branch, "main-triage");
    assert_ne!(setup.branch, "main");
}

#[test]
fn triage_feature_records_the_prs_own_branch_not_the_checked_out_one() {
    // The documented "other PR" flow triages a PR whose head branch is not what
    // the feature has checked out. `TriageSource::pr_branch` is used verbatim as
    // the `git push origin <triage>:<dest>` destination, so reading it off the
    // source feature would push review fixes onto an unrelated remote branch.
    let repo_dir = TempDir::new().unwrap();
    let repo = repo_dir.path().to_path_buf();
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
    };
    git(&repo, &["init", "-q", "-b", "my-feat"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "base"]);
    // The PR lives on its own branch, one commit ahead of what's checked out —
    // so `my-feat` does not even contain the PR head.
    git(&repo, &["checkout", "-q", "-b", "pr-head"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "pr work"]);
    let head_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    git(&repo, &["checkout", "-q", "my-feat"]);

    let triage_workdir = repo.join(".worktrees").join("pr-head-triage");
    std::fs::create_dir_all(&triage_workdir).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_for_root = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_for_root.clone()));
    let triage_workdir_clone = triage_workdir.clone();
    worktree
        .expect_create_from()
        .times(1)
        // The worktree is branched off the PR's branch, not the checkout's.
        .withf(|_, _, branch, base| branch == "pr-head-triage" && base == "pr-head")
        .returning(move |_, _, _, _| Ok(triage_workdir_clone.clone()));

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| false);
    tmux.expect_create_session_with_window()
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].repo = repo.clone();
    store.projects[0].features[0].workdir = repo.clone();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(worktree));
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();

    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo.clone();
        state.review.pr.head_ref = "pr-head".to_string();
        state.review.pr.head_sha = head_sha.clone();
        state.checked_out_branch = Some("my-feat".to_string());
    }

    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();
    // The overlay pre-fills from the PR's branch, and the created feature has
    // to agree with it.
    assert_eq!(triage_setup(&app).branch, "pr-head-triage");
    app.pr_review_triage_setup_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => assert!(
            state.new_feature_setup.is_none(),
            "creation failed: {:?}",
            state
                .new_feature_setup
                .as_ref()
                .and_then(|s| s.error.clone())
        ),
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    let companion = app.store.projects[0]
        .features
        .iter()
        .find(|f| f.name == "pr-head-triage")
        .expect("the companion feature should have been created");
    let link = companion
        .triage_source
        .as_ref()
        .expect("the companion is linked back to the PR");
    assert_eq!(
        link.pr_branch, "pr-head",
        "fixes land on the PR's branch, not the source feature's `my-feat`"
    );
    assert_eq!(link.source_feature_id, "feat-1");
}

#[test]
fn triage_setup_branch_is_deduplicated_against_existing_features() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    // A previous triage feature for this branch already claimed the name.
    let mut earlier = store.projects[0].features[0].clone();
    earlier.id = "feat-2".to_string();
    earlier.name = "main-triage".to_string();
    earlier.workdir = PathBuf::from("/tmp/test-workdir-triage");
    store.projects[0].features.push(earlier);

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    assert_eq!(triage_setup(&app).branch, "main-triage-2");
}

#[test]
fn triage_setup_rows_cycle_their_values() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    // Row 0 is Preset; move to Mode and cycle it. This is the setting the
    // whole target exists for — triaging in a different mode than the PR was
    // built in.
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibeless);
    app.pr_review_triage_setup_move(2);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Mode
    );
    app.pr_review_triage_setup_adjust(1);
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibe);
    app.pr_review_triage_setup_adjust(-1);
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibeless);

    // Booleans toggle regardless of direction.
    app.pr_review_triage_setup_move(1);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Review
    );
    assert!(!triage_setup(&app).review);
    app.pr_review_triage_setup_adjust(1);
    assert!(triage_setup(&app).review);

    // Row movement wraps: from Review (index 3), -4 lands on Branch (index 5).
    app.pr_review_triage_setup_move(-4);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Branch
    );
}

#[test]
fn triage_setup_branch_row_takes_typed_input() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    // Not on the branch row: typing is not routed here at all.
    assert!(!app.pr_review_triage_setup_on_branch_row());

    app.pr_review_triage_setup_move(5);
    assert!(app.pr_review_triage_setup_on_branch_row());
    app.pr_review_triage_setup_branch_backspace();
    app.pr_review_triage_setup_branch_push('X');
    assert_eq!(triage_setup(&app).branch, "main-triagX");

    // Cycling is a no-op on the text row (nothing to cycle through).
    app.pr_review_triage_setup_adjust(1);
    assert_eq!(triage_setup(&app).branch, "main-triagX");
}

#[test]
fn triage_setup_preset_fills_in_the_rows_below_it() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];
    let mut app = triage_target_app(store);
    app.active_extension.feature_presets = vec![crate::extension::FeaturePreset {
        name: "Careful triage".to_string(),
        branch_prefix: Some("triage/".to_string()),
        mode: VibeMode::Vibeless,
        agent: AgentKind::Codex,
        review: true,
        plan_mode: true,
        enable_chrome: true,
        remote_control: false,
    }];
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    assert_eq!(triage_setup(&app).preset_label(), "Manual");
    // Row 0 is the preset row; cycling forward selects the only preset.
    app.pr_review_triage_setup_adjust(1);

    let setup = triage_setup(&app);
    assert_eq!(setup.preset_label(), "Careful triage");
    assert_eq!(setup.agent(), AgentKind::Codex);
    assert!(setup.review);
    assert!(setup.enable_chrome);
    assert_eq!(
        setup.branch, "triage/main-triage",
        "the preset's branch prefix applies to the companion branch too"
    );
}

#[test]
fn cancelling_triage_setup_leaves_the_fix_target_unresolved() {
    // Backing out of the setup means no target was chosen — the next `f` must
    // re-offer every option rather than silently using the default.
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    app.pr_review_triage_setup_cancel();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.new_feature_setup.is_none());
            assert!(state.fix_confirm.is_none());
            assert!(!state.fix_target_picked);
            assert!(!state.pending_batch);
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_some()),
        "the picker is offered again"
    );
}

/// Attach a companion triage feature for PR #7 to the store, linked back to
/// the source feature the way `create_triage_feature` persists it.
fn push_companion_feature(store: &mut ProjectStore, agent: AgentKind, mode: VibeMode) {
    let source_id = store.projects[0].features[0].id.clone();
    let mut companion = store.projects[0].features[0].clone();
    companion.id = "feat-triage".to_string();
    companion.name = "main-triage".to_string();
    companion.branch = "main-triage".to_string();
    companion.workdir = PathBuf::from("/tmp/test-workdir/.worktrees/main-triage");
    companion.is_worktree = true;
    companion.agent = agent;
    companion.mode = mode;
    companion.sessions = vec![];
    companion.add_session_named(SessionKind::Claude, "PR Triage".to_string());
    companion.triage_source = Some(crate::project::TriageSource {
        pr_number: 7,
        source_feature_id: source_id,
        // The PR's head branch (`pr_review_with_comments` puts the PR on
        // `main`), not the source feature's own branch — that's what
        // `create_triage_feature` records, and it's the push destination.
        pr_branch: "main".to_string(),
        base_sha: "basesha".to_string(),
    });
    store.projects[0].features.push(companion);
}

#[test]
fn companion_target_resolves_sessions_in_the_triage_feature() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    // A same-named session in the *source* feature must not be mistaken for
    // the companion's — that would inject fixes into the wrong worktree.
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibeless);

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }

    assert_eq!(
        app.pr_review_target_feature(),
        Some((0, 1)),
        "the companion feature, not the source one"
    );
}

#[test]
fn opening_a_pr_with_an_existing_companion_adopts_it() {
    // The companion is reused for every fix in the PR, across pane re-opens
    // and restarts — so entering the pane must not re-ask for a target.
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);

    app.adopt_existing_triage_feature();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.fix_target,
                crate::app::pr_review::FixTarget::NewFeature
            );
            assert!(state.fix_target_picked, "no re-ask on the next fix");
            assert_eq!(state.review_harness, Some(AgentKind::Codex));
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_none()
            && state.new_feature_setup.is_none()
            && state.fix_confirm.is_some()),
        "the fix goes straight to the confirm dialog in the adopted feature"
    );
}

#[test]
fn adopt_is_a_no_op_without_a_companion_for_this_pr() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    // Same feature, different PR: the link is keyed on the PR number too.
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .pr_number = 99;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if !state.fix_target_picked),
        "an unrelated PR's triage feature is not adopted"
    );
}

#[test]
fn a_vanished_companion_really_does_reopen_the_picker() {
    // The error text promises "press f and pick a target again", so the pane
    // must actually un-resolve the target — both `fix_target_picked` and
    // `review_harness` short-circuit the picker.
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    // The companion is deleted mid-visit (from the dashboard, say).
    app.store.projects[0].features.remove(1);
    app.pr_review_inject_fix().unwrap();

    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("no longer exists")),
        "the pane reports it: {:?}",
        app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(!state.fix_target_picked);
            assert!(
                state.review_harness.is_none(),
                "the dead harness is dropped"
            );
        }
        other => panic!(
            "the pane stays open so `f` is still reachable, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_some()),
        "`f` re-offers every target instead of resolving against the dead one"
    );
}

#[test]
fn fix_confirm_names_the_companion_feature_and_its_mode() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibeless);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    assert_eq!(
        app.pr_review_triage_feature_summary().as_deref(),
        Some("main-triage · Codex · Vibeless")
    );

    // The in-feature targets say nothing extra — they run where the user
    // already is.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_target = crate::app::pr_review::FixTarget::DedicatedReview;
    }
    assert!(app.pr_review_triage_feature_summary().is_none());
}

#[test]
fn integrate_is_rejected_for_the_in_feature_targets() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_open_integrate();

    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.integrate.is_none()),
        "the dedicated/live targets already commit on the PR branch"
    );
    assert!(!app.toasts.is_empty(), "the user is told why");
}

#[test]
fn integrate_reports_the_commits_and_blocks_cherry_pick_on_a_dirty_source() {
    use crate::app::TriageIntegration;

    // Real git: a source repo with a commit, and a companion worktree branched
    // from it carrying one triage commit.
    let repo = TempDir::new().unwrap();
    let source = repo.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
    };
    git(&source, &["init", "-q", "-b", "main"]);
    git(&source, &["config", "user.email", "t@example.com"]);
    git(&source, &["config", "user.name", "T"]);
    std::fs::write(source.join("a.txt"), "one\n").unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "base"]);
    let base_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&source)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let triage = repo.path().join("triage");
    git(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            "main-triage",
            triage.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(triage.join("a.txt"), "one\ntwo\n").unwrap();
    git(&triage, &["add", "-A"]);
    git(&triage, &["commit", "-qm", "apply review comment"]);

    // Dirty the source worktree — the cherry-pick must refuse rather than
    // clobber in-progress work.
    std::fs::write(source.join("b.txt"), "wip\n").unwrap();

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].workdir = source.clone();
    store.projects[0].features[0].branch = "main".to_string();
    push_companion_feature(&mut store, AgentKind::Claude, VibeMode::Vibeless);
    store.projects[0].features[1].workdir = triage.clone();
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .base_sha = base_sha;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = source.clone();
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }

    app.pr_review_open_integrate();

    let (commits, source_dirty) = match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().expect("overlay should be open");
            assert_eq!(integrate.triage_branch, "main-triage");
            assert_eq!(integrate.pr_branch, "main");
            (integrate.commits.clone(), integrate.source_dirty.clone())
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    };
    assert_eq!(commits.len(), 1, "one triage commit to land");
    assert!(commits[0].contains("apply review comment"));
    assert!(
        source_dirty.is_some(),
        "the dirty source worktree disables the cherry-pick"
    );

    // Confirming the cherry-pick refuses instead of running it, and leaves the
    // source worktree exactly as it was.
    if let AppMode::PrReview(state) = &mut app.mode {
        let integrate = state.integrate.as_mut().unwrap();
        integrate.selected = TriageIntegration::ALL
            .iter()
            .position(|o| *o == TriageIntegration::CherryPick)
            .unwrap();
    }
    app.pr_review_integrate_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().unwrap();
            assert!(
                integrate
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("Cherry-pick refused")),
                "got {:?}",
                integrate.error
            );
            assert!(integrate.done.is_none());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert_eq!(
        std::fs::read_to_string(source.join("a.txt")).unwrap(),
        "one\n",
        "the source worktree is untouched"
    );
}

#[test]
fn integrate_cherry_picks_into_a_clean_source_worktree() {
    let repo = TempDir::new().unwrap();
    let source = repo.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
    };
    git(&source, &["init", "-q", "-b", "main"]);
    git(&source, &["config", "user.email", "t@example.com"]);
    git(&source, &["config", "user.name", "T"]);
    std::fs::write(source.join("a.txt"), "one\n").unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "base"]);
    let base_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&source)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let triage = repo.path().join("triage");
    git(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            "main-triage",
            triage.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(triage.join("b.txt"), "fix\n").unwrap();
    git(&triage, &["add", "-A"]);
    git(&triage, &["commit", "-qm", "apply review comment"]);

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].workdir = source.clone();
    store.projects[0].features[0].branch = "main".to_string();
    push_companion_feature(&mut store, AgentKind::Claude, VibeMode::Vibeless);
    store.projects[0].features[1].workdir = triage.clone();
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .base_sha = base_sha;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = source.clone();
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }
    app.pr_review_open_integrate();
    if let AppMode::PrReview(state) = &mut app.mode {
        let integrate = state.integrate.as_mut().unwrap();
        assert!(integrate.source_dirty.is_none());
        integrate.selected = crate::app::TriageIntegration::ALL
            .iter()
            .position(|o| *o == crate::app::TriageIntegration::CherryPick)
            .unwrap();
    }

    app.pr_review_integrate_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().unwrap();
            assert!(integrate.error.is_none(), "got {:?}", integrate.error);
            assert!(
                integrate
                    .done
                    .as_deref()
                    .is_some_and(|d| d.contains("Cherry-picked 1 commit")),
                "got {:?}",
                integrate.done
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(
        source.join("b.txt").exists(),
        "the triage commit landed in the source worktree"
    );
}

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

/// A real shell with a real child, standing in for a tmux pane that has a
/// harness running in it. The census asks `ps` whether the pane's shell has
/// anything under it, so a made-up pid would read as an idle prompt — which is
/// exactly the distinction being tested elsewhere.
struct BusyPane(std::process::Child);

impl Drop for BusyPane {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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

// ---------------------------------------------------------------------------
// Editor reclamation on stop
// ---------------------------------------------------------------------------

/// Stand-in for a VS Code window AMF opened: a copy of `sh` named `code`,
/// launched with a VS Code-shaped argv, holding a child of its own the way a
/// real window holds a language server.
fn spawn_fake_editor(dir: &std::path::Path, workdir: &std::path::Path) -> std::process::Child {
    let fake = dir.join("code");
    std::fs::copy("/bin/sh", &fake).expect("copy sh");
    spawn_stand_in(std::process::Command::new(&fake).args([
        "-c".as_ref(),
        "sleep 60 & wait".as_ref(),
        "--new-window".as_ref(),
        workdir.as_os_str(),
    ]))
}

/// Spawn a stand-in binary, retrying `ETXTBSY`.
///
/// These helpers copy `/bin/sh` and immediately execute the copy; a concurrent
/// test forking in that window inherits the write descriptor and makes the exec
/// fail with "text file busy". It is an artefact of the fixture, not of
/// anything under test.
fn spawn_stand_in(command: &mut std::process::Command) -> std::process::Child {
    command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for _ in 0..40 {
        match command.spawn() {
            Ok(child) => return child,
            Err(err) if err.raw_os_error() == Some(26) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(err) => panic!("stand-in should launch: {err}"),
        }
    }
    panic!("stand-in stayed busy");
}

/// The same stand-in, holding `windows` renderer subprocesses — how a real VS
/// Code instance carries the windows open inside it.
fn spawn_fake_editor_hosting_windows(
    dir: &std::path::Path,
    workdir: &std::path::Path,
    windows: usize,
) -> std::process::Child {
    let fake = dir.join("code");
    std::fs::copy("/bin/sh", &fake).expect("copy sh");
    // The renderers are spawned from a script file rather than an inline `-c`
    // string: an inline one would put `--type=renderer` in the *parent's* argv,
    // which is exactly what marks a process as a helper rather than a window.
    let mut script = String::new();
    for id in 1..=windows {
        script.push_str(&format!(
            "{} -c 'sleep 60' --type=renderer --window-id={id} &\n",
            fake.display()
        ));
    }
    script.push_str("wait\n");
    let script_path = dir.join("windows.sh");
    std::fs::write(&script_path, script).expect("write window script");

    spawn_stand_in(std::process::Command::new(&fake).args([
        script_path.as_os_str(),
        "--new-window".as_ref(),
        workdir.as_os_str(),
    ]))
}

/// An app whose feature lives in `workdir`, with a temp DB attached.
fn app_with_db(workdir: &std::path::Path, db_file: &NamedTempFile) -> App {
    let mut store = store_with_feature(ProjectStatus::Idle);
    store.projects[0].features[0].workdir = workdir.to_path_buf();

    let mut tmux = MockTmuxOps::new();
    tmux.expect_kill_session().returning(|_| Ok(()));

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    app.selection = Selection::Feature(0, 0);
    app
}

#[test]
fn stopping_a_feature_closes_the_editor_amf_opened() {
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor(tmp.path(), &workdir);
    // Let the shell fork the child that stands in for a language server.
    std::thread::sleep(std::time::Duration::from_millis(300));
    let editor_pid = editor.id() as i64;
    let tree = crate::resources::procs::process_tree(
        &crate::resources::procs::list_processes(),
        editor_pid,
    );
    assert!(tree.len() > 1, "expected a child process, got {tree:?}");

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();

    app.stop_feature().unwrap();
    let _ = editor.wait();

    for pid in &tree {
        assert!(
            !crate::resources::procs::pid_alive(*pid),
            "pid {pid} survived the stop"
        );
    }
    // The record is dropped along with the process it described.
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .launched_editors_for_feature("feat-1")
            .unwrap()
            .is_empty()
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or_default()
            .contains("closed 1 editor"),
        "got {:?}",
        app.message
    );
}

#[test]
fn kill_editor_on_stop_opt_out_leaves_the_editor_running() {
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor(tmp.path(), &workdir);
    let editor_pid = editor.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    app.config.kill_editor_on_stop = false;
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();

    app.stop_feature().unwrap();

    assert!(
        crate::resources::procs::pid_alive(editor_pid),
        "the opt-out must bypass the kill entirely"
    );
    // And the record survives, since nothing was resolved.
    assert_eq!(
        app.db
            .as_ref()
            .unwrap()
            .launched_editors_for_feature("feat-1")
            .unwrap()
            .len(),
        1
    );

    let _ = editor.kill();
    let _ = editor.wait();
}

#[test]
fn an_editor_amf_did_not_open_is_reported_not_killed() {
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor(tmp.path(), &workdir);
    let editor_pid = editor.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    // `dedicated: false` — the folder was handed to a window the user opened.
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            false,
            "code",
        )
        .unwrap();

    let report = app.kill_tracked_editors("feat-1");

    assert!(report.killed.is_empty());
    assert!(crate::resources::procs::pid_alive(editor_pid));
    assert_eq!(
        report.summary().as_deref(),
        Some("left VS Code running (AMF did not open this window)")
    );

    let _ = editor.kill();
    let _ = editor.wait();
}

#[test]
fn a_recycled_pid_is_never_signalled() {
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();

    // A live process that is emphatically not the recorded editor: if identity
    // revalidation were skipped, this would be killed.
    let mut bystander = std::process::Command::new("sleep")
        .arg("60")
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let bystander_pid = bystander.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            bystander_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();

    let report = app.kill_tracked_editors("feat-1");

    assert!(report.killed.is_empty());
    assert_eq!(
        report.skipped,
        vec![(
            "VS Code".to_string(),
            crate::app::editor_ops::SkipReason::PidRecycled
        )]
    );
    assert!(
        crate::resources::procs::pid_alive(bystander_pid),
        "an unrelated process must never be signalled"
    );

    let _ = bystander.kill();
    let _ = bystander.wait();
}

#[test]
fn a_window_sharing_its_instance_with_others_is_left_alone() {
    // AMF's launch started VS Code, so it owns the process — but the user has
    // since opened other windows in it. Killing the process would close their
    // work, which no amount of memory is worth.
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor_hosting_windows(tmp.path(), &workdir, 2);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let editor_pid = editor.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();

    let report = app.kill_tracked_editors("feat-1");

    assert!(report.killed.is_empty());
    assert_eq!(
        report.skipped,
        vec![(
            "VS Code".to_string(),
            crate::app::editor_ops::SkipReason::SharedInstance
        )]
    );
    assert!(
        crate::resources::procs::pid_alive(editor_pid),
        "a shared instance must survive the stop"
    );

    let _ = editor.kill();
    let _ = editor.wait();
}

#[test]
fn a_window_of_its_own_is_still_closed() {
    // The counterpart to the test above: one window in the instance is exactly
    // the case ownership was resolved for, and it must not be skipped.
    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor_hosting_windows(tmp.path(), &workdir, 1);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let editor_pid = editor.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    app.db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();

    let report = app.kill_tracked_editors("feat-1");
    let _ = editor.wait();

    assert_eq!(report.killed.len(), 1);
    assert!(!crate::resources::procs::pid_alive(editor_pid));
}

#[test]
fn stopping_while_the_window_is_still_opening_hands_it_to_the_resolver() {
    use crate::app::editor_ops::{PendingEditorLaunch, PendingLaunchState, lock_state};

    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    // The state a launch is in for the first seconds: recorded, not yet owned.
    let record = app
        .db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            0,
            &workdir,
            false,
            "code --new-window",
        )
        .unwrap();
    let state = std::sync::Arc::new(std::sync::Mutex::new(PendingLaunchState::Resolving));
    app.pending_editor_launches.push(PendingEditorLaunch {
        feature_id: "feat-1".to_string(),
        record_id: record.id.clone(),
        kind: crate::db::editors::EditorKind::Vscode,
        state: state.clone(),
    });

    let report = app.kill_tracked_editors("feat-1");

    assert_eq!(report.pending, vec!["VS Code".to_string()]);
    assert!(
        report.skipped.is_empty(),
        "a window still opening is not a window AMF does not own, got {:?}",
        report.skipped
    );
    assert_eq!(
        *lock_state(&state),
        PendingLaunchState::Reclaim,
        "the resolver must be told to close the window it finds"
    );
    assert_eq!(
        report.summary().as_deref(),
        Some("VS Code will close once its window opens")
    );
}

#[test]
fn a_launch_that_resolved_before_the_stop_is_closed_by_the_stop() {
    // The other side of the race: the resolver won, so the row is owned and the
    // stop kills it directly. The pending entry must not swallow it.
    use crate::app::editor_ops::{PendingEditorLaunch, PendingLaunchState};

    let tmp = TempDir::new().unwrap();
    let workdir = tmp.path().join("worktree");
    std::fs::create_dir_all(&workdir).unwrap();
    let mut editor = spawn_fake_editor(tmp.path(), &workdir);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let editor_pid = editor.id() as i64;

    let db_file = NamedTempFile::new().unwrap();
    let mut app = app_with_db(&workdir, &db_file);
    let record = app
        .db
        .as_ref()
        .unwrap()
        .record_launched_editor(
            "feat-1",
            None,
            crate::db::editors::EditorKind::Vscode,
            editor_pid,
            &workdir,
            true,
            "code --new-window",
        )
        .unwrap();
    app.pending_editor_launches.push(PendingEditorLaunch {
        feature_id: "feat-1".to_string(),
        record_id: record.id,
        kind: crate::db::editors::EditorKind::Vscode,
        state: std::sync::Arc::new(std::sync::Mutex::new(PendingLaunchState::Done)),
    });

    let report = app.kill_tracked_editors("feat-1");
    let _ = editor.wait();

    assert!(report.pending.is_empty());
    assert_eq!(report.killed.len(), 1);
    assert!(!crate::resources::procs::pid_alive(editor_pid));
    assert!(
        app.pending_editor_launches.is_empty(),
        "a resolved launch should be pruned"
    );
}
