use super::support::*;
use crate::app::setup::{
    cleanup_agent_injected_files, ensure_notification_hooks, ensure_plan_mode_instructions,
    ensure_review_claude_md, strip_between_markers,
};
use crate::app::*;
use crate::project::{AgentKind, Feature, Project, SessionKind};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use chrono::Utc;
use std::collections::HashMap;
use tempfile::NamedTempFile;
use tempfile::TempDir;

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
    assert!(instructions.contains("`AMF_PLAN.md`"));
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
    assert!(!workdir.path().join("AMF_PLAN.md").exists());

    ensure_plan_mode_instructions(workdir.path(), &AgentKind::Claude, false);
    assert_eq!(
        std::fs::read_to_string(workdir.path().join("CLAUDE.local.md")).unwrap(),
        "# Existing\n"
    );
}

#[test]
fn plan_mode_instructions_refresh_legacy_managed_block() {
    let workdir = tempfile::TempDir::new().unwrap();
    std::fs::write(
        workdir.path().join("CLAUDE.local.md"),
        "# Existing\n\n\
         <!-- AMF:plan-instructions:begin -->\n\n\
         ## Plan Mode\n\n\
         Read `.claude/plan.md` before implementation.\n\n\
         <!-- AMF:plan-instructions:end -->\n",
    )
    .unwrap();

    ensure_plan_mode_instructions(workdir.path(), &AgentKind::Claude, true);
    ensure_plan_mode_instructions(workdir.path(), &AgentKind::Claude, true);

    let instructions = std::fs::read_to_string(workdir.path().join("CLAUDE.local.md")).unwrap();
    assert!(instructions.starts_with("# Existing\n"));
    assert!(!instructions.contains("`.claude/plan.md`"));
    assert!(instructions.contains("`AMF_PLAN.md`"));
    assert_eq!(
        instructions
            .matches("<!-- AMF:plan-instructions:begin -->")
            .count(),
        1,
        "managed instruction replacement should be idempotent"
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
    assert!(
        instructions.contains("Do not read `.claude/review-notes.md`"),
        "Review Mode must tell the agent to blind-append, not read the notes file back"
    );
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
        assert!(instructions.contains("`AMF_PLAN.md`"));
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
        todo_origin: None,
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

fn hook_uses_exec_form(settings: &serde_json::Value, event: &str, script_name: &str) -> bool {
    settings["hooks"][event]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
        .any(|hook| {
            hook["command"]
                .as_str()
                .is_some_and(|command| command.ends_with(script_name))
                && hook["args"].as_array().is_some_and(Vec::is_empty)
        })
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
    let cmds = hook_commands_for(&s, "Notification");
    // The legacy wiring ran notify.sh on Notification, which queued a pending
    // input for what is only a permission prompt. Notification is now wired to
    // attention.sh alone: it records *why* the session stopped and never
    // touches the notification flow.
    assert!(
        !cmds.iter().any(|c| c.contains("notify.sh")),
        "legacy Notification -> notify.sh hook should be removed; got: {cmds:?}"
    );
    assert!(
        cmds.iter().any(|c| c.contains("attention.sh")),
        "Notification missing attention.sh; got: {cmds:?}"
    );
}

#[test]
fn attention_hook_classifies_notifications_and_reports_completed_on_stop() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks(&workdir, VibeMode::Vibe);
    let s = read_settings(&workdir);

    // The event kind travels as argv[1], so one script serves every lifecycle
    // event that means "this session stopped".
    let args_for = |event: &str| -> Vec<String> {
        s["hooks"][event]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
            .filter(|hook| {
                hook["command"]
                    .as_str()
                    .is_some_and(|c| c.contains("attention.sh"))
            })
            .flat_map(|hook| hook["args"].as_array().cloned().unwrap_or_default())
            .filter_map(|arg| arg.as_str().map(str::to_string))
            .collect()
    };

    // Stop is unambiguous, so it names its kind outright. Notification is not:
    // it covers idle nudges and auth notices as well as permission prompts, so
    // the script is asked to classify the payload rather than being told it is
    // a question.
    assert_eq!(args_for("Notification"), vec!["notification".to_string()]);
    assert_eq!(args_for("Stop"), vec!["completed".to_string()]);
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
fn claude_exec_hooks_pass_paths_with_spaces_without_shell_quoting() {
    let path = PathBuf::from("/Users/me/Library/Application Support/amf/tool-stop.sh");
    let hook = crate::app::setup::claude_exec_hook(&path);

    assert_eq!(hook["command"].as_str(), path.to_str());
    assert!(hook["args"].as_array().is_some_and(Vec::is_empty));
}

#[test]
fn generated_claude_hooks_use_the_space_free_runtime_directory() {
    let workdir = TempDir::new().unwrap();
    call_ensure_hooks(&workdir, VibeMode::Vibe);
    let settings = read_settings(&workdir);
    let commands = hook_commands_for(&settings, "PostToolUse");
    let expected = crate::project::amf_claude_hooks_dir().join("tool-stop.sh");

    assert_eq!(commands, vec![expected.to_string_lossy()]);
    assert_eq!(
        expected.parent().and_then(|path| path.file_name()),
        Some(std::ffi::OsStr::new("hooks"))
    );
    assert_eq!(
        expected
            .parent()
            .and_then(|path| path.parent())
            .and_then(|path| path.file_name()),
        Some(std::ffi::OsStr::new(".amf"))
    );
}

#[test]
fn worktree_hook_setup_repairs_inherited_root_repo_hooks() {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().join(".worktrees").join("feature");
    let root_claude_dir = repo.path().join(".claude");
    std::fs::create_dir_all(&root_claude_dir).unwrap();
    std::fs::create_dir_all(&workdir).unwrap();
    std::fs::write(
        root_claude_dir.join("settings.local.json"),
        r#"{
          "hooks": {
            "PostToolUse": [{
              "matcher": "inherited-root-hook",
              "hooks": [
                {
                  "type": "command",
                  "command": "/Users/me/Library/Application Support/amf/tool-stop.sh"
                },
                {
                  "type": "command",
                  "command": "/tmp/user-post-tool-hook.sh"
                }
              ]
            }]
          }
        }"#,
    )
    .unwrap();
    std::fs::write(
        root_claude_dir.join("settings.json"),
        r#"{
          "hooks": {
            "Stop": [{
              "matcher": "legacy-root-hook",
              "hooks": [{
                "type": "command",
                "command": "'/Users/me/Library/Application Support/amf/thinking-stop.sh'"
              }]
            }]
          }
        }"#,
    )
    .unwrap();

    ensure_notification_hooks(
        &workdir,
        repo.path(),
        &VibeMode::Vibe,
        &AgentKind::Claude,
        true,
    );

    let root_local: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root_claude_dir.join("settings.local.json")).unwrap(),
    )
    .unwrap();
    let root_legacy: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root_claude_dir.join("settings.json")).unwrap(),
    )
    .unwrap();
    let runtime_dir = crate::project::amf_claude_hooks_dir();

    assert_eq!(
        root_local["hooks"]["PostToolUse"][0]["matcher"].as_str(),
        Some("inherited-root-hook")
    );
    assert_eq!(
        root_local["hooks"]["PostToolUse"][0]["hooks"][0]["command"].as_str(),
        Some(runtime_dir.join("tool-stop.sh").to_string_lossy().as_ref())
    );
    assert!(
        root_local["hooks"]["PostToolUse"][0]["hooks"][0]["args"]
            .as_array()
            .is_some_and(Vec::is_empty)
    );
    assert_eq!(
        root_local["hooks"]["PostToolUse"][0]["hooks"][1]["command"].as_str(),
        Some("/tmp/user-post-tool-hook.sh"),
        "user-owned hooks in the inherited entry must be preserved"
    );
    assert_eq!(
        root_legacy["hooks"]["Stop"][0]["hooks"][0]["command"].as_str(),
        Some(
            runtime_dir
                .join("thinking-stop.sh")
                .to_string_lossy()
                .as_ref()
        )
    );

    let worktree_settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(workdir.join(".claude").join("settings.local.json")).unwrap(),
    )
    .unwrap();
    assert!(hook_uses_exec_form(
        &worktree_settings,
        "PostToolUse",
        "tool-stop.sh"
    ));
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

    let repaired = crate::app::setup::repair_unquoted_claude_hooks_for_store(&store);

    assert_eq!(repaired, 1);
    let s = read_settings(&workdir);
    let cmds = hook_commands_for(&s, "Stop");
    assert!(
        cmds.iter()
            .all(|cmd| !cmd.starts_with("/Users/me/Library/Application Support/amf/")),
        "stale unquoted hook should be repaired, got: {cmds:?}"
    );
    assert!(hook_uses_exec_form(&s, "Stop", "thinking-stop.sh"));
}

#[test]
fn repair_unquoted_claude_hooks_refreshes_legacy_settings_json() {
    let repo = TempDir::new().unwrap();
    let workdir = TempDir::new().unwrap();
    let claude_dir = workdir.path().join(".claude");
    std::fs::create_dir_all(&claude_dir).unwrap();
    std::fs::write(
        claude_dir.join("settings.json"),
        r#"{
          "hooks": {
            "PostToolUse": [{
              "matcher": "",
              "hooks": [{
                "type": "command",
                "command": "/Users/me/Library/Application Support/amf/tool-stop.sh"
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
        AgentKind::Codex,
        false,
        false,
    );
    feature.add_session_named(SessionKind::Claude, "Pairing Claude".to_string());
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

    let repaired = crate::app::setup::repair_unquoted_claude_hooks_for_store(&store);

    assert_eq!(repaired, 1);
    assert!(
        !claude_dir.join("settings.json").exists(),
        "legacy settings.json should be removed once its stale AMF hook is cleaned"
    );
    let s = read_settings(&workdir);
    assert!(hook_uses_exec_form(&s, "PostToolUse", "tool-stop.sh"));
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
fn root_vibeless_diff_review_hook_is_scoped_away_from_worktrees() {
    let repo = TempDir::new().unwrap();
    let workdir = repo.path().join(".worktrees").join("feature");
    std::fs::create_dir_all(&workdir).unwrap();

    ensure_notification_hooks(
        repo.path(),
        repo.path(),
        &VibeMode::Vibeless,
        &AgentKind::Claude,
        false,
    );
    ensure_notification_hooks(
        &workdir,
        repo.path(),
        &VibeMode::Vibe,
        &AgentKind::Claude,
        true,
    );

    let root_settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(repo.path().join(".claude/settings.local.json")).unwrap(),
    )
    .unwrap();
    let root_diff_hook = root_settings["hooks"]["PreToolUse"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|entry| entry["hooks"].as_array().into_iter().flatten())
        .find(|hook| {
            hook["command"]
                .as_str()
                .is_some_and(|command| command.ends_with("custom-diff-review.sh"))
        })
        .expect("root Vibeless settings should contain the diff-review hook");
    assert_eq!(
        root_diff_hook["args"].as_array(),
        Some(&vec![serde_json::json!(repo.path().to_string_lossy())]),
        "the inherited hook must carry the Git root that owns it"
    );

    let worktree_settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(workdir.join(".claude/settings.local.json")).unwrap(),
    )
    .unwrap();
    assert!(
        hook_commands_for(&worktree_settings, "PreToolUse")
            .iter()
            .all(|command| !command.ends_with("custom-diff-review.sh")),
        "a Vibe worktree should not install its own diff-review hook"
    );
}

#[test]
fn inherited_diff_review_script_exits_for_a_different_git_root() {
    let owning_repo = TempDir::new().unwrap();
    let active_repo = TempDir::new().unwrap();
    for repo in [&owning_repo, &active_repo] {
        let status = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(repo.path())
            .status()
            .unwrap();
        assert!(status.success());
    }

    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("plugins/diff-review/scripts/custom-diff-review.sh");
    let output = std::process::Command::new("bash")
        .arg(script)
        .arg(owning_repo.path())
        .current_dir(active_repo.path())
        .env("AMF_ACTIVE", "1")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "an inherited hook should exit before invoking popup dependencies: {}",
        String::from_utf8_lossy(&output.stderr)
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
            .filter(|cmd| cmd.ends_with("/.amf/hooks/tool-stop.sh"))
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
    // The script no longer assembles the payload itself — it forwards Codex's
    // JSON and lets `amf notify` merge in the session identity from the
    // environment it inherits. Preserving `provider_session_id` /
    // `amf_feature_session_id` is therefore covered by `hook_payload`'s unit
    // tests; what stays this script's job is being AMF-gated, passing the
    // payload through, and reporting the turn as completed.
    assert!(
        codex_notify.contains("AMF_ACTIVE"),
        "Codex notify hook should stay gated on AMF_ACTIVE, got: {codex_notify}"
    );
    assert!(
        codex_notify.contains("notify") && codex_notify.contains("AMF_BIN"),
        "Codex notify hook should deliver through `amf notify`, got: {codex_notify}"
    );
    assert!(
        codex_notify.contains("--event-kind completed"),
        "Codex notify hook should report the finished turn, got: {codex_notify}"
    );
    assert!(
        !workdir.path().join(".codex").join("config.toml").exists(),
        "repo-root codex feature should not write unsupported project-local config"
    );
    assert!(
        !workdir
            .path()
            .join(".agents/skills/amf-screenshot")
            .exists(),
        "AMF should not inject its repository-specific screenshot skill into managed projects"
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
fn screenshot_skill_is_not_injected_into_managed_workspaces() {
    for (agent, skills_root) in [
        (AgentKind::Claude, ".claude"),
        (AgentKind::Codex, ".agents"),
        (AgentKind::Opencode, ".opencode"),
    ] {
        let workdir = TempDir::new().unwrap();
        call_ensure_hooks_for(&workdir, VibeMode::Vibe, agent.clone(), true);
        assert!(
            !workdir
                .path()
                .join(skills_root)
                .join("skills/amf-screenshot")
                .exists(),
            "AMF should not inject its repository-specific screenshot skill for {agent:?}"
        );
    }
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
    let screenshot_skill = workdir
        .path()
        .join(".agents/skills/amf-screenshot/SKILL.md");
    std::fs::create_dir_all(screenshot_skill.parent().unwrap()).unwrap();
    std::fs::write(&screenshot_skill, "repo-local screenshot skill").unwrap();

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
        screenshot_skill.exists(),
        "cleanup should preserve a repository-local screenshot skill"
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
            todo_reference: None,
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
            todo_reference: None,
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
