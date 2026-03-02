use super::*;
use super::setup::{ensure_notification_hooks, strip_between_markers};
use super::util::{shorten_path, slugify};

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
    let result =
        strip_between_markers(s, "<!-- BEGIN -->", "<!-- END -->");
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

use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crate::project::{Feature, Project, AgentKind};
use chrono::Utc;
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
        agent: AgentKind::default(),
        enable_chrome: false,
        has_notes: false,
        status,
        created_at: now,
        last_accessed: now,
    };
    let project = Project {
        id: "proj-1".to_string(),
        name: "my-project".to_string(),
        repo: PathBuf::from("/tmp/test-repo"),
        collapsed: false,
        features: vec![feature],
        created_at: now,
        is_git: false,
    };
    ProjectStore { version: 2, projects: vec![project] }
}

// ── sync_statuses ─────────────────────────────────────────────

#[test]
fn sync_statuses_stopped_becomes_idle_when_session_live() {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_list_sessions()
        .times(1)
        .returning(|| Ok(vec!["amf-my-feat".to_string()]));

    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = App::new_for_test(
        store,
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
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
    let mut app = App::new_for_test(
        store,
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
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
    let mut app = App::new_for_test(
        store,
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.sync_statuses();

    // Already Idle; stays Idle (not overwritten)
    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Idle
    );
}

// ── create_feature validation ─────────────────────────────────

fn app_in_creating_feature_mode(
    store: ProjectStore,
    project_name: &str,
    branch: &str,
    use_worktree: bool,
) -> App {
    use crate::app::state::{
        CreateFeatureState, CreateFeatureStep,
    };
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
        source_index: 0,
        worktrees: vec![],
        worktree_index: 0,
        use_worktree,
        enable_chrome: false,
        enable_notes: false,
        preset_index: 0,
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
        "",    // empty branch
        false,
    );
    app.create_feature().unwrap();

    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("cannot be empty"),
        "got: {:?}", app.message
    );
}

#[test]
fn create_feature_duplicate_name_sets_error_no_external_calls() {
    let store = store_with_feature(ProjectStatus::Stopped);
    // "my-feat" already exists in the store
    let mut app = app_in_creating_feature_mode(
        store,
        "my-project",
        "my-feat",
        false,
    );
    app.create_feature().unwrap();

    let msg = app.message.as_deref().unwrap_or("");
    assert!(
        msg.contains("already exists"),
        "got: {msg}"
    );
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
    assert!(
        msg.contains("Only one non-worktree"),
        "got: {msg}"
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
    let mut app = App::new_for_test(
        store,
        Box::new(tmux),
        Box::new(MockWorktreeOps::new()),
    );
    app.store_path = tmp.path().to_path_buf();
    app.selection = Selection::Feature(0, 0);

    app.stop_feature().unwrap();

    assert_eq!(
        app.store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Stopped"),
        "got: {:?}", app.message
    );
}

// ── ensure_notification_hooks ─────────────────────────────

use tempfile::TempDir;

fn read_settings(dir: &TempDir) -> serde_json::Value {
    let path = dir.path().join(".claude").join("settings.json");
    let s = std::fs::read_to_string(&path)
        .expect("settings.json should exist");
    serde_json::from_str(&s).expect("valid JSON")
}

fn hook_commands_for(
    settings: &serde_json::Value,
    event: &str,
) -> Vec<String> {
    settings["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| {
            entry["hooks"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|h| {
                    h["command"].as_str().map(|s| s.to_string())
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

fn call_ensure_hooks(workdir: &TempDir, mode: VibeMode) {
    let repo = workdir.path(); // repo = workdir in tests
    ensure_notification_hooks(
        workdir.path(),
        repo,
        &mode,
        &AgentKind::Claude,
    );
}

#[test]
fn stop_hook_has_thinking_remove_and_notify() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks(&workdir, VibeMode::Vibe);
    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "Stop");
    assert!(
        cmds.iter().any(|c| c.contains("amf-thinking") && c.contains("rm")),
        "Stop hook missing thinking-remove cmd; got: {cmds:?}"
    );
    assert!(
        cmds.iter().any(|c| c.contains("notify.sh")),
        "Stop hook missing notify.sh; got: {cmds:?}"
    );
}

#[test]
fn pre_tool_use_hook_has_thinking_touch_and_clear() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks(&workdir, VibeMode::Vibe);
    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "PreToolUse");
    assert!(
        cmds.iter().any(|c| c.contains("amf-thinking") && c.contains("touch")),
        "PreToolUse missing thinking-touch cmd; got: {cmds:?}"
    );
    assert!(
        cmds.iter().any(|c| c.contains("clear-notify.sh")),
        "PreToolUse missing clear-notify.sh; got: {cmds:?}"
    );
}

#[test]
fn notification_hook_is_removed() {
    let workdir = TempDir::new().unwrap();
    // Pre-populate with the legacy Notification hook.
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{"hooks":{"Notification":[{"matcher":"","hooks":[{"type":"command","command":"/old/notify.sh"}]}]}}"#,
    ).unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibe);

    let s = read_settings(&workdir);
    assert!(
        s["hooks"].get("Notification").is_none(),
        "legacy Notification hook should be removed"
    );
}

#[test]
fn vibeless_pre_tool_use_includes_diff_review_when_script_present() {
    let workdir = TempDir::new().unwrap();
    // Create the diff-review script so it gets picked up.
    let scripts_dir = workdir
        .path()
        .join("plugins")
        .join("diff-review")
        .join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();
    std::fs::write(scripts_dir.join("diff-review.sh"), "").unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibeless);

    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "PreToolUse");
    assert!(
        cmds.iter().any(|c| c.contains("diff-review.sh")),
        "Vibeless PreToolUse should include diff-review; got: {cmds:?}"
    );
}

#[test]
fn vibeless_permissions_include_edit_and_write() {
    let workdir = TempDir::new().unwrap();
    // Need diff-review script for vibeless path to complete.
    let scripts_dir = workdir
        .path()
        .join("plugins")
        .join("diff-review")
        .join("scripts");
    std::fs::create_dir_all(&scripts_dir).unwrap();
    std::fs::write(scripts_dir.join("diff-review.sh"), "").unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibeless);

    let s = read_settings(&workdir);
    let allow = s["permissions"]["allow"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let strs: Vec<&str> = allow
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
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
        claude_dir.join("settings.json"),
        r#"{"permissions":{"allow":["Edit","Write","Bash"]}}"#,
    ).unwrap();

    call_ensure_hooks(&workdir, VibeMode::Vibe);

    let s = read_settings(&workdir);
    let allow = s["permissions"]["allow"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let strs: Vec<&str> = allow
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(!strs.contains(&"Edit"), "Edit should be removed for Vibe mode");
    assert!(!strs.contains(&"Write"), "Write should be removed for Vibe mode");
    // Unrelated permissions are preserved.
    assert!(strs.contains(&"Bash"), "unrelated permissions should remain");
}

#[test]
fn ensure_hooks_is_idempotent() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks(&workdir, VibeMode::Vibe);
    let first = read_settings(&workdir);
    call_ensure_hooks(&workdir, VibeMode::Vibe);
    let second = read_settings(&workdir);
    assert_eq!(first, second, "calling twice should produce identical output");
}
