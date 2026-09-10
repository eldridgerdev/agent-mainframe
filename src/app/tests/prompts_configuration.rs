use super::support::*;
use crate::app::steering::PromptConstraint;
use crate::app::*;
use crate::project::AgentKind;
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashMap;
use tempfile::NamedTempFile;

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
fn app_config_waiting_stale_minutes_defaults_to_thirty() {
    assert_eq!(AppConfig::default().waiting_stale_minutes, 30);
    assert_eq!(
        AppConfig::default().waiting_stale_threshold(),
        Some(std::time::Duration::from_secs(30 * 60))
    );
}

#[test]
fn app_config_missing_waiting_stale_minutes_uses_default() {
    // Configs written before the key existed must keep loading.
    let config: AppConfig = serde_json::from_str(r#"{"nerd_font":false}"#).unwrap();
    assert_eq!(
        config.waiting_stale_threshold(),
        Some(std::time::Duration::from_secs(30 * 60))
    );
}

#[test]
fn app_config_waiting_stale_minutes_is_configurable_and_zero_disables() {
    let config: AppConfig = serde_json::from_str(r#"{"waiting_stale_minutes":5}"#).unwrap();
    assert_eq!(
        config.waiting_stale_threshold(),
        Some(std::time::Duration::from_secs(5 * 60))
    );

    let disabled: AppConfig = serde_json::from_str(r#"{"waiting_stale_minutes":0}"#).unwrap();
    assert_eq!(disabled.waiting_stale_threshold(), None);
}

#[test]
fn app_config_default_diff_review_viewer_is_amf() {
    let config = AppConfig::default();
    assert_eq!(config.diff_review_viewer, DiffReviewViewer::Amf);
}

#[test]
fn app_config_context_defaults_match_the_hardcoded_fallbacks() {
    let config = AppConfig::default();
    assert_eq!(config.context_window_override, None);
    assert_eq!(
        config.context_warning_percent,
        crate::context_tracking::DEFAULT_CONTEXT_WARNING_PERCENT
    );
    assert_eq!(
        config.context_critical_percent,
        crate::context_tracking::DEFAULT_CONTEXT_CRITICAL_PERCENT
    );
}

#[test]
fn app_config_missing_context_keys_use_defaults() {
    // Configs written before these keys existed must keep loading.
    let config: AppConfig = serde_json::from_str(r#"{"nerd_font":false}"#).unwrap();
    assert_eq!(config.context_window_override, None);
    assert_eq!(config.context_warning_percent, 70);
    assert_eq!(config.context_critical_percent, 85);
}

#[test]
fn app_config_context_values_are_configurable() {
    let config: AppConfig = serde_json::from_str(
        r#"{"context_window_override":500000,"context_warning_percent":50,"context_critical_percent":75}"#,
    )
    .unwrap();
    assert_eq!(config.context_window_override, Some(500_000));
    assert_eq!(config.context_warning_percent, 50);
    assert_eq!(config.context_critical_percent, 75);
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

// ── ContextSettings dialog ─────────────────────────────────

fn context_settings_test_app() -> App {
    App::new_for_test(
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
    )
}

#[test]
fn context_settings_opens_prefilled_with_current_config() {
    let mut app = context_settings_test_app();
    app.config.context_window_override = Some(500_000);
    app.config.context_warning_percent = 60;
    app.config.context_critical_percent = 90;

    app.start_context_settings();

    let AppMode::ContextSettings(state) = &app.mode else {
        panic!("expected ContextSettings mode");
    };
    assert_eq!(state.window_limit_input, "500000");
    assert_eq!(state.warning_input, "60");
    assert_eq!(state.critical_input, "90");
    assert_eq!(state.field, crate::app::ContextSettingsField::WindowLimit);
}

#[test]
fn context_settings_confirm_persists_valid_values() {
    let mut app = context_settings_test_app();
    app.start_context_settings();

    let AppMode::ContextSettings(state) = &mut app.mode else {
        panic!("expected ContextSettings mode");
    };
    state.window_limit_input = "500000".to_string();
    state.warning_input = "50".to_string();
    state.critical_input = "80".to_string();

    assert!(app.context_settings_confirm());
    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(app.config.context_window_override, Some(500_000));
    assert_eq!(app.config.context_warning_percent, 50);
    assert_eq!(app.config.context_critical_percent, 80);
}

#[test]
fn context_settings_confirm_blank_window_limit_clears_the_override() {
    let mut app = context_settings_test_app();
    app.config.context_window_override = Some(500_000);
    app.start_context_settings();

    let AppMode::ContextSettings(state) = &mut app.mode else {
        panic!("expected ContextSettings mode");
    };
    state.window_limit_input.clear();

    assert!(app.context_settings_confirm());
    assert_eq!(app.config.context_window_override, None);
}

#[test]
fn context_settings_confirm_rejects_critical_at_or_below_warning() {
    let mut app = context_settings_test_app();
    app.start_context_settings();

    let AppMode::ContextSettings(state) = &mut app.mode else {
        panic!("expected ContextSettings mode");
    };
    state.warning_input = "80".to_string();
    state.critical_input = "80".to_string();

    assert!(!app.context_settings_confirm());
    let AppMode::ContextSettings(state) = &app.mode else {
        panic!("dialog should stay open after a rejected confirm");
    };
    assert!(state.error.is_some());
    // Rejected values must not leak into the live config.
    assert_eq!(app.config.context_warning_percent, 70);
}

#[test]
fn context_settings_confirm_rejects_out_of_range_percentages() {
    let mut app = context_settings_test_app();
    app.start_context_settings();

    let AppMode::ContextSettings(state) = &mut app.mode else {
        panic!("expected ContextSettings mode");
    };
    state.critical_input = "150".to_string();

    assert!(!app.context_settings_confirm());
    assert!(matches!(&app.mode, AppMode::ContextSettings(s) if s.error.is_some()));
}

#[test]
fn context_settings_confirm_rejects_zero_window_limit() {
    let mut app = context_settings_test_app();
    app.start_context_settings();

    let AppMode::ContextSettings(state) = &mut app.mode else {
        panic!("expected ContextSettings mode");
    };
    state.window_limit_input = "0".to_string();

    assert!(!app.context_settings_confirm());
    assert!(matches!(&app.mode, AppMode::ContextSettings(s) if s.error.is_some()));
}

#[test]
fn context_settings_cancel_discards_changes() {
    let mut app = context_settings_test_app();
    app.start_context_settings();

    let AppMode::ContextSettings(state) = &mut app.mode else {
        panic!("expected ContextSettings mode");
    };
    state.warning_input = "10".to_string();

    app.cancel_context_settings();

    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(app.config.context_warning_percent, 70);
}

#[test]
fn context_settings_focus_cycles_through_all_fields_and_wraps() {
    use crate::app::ContextSettingsField;

    let mut app = context_settings_test_app();
    app.start_context_settings();

    app.context_settings_focus_next();
    assert!(matches!(
        &app.mode,
        AppMode::ContextSettings(s) if s.field == ContextSettingsField::WarningPercent
    ));
    app.context_settings_focus_next();
    assert!(matches!(
        &app.mode,
        AppMode::ContextSettings(s) if s.field == ContextSettingsField::CriticalPercent
    ));
    app.context_settings_focus_next();
    assert!(matches!(
        &app.mode,
        AppMode::ContextSettings(s) if s.field == ContextSettingsField::WindowLimit
    ));
    app.context_settings_focus_prev();
    assert!(matches!(
        &app.mode,
        AppMode::ContextSettings(s) if s.field == ContextSettingsField::CriticalPercent
    ));
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

// ---------------------------------------------------------------------------
// Editable headless prompts: registry resolution + interpolation
// (`crate::prompts`). Kept here per AMF_PLAN.md; deeper cases live in
// `src/prompts/resolve.rs`.
// ---------------------------------------------------------------------------

#[test]
fn resolve_prompt_with_no_override_returns_the_builtin_default() {
    use crate::prompts::{PromptContext, PromptId, resolve_prompt};
    let ctx = PromptContext::new()
        .with("harness_name", "Claude")
        .with("max_chars", "60")
        .with("recent_lines", "did some work");
    let rendered = resolve_prompt(PromptId::SessionSummary, &AgentKind::Claude, &ctx);
    assert_eq!(
        rendered,
        crate::prompts::render_template(PromptId::SessionSummary.spec().default_template, &ctx)
    );
    assert!(rendered.contains("Summarize this Claude session in one line (max 60 chars)"));
    assert!(rendered.contains("did some work"));
}

#[test]
fn resolve_prompt_renders_a_missing_placeholder_literally() {
    use crate::prompts::{PromptContext, PromptId, resolve_prompt};
    // `recent_lines` deliberately omitted.
    let ctx = PromptContext::new()
        .with("harness_name", "Codex")
        .with("max_chars", "60");
    let rendered = resolve_prompt(PromptId::SessionSummary, &AgentKind::Codex, &ctx);
    assert!(
        rendered.contains("Session output:\n{{recent_lines}}"),
        "{rendered}"
    );
}

#[test]
fn resolve_prompt_renders_an_unknown_token_literally() {
    use crate::prompts::{PromptContext, render_template};
    let ctx = PromptContext::new().with("a", "1");
    assert_eq!(
        render_template("{{a}} {{not_a_real_token}}", &ctx),
        "1 {{not_a_real_token}}"
    );
}

#[test]
fn resolve_headless_template_applies_a_global_db_override_and_falls_back_otherwise() {
    use crate::db::AmfDb;
    use crate::db::prompt_overrides::OverrideScope;
    use crate::prompts::PromptId;

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = NamedTempFile::new().unwrap();
    app.db = Some(AmfDb::open(db_file.path()).unwrap());

    let repo = app.store.projects[0].repo.clone();
    let workdir = app.store.projects[0].features[0].workdir.clone();

    // No override yet → built-in default.
    let (before, source) = app.resolve_headless_template(
        PromptId::SessionSummary,
        &AgentKind::Claude,
        &repo,
        &workdir,
    );
    assert_eq!(source, crate::prompts::PromptSource::BuiltIn);
    assert_eq!(before, PromptId::SessionSummary.spec().default_template);

    // Write a global-scope shared override.
    app.db
        .as_ref()
        .unwrap()
        .upsert_prompt_override(
            PromptId::SessionSummary.as_str(),
            &OverrideScope::Global,
            None,
            "OVERRIDDEN {{recent_lines}}",
        )
        .unwrap();

    let (after, source) = app.resolve_headless_template(
        PromptId::SessionSummary,
        &AgentKind::Claude,
        &repo,
        &workdir,
    );
    assert_eq!(source, crate::prompts::PromptSource::Global);
    assert_eq!(after, "OVERRIDDEN {{recent_lines}}");

    // And it interpolates through the render helper.
    let rendered = app.resolve_headless_prompt(
        PromptId::SessionSummary,
        &AgentKind::Claude,
        &repo,
        &workdir,
        &crate::prompts::PromptContext::new().with("recent_lines", "did work"),
    );
    assert_eq!(rendered, "OVERRIDDEN did work");
}

#[test]
fn prompt_override_manager_saves_to_each_scope_and_survives_reopen() {
    use crate::db::AmfDb;
    use crate::prompts::{PromptId, PromptSource};

    let repo = tempfile::TempDir::new().unwrap();
    let store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = NamedTempFile::new().unwrap();
    app.db = Some(AmfDb::open(db_file.path()).unwrap());
    app.selection = Selection::Feature(0, 0);

    // Save a distinct override at each of the three scopes.
    let cases = [
        (
            PromptId::SessionSummary,
            "This project (amf.json)",
            "PROJECT one-liner {{recent_lines}}",
            PromptSource::Project,
        ),
        (
            PromptId::ReviewWalkthrough,
            "Global (all projects)",
            "GLOBAL walkthrough {{patch}}",
            PromptSource::Global,
        ),
        (
            PromptId::LearningAnswer,
            "This feature",
            "FEATURE answer {{question}}",
            PromptSource::Feature,
        ),
    ];

    for (id, scope_label, template, _expected) in cases {
        app.open_prompt_overrides(None);
        // Select the target row.
        let idx = crate::prompts::PromptId::ALL
            .iter()
            .position(|p| *p == id)
            .unwrap();
        if let AppMode::PromptOverrides(state) = &mut app.mode {
            state.selected = idx;
        }
        app.prompt_overrides_start_edit();
        if let AppMode::PromptOverrides(state) = &mut app.mode {
            let edit = state.edit.as_mut().unwrap();
            edit.editor.clear();
            edit.editor.insert_str(template);
            // Pick the scope.
            edit.scope_index = edit
                .scopes
                .iter()
                .position(|s| s.label() == scope_label)
                .unwrap();
            // harness_index 0 = shared.
        }
        app.prompt_overrides_confirm_save().unwrap();
        assert!(
            matches!(&app.mode, AppMode::PromptOverrides(s) if s.edit.is_none()),
            "editor closes after save"
        );
        app.prompt_overrides_close();
    }

    // Reopen: each row now reports its override source, and the effective
    // template (resolved the same way the call sites do) is the saved text.
    app.open_prompt_overrides(None);
    let rows = match &app.mode {
        AppMode::PromptOverrides(state) => state.rows.clone(),
        _ => panic!("manager not open"),
    };
    let repo_path = repo.path().to_path_buf();
    for (id, _scope, template, expected) in cases {
        let row = rows.iter().find(|r| r.id == id).unwrap();
        assert_eq!(row.source, expected, "{} source after reopen", id.as_str());
        let (effective, source) =
            app.resolve_headless_template(id, &AgentKind::default(), &repo_path, &repo_path);
        assert_eq!(source, expected, "{} resolved source", id.as_str());
        assert_eq!(effective, template, "{} effective template", id.as_str());
    }
}

#[test]
fn dashboard_shift_e_opens_the_prompt_override_manager() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.selection = Selection::Feature(0, 0);

    crate::handlers::handle_normal_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('E'), KeyModifiers::NONE),
    )
    .unwrap();

    assert!(
        matches!(&app.mode, AppMode::PromptOverrides(state)
            if state.rows.len() == crate::prompts::PromptId::ALL.len() && state.from_view.is_none()),
        "E on the dashboard opens the manager"
    );

    // Esc closes back to the dashboard.
    crate::handlers::handle_key(
        &mut app,
        KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE),
        20,
    )
    .unwrap();
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn precall_edit_opens_the_manager_focused_and_returns_to_the_notice() {
    use crate::app::precall::{PendingPrecall, PrecallAction};
    use crate::prompts::PromptId;

    let repo = tempfile::TempDir::new().unwrap();
    let store = store_with_repo(repo.path().to_path_buf(), ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(NamedTempFile::new().unwrap().path()).unwrap());
    app.selection = Selection::Feature(0, 0);

    app.mode = AppMode::PromptPrecall(Box::new(PendingPrecall {
        action: PrecallAction::ReviewChangesetOverview,
        prompt_id: PromptId::ReviewChangesetOverview,
        harness: AgentKind::Claude,
        preview: "some rendered prompt".to_string(),
        viewing: false,
        scroll: 0,
        prior_mode: Box::new(AppMode::Normal),
    }));

    // `e` opens the override manager, pre-selected on that prompt.
    app.precall_edit();
    let selected_id = match &app.mode {
        AppMode::PromptOverrides(state) => state.rows[state.selected].id,
        other => panic!("expected manager, got {:?}", std::mem::discriminant(other)),
    };
    assert_eq!(selected_id, PromptId::ReviewChangesetOverview);

    // Closing the manager returns to the pre-call notice (not the dashboard).
    app.prompt_overrides_close();
    assert!(matches!(&app.mode, AppMode::PromptPrecall(p)
        if p.prompt_id == PromptId::ReviewChangesetOverview));

    // Cancel restores the mode the run was initiated from.
    app.precall_cancel();
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn automated_headless_runs_announce_with_a_toast_not_a_modal() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.announce_headless_run(crate::prompts::PromptId::LearningAnswer, &AgentKind::Codex);
    assert!(!matches!(app.mode, AppMode::PromptPrecall(_)));
    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("Headless AI call") && t.message.contains("Codex")),
        "expected an announcing toast"
    );
}
