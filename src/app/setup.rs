use std::path::{Path, PathBuf};

use crate::project::{AgentKind, ProjectStore, VibeMode};

use super::AppConfig;

const NOTIFY_SH: &str =
    include_str!("../../scripts/notify.sh");
const CLEAR_NOTIFY_SH: &str =
    include_str!("../../scripts/clear-notify.sh");
const SAVE_PROMPT_SH: &str =
    include_str!("../../scripts/save-prompt.sh");
const THINKING_START_SH: &str =
    include_str!("../../scripts/thinking-start.sh");
const THINKING_STOP_SH: &str =
    include_str!("../../scripts/thinking-stop.sh");
const TOOL_START_SH: &str =
    include_str!("../../scripts/tool-start.sh");
const TOOL_STOP_SH: &str =
    include_str!("../../scripts/tool-stop.sh");
const CODEX_NOTIFY_SH: &str =
    include_str!("../../scripts/codex-notify.sh");
const INPUT_REQUEST_JS: &str =
    include_str!("../../.opencode/plugins/input-request.js");

pub fn ensure_notify_scripts() {
    let config_dir = crate::project::amf_config_dir();
    let _ = std::fs::create_dir_all(&config_dir);
    let notify_path = config_dir.join("notify.sh");
    let clear_path = config_dir.join("clear-notify.sh");
    let _ = std::fs::write(&notify_path, NOTIFY_SH);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&notify_path, std::fs::Permissions::from_mode(0o755));
    }
    let _ = std::fs::write(&clear_path, CLEAR_NOTIFY_SH);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&clear_path, std::fs::Permissions::from_mode(0o755));
    }
    let save_prompt_path = config_dir.join("save-prompt.sh");
    let thinking_start_path =
        config_dir.join("thinking-start.sh");
    let thinking_stop_path =
        config_dir.join("thinking-stop.sh");
    let tool_start_path = config_dir.join("tool-start.sh");
    let tool_stop_path = config_dir.join("tool-stop.sh");
    let _ = std::fs::write(&save_prompt_path, SAVE_PROMPT_SH);
    let _ = std::fs::write(
        &thinking_start_path,
        THINKING_START_SH,
    );
    let _ =
        std::fs::write(&thinking_stop_path, THINKING_STOP_SH);
    let _ = std::fs::write(&tool_start_path, TOOL_START_SH);
    let _ = std::fs::write(&tool_stop_path, TOOL_STOP_SH);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            &save_prompt_path,
            std::fs::Permissions::from_mode(0o755),
        );
        let _ = std::fs::set_permissions(
            &thinking_start_path,
            std::fs::Permissions::from_mode(0o755),
        );
        let _ = std::fs::set_permissions(
            &thinking_stop_path,
            std::fs::Permissions::from_mode(0o755),
        );
        let _ = std::fs::set_permissions(
            &tool_start_path,
            std::fs::Permissions::from_mode(0o755),
        );
        let _ = std::fs::set_permissions(
            &tool_stop_path,
            std::fs::Permissions::from_mode(0o755),
        );
    }
    let plugins_dir = config_dir.join("plugins");
    let _ = std::fs::create_dir_all(&plugins_dir);
    let input_request_path = plugins_dir.join("input-request.js");
    let _ = std::fs::write(&input_request_path, INPUT_REQUEST_JS);
}

/// Refresh opencode plugin files in all known opencode feature
/// workdirs, so existing sessions/worktrees pick up plugin fixes
/// without requiring feature recreation.
pub fn refresh_opencode_plugins_for_store(
    store: &ProjectStore,
) -> usize {
    let mut refreshed = 0usize;
    for project in &store.projects {
        for feature in &project.features {
            if !matches!(feature.agent, AgentKind::Opencode) {
                continue;
            }
            ensure_opencode_plugins(
                &feature.workdir,
                &project.repo,
                &feature.mode,
            );
            refreshed += 1;
        }
    }
    refreshed
}

pub fn load_config() -> AppConfig {
    let config_path = crate::project::amf_config_dir().join("config.json");

    let config = if config_path.exists() {
        std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        let config = AppConfig::default();
        let dir = config_path.parent().unwrap();
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&config).unwrap_or_default(),
        );
        config
    };

    if let Some(ref theme) = config.opencode_theme {
        let _ = update_opencode_theme(theme);
    }

    config
}

pub fn update_opencode_theme(theme: &str) -> anyhow::Result<()> {
    let opencode_config_path = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("opencode")
        .join("opencode.json");

    let mut config: serde_json::Value = if opencode_config_path.exists() {
        std::fs::read_to_string(&opencode_config_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if let Some(obj) = config.as_object_mut() {
        obj.insert("theme".to_string(), serde_json::json!(theme));
    }

    if let Some(parent) = opencode_config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    std::fs::write(
        &opencode_config_path,
        serde_json::to_string_pretty(&config)?,
    )?;

    Ok(())
}

pub fn remove_old_diff_review_plugin(repo: &Path) {
    let settings_path = repo.join(".claude").join("settings.local.json");
    if !settings_path.exists() {
        return;
    }

    let mut settings: serde_json::Value = match std::fs::read_to_string(&settings_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(v) => v,
        None => return,
    };

    let changed = settings
        .get_mut("enabledPlugins")
        .and_then(|p| p.as_object_mut())
        .map(|obj| obj.remove("diff-review@claude_vibeless").is_some())
        .unwrap_or(false);

    if !changed {
        return;
    }

    if settings
        .get("enabledPlugins")
        .and_then(|p| p.as_object())
        .is_some_and(|obj| obj.is_empty())
    {
        settings.as_object_mut().unwrap().remove("enabledPlugins");
    }

    if settings.as_object().is_some_and(|obj| obj.is_empty()) {
        let _ = std::fs::remove_file(&settings_path);
    } else {
        let _ = std::fs::write(
            &settings_path,
            serde_json::to_string_pretty(&settings).unwrap_or_default() + "\n",
        );
    }
}

fn ensure_opencode_plugins(workdir: &Path, repo: &Path, mode: &VibeMode) {
    let plugins_dir = workdir.join(".opencode").join("plugins");
    let _ = std::fs::create_dir_all(&plugins_dir);

    let global_input_request = crate::project::amf_config_dir()
        .join("plugins")
        .join("input-request.js");
    let dst_input_request = plugins_dir.join("input-request.js");

    if global_input_request.exists() {
        let _ = std::fs::copy(&global_input_request, &dst_input_request);
    }

    let dst_diff_review_js = plugins_dir.join("diff-review.js");
    let dst_diff_review_sh = plugins_dir.join("diff-review.sh");
    let dst_change_tracker = plugins_dir.join("change-tracker.js");
    let dst_feedback_prompt = plugins_dir.join("feedback-prompt.sh");
    let dst_explain = plugins_dir.join("explain.sh");
    let _ = std::fs::remove_file(&dst_diff_review_js);
    let _ = std::fs::remove_file(&dst_diff_review_sh);
    let _ = std::fs::remove_file(&dst_change_tracker);
    let _ = std::fs::remove_file(&dst_feedback_prompt);
    let _ = std::fs::remove_file(&dst_explain);

    if matches!(mode, VibeMode::Vibeless | VibeMode::Review) {
        let src_change_tracker = repo
            .join(".opencode")
            .join("plugins")
            .join("change-tracker.js");

        if src_change_tracker.exists() {
            let _ = std::fs::copy(&src_change_tracker, &dst_change_tracker);
        }

        let src_diff_review_js = repo
            .join(".opencode")
            .join("plugins")
            .join("diff-review.js");
        let src_diff_review_sh = repo
            .join(".opencode")
            .join("plugins")
            .join("diff-review.sh");

        if src_diff_review_js.exists() {
            let _ = std::fs::copy(&src_diff_review_js, &dst_diff_review_js);
        }

        if src_diff_review_sh.exists() {
            let _ = std::fs::copy(&src_diff_review_sh, &dst_diff_review_sh);
        }

        let src_feedback_prompt = repo
            .join(".opencode")
            .join("plugins")
            .join("feedback-prompt.sh");

        if src_feedback_prompt.exists() {
            let _ = std::fs::copy(&src_feedback_prompt, &dst_feedback_prompt);
        }

        let src_explain = repo.join(".opencode").join("plugins").join("explain.sh");

        if src_explain.exists() {
            let _ = std::fs::copy(&src_explain, &dst_explain);
        }
    }
}

fn ensure_codex_notify_hook(workdir: &Path) {
    let codex_dir = workdir.join(".codex");
    let _ = std::fs::create_dir_all(&codex_dir);

    let hook_path = codex_dir.join("amf-codex-notify.sh");
    let original_notify_path =
        codex_dir.join("amf-codex-notify-original.json");
    let _ = std::fs::write(&hook_path, CODEX_NOTIFY_SH);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            &hook_path,
            std::fs::Permissions::from_mode(0o755),
        );
    }

    let config_path = codex_dir.join("config.toml");
    let mut config = if config_path.exists() {
        match std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
        {
            Some(v) => v,
            None => return,
        }
    } else {
        toml::Value::Table(Default::default())
    };

    let Some(table) = config.as_table_mut() else {
        return;
    };
    let hook_cmd = hook_path.to_string_lossy().to_string();
    let existing_notify = table.get("notify").and_then(|notify| {
        if let Some(arr) = notify.as_array() {
            let values: Option<Vec<String>> = arr
                .iter()
                .map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
            values.filter(|v| !v.is_empty())
        } else {
            notify.as_str().map(|s| vec![s.to_string()])
        }
    });

    if let Some(existing) = existing_notify {
        if existing != vec![hook_cmd.clone()] {
            if let Ok(rendered) = serde_json::to_string_pretty(&existing) {
                let _ = std::fs::write(&original_notify_path, rendered);
            }
        } else {
            let _ = std::fs::remove_file(&original_notify_path);
        }
    } else {
        let _ = std::fs::remove_file(&original_notify_path);
    }
    table.insert(
        "notify".to_string(),
        toml::Value::Array(vec![toml::Value::String(hook_cmd)]),
    );

    if let Ok(rendered) = toml::to_string_pretty(&config) {
        let _ = std::fs::write(&config_path, rendered);
    }
}

pub fn ensure_notification_hooks(
    workdir: &Path,
    repo: &Path,
    mode: &VibeMode,
    agent: &AgentKind,
    is_worktree: bool,
) {
    remove_old_diff_review_plugin(repo);

    if matches!(agent, AgentKind::Opencode) {
        ensure_opencode_plugins(workdir, repo, mode);
        return;
    }
    if matches!(agent, AgentKind::Codex) {
        if is_worktree {
            ensure_codex_notify_hook(workdir);
        }
        return;
    }

    let claude_dir = workdir.join(".claude");
    let settings_path = claude_dir.join("settings.json");

    let config_dir = crate::project::amf_config_dir();
    let notify_cmd = config_dir.join("notify.sh").to_string_lossy().into_owned();
    let clear_cmd = config_dir
        .join("clear-notify.sh")
        .to_string_lossy()
        .into_owned();
    let save_prompt_cmd = config_dir
        .join("save-prompt.sh")
        .to_string_lossy()
        .into_owned();
    let thinking_start_cmd = config_dir
        .join("thinking-start.sh")
        .to_string_lossy()
        .into_owned();
    let thinking_stop_cmd = config_dir
        .join("thinking-stop.sh")
        .to_string_lossy()
        .into_owned();
    let tool_start_cmd = config_dir
        .join("tool-start.sh")
        .to_string_lossy()
        .into_owned();
    let tool_stop_cmd = config_dir
        .join("tool-stop.sh")
        .to_string_lossy()
        .into_owned();

    let script_suffix = ["plugins", "diff-review", "scripts", "diff-review.sh"];
    let amf_root = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent()?.parent()?.parent().map(PathBuf::from));
    let diff_review_cmd = [
        Some(workdir.to_path_buf()),
        Some(repo.to_path_buf()),
        amf_root,
    ]
    .into_iter()
    .flatten()
    .map(|base| script_suffix.iter().fold(base, |p, s| p.join(s)))
    .find(|p| p.exists())
    .map(|p| p.to_string_lossy().into_owned());

    let wants_diff_review = matches!(mode, VibeMode::Vibeless);

    let mut settings: serde_json::Value = if settings_path.exists() {
        std::fs::read_to_string(&settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    let hooks_obj = hooks.as_object_mut().unwrap();

    // Stop: clear active thinking + write stop notification.
    hooks_obj.insert("Stop".to_string(), serde_json::json!([{
        "matcher": "",
        "hooks": [
            { "type": "command", "command": thinking_stop_cmd },
            { "type": "command", "command": notify_cmd }
        ]
    }]));

    // Remove legacy Notification hook (replaced by Stop above).
    hooks_obj.remove("Notification");

    // PreToolUse: set thinking + tool-running + clear notification,
    // plus diff-review for vibeless mode.
    let mut pre_tool_hooks: Vec<serde_json::Value> = vec![
        serde_json::json!({
            "type": "command",
            "command": thinking_start_cmd
        }),
        serde_json::json!({
            "type": "command",
            "command": tool_start_cmd
        }),
        serde_json::json!({
            "type": "command",
            "command": clear_cmd
        }),
    ];
    if wants_diff_review {
        if let Some(ref dr_cmd) = diff_review_cmd {
            pre_tool_hooks.push(serde_json::json!({
                "type": "command",
                "command": dr_cmd,
                "timeout": 600
            }));
        }
    }
    hooks_obj.insert(
        "PreToolUse".to_string(),
        serde_json::json!([{
            "matcher": if wants_diff_review && diff_review_cmd.is_some() {
                "Edit|Write"
            } else {
                ""
            },
            "hooks": pre_tool_hooks
        }]),
    );

    hooks_obj.insert("PostToolUse".to_string(), serde_json::json!([{
        "matcher": "",
        "hooks": [
            { "type": "command", "command": tool_stop_cmd }
        ]
    }]));

    // UserPromptSubmit: set thinking + persist latest prompt.
    hooks_obj.insert("UserPromptSubmit".to_string(), serde_json::json!([{
        "matcher": "",
        "hooks": [
            { "type": "command", "command": thinking_start_cmd },
            { "type": "command", "command": save_prompt_cmd }
        ]
    }]));

    if wants_diff_review {
        let perms = settings
            .as_object_mut()
            .unwrap()
            .entry("permissions")
            .or_insert_with(|| serde_json::json!({}));
        let allow = perms
            .as_object_mut()
            .unwrap()
            .entry("allow")
            .or_insert_with(|| serde_json::json!([]));
        let arr = allow.as_array_mut().unwrap();
        if !arr.iter().any(|v| v.as_str() == Some("Edit")) {
            arr.push(serde_json::json!("Edit"));
        }
        if !arr.iter().any(|v| v.as_str() == Some("Write")) {
            arr.push(serde_json::json!("Write"));
        }
    } else if let Some(arr) = settings
        .pointer_mut("/permissions/allow")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|v| v.as_str() != Some("Edit") && v.as_str() != Some("Write"));
    }

    let _ = std::fs::create_dir_all(&claude_dir);
    let _ = std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings).unwrap_or_default(),
    );

    // Ensure notifications/ is gitignored within .claude/
    let claude_gitignore = claude_dir.join(".gitignore");
    let gitignore_entry = "notifications/\n";
    let needs_entry = std::fs::read_to_string(&claude_gitignore)
        .map(|s| !s.contains("notifications/"))
        .unwrap_or(true);
    if needs_entry {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&claude_gitignore);
        if let Ok(ref mut file) = f {
            use std::io::Write;
            let _ = file.write_all(gitignore_entry.as_bytes());
        }
    }

    // Ensure review-notes.md is gitignored within .claude/
    let needs_review_entry = std::fs::read_to_string(&claude_gitignore)
        .map(|s| !s.contains("review-notes.md"))
        .unwrap_or(true);
    if needs_review_entry
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&claude_gitignore)
    {
        use std::io::Write as _;
        let _ = f.write_all(b"review-notes.md\n");
    }

    // Ensure latest-prompt.txt is gitignored within .claude/
    let needs_prompt_entry = std::fs::read_to_string(&claude_gitignore)
        .map(|s| !s.contains("latest-prompt.txt"))
        .unwrap_or(true);
    if needs_prompt_entry
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&claude_gitignore)
    {
        use std::io::Write as _;
        let _ = f.write_all(b"latest-prompt.txt\n");
    }
}

pub fn strip_between_markers(s: &str, begin: &str, end: &str) -> String {
    if let (Some(bi), Some(ei)) = (s.find(begin), s.find(end)) {
        let end_pos = ei + end.len();
        // eat trailing newline after end marker
        let end_pos = if s.as_bytes().get(end_pos) == Some(&b'\n') {
            end_pos + 1
        } else {
            end_pos
        };
        // eat leading blank line before begin marker
        let begin_pos = if bi >= 2 && &s[bi - 2..bi] == "\n\n" {
            bi - 1
        } else {
            bi
        };
        format!("{}{}", &s[..begin_pos], &s[end_pos..])
    } else {
        s.to_string()
    }
}

pub fn ensure_review_claude_md(workdir: &Path, enabled: bool) {
    const BEGIN: &str = "<!-- AMF:review-instructions:begin -->";
    const END: &str = "<!-- AMF:review-instructions:end -->";
    const BLOCK: &str = concat!(
        "<!-- AMF:review-instructions:begin -->\n\n",
        "## Review Mode\n\n",
        "You are in **REVIEW MODE**. Before making any file ",
        "change (Edit or Write), append a note to ",
        "`.claude/review-notes.md` explaining:\n\n",
        "- The relative path of the file you are changing\n",
        "- What you are changing and why\n",
        "- How it fits the overall approach\n\n",
        "Use this exact format:\n\n",
        "```\n",
        "## <relative-file-path> — <brief title>\n\n",
        "<your explanation>\n\n",
        "---\n",
        "```\n\n",
        "Write the note BEFORE the edit so the reviewer can\n",
        "see your reasoning when the diff appears.\n\n",
        "<!-- AMF:review-instructions:end -->\n",
    );

    // CLAUDE.local.md is Claude Code's designated gitignored variant of
    // CLAUDE.md — it is read automatically but never committed.
    let md_path = workdir.join("CLAUDE.local.md");
    let current = std::fs::read_to_string(&md_path).unwrap_or_default();
    let has_block = current.contains(BEGIN);

    // Ensure CLAUDE.local.md is gitignored at the workdir root.
    let gitignore_path = workdir.join(".gitignore");
    let needs_ignore = std::fs::read_to_string(&gitignore_path)
        .map(|s| !s.contains("CLAUDE.local.md"))
        .unwrap_or(true);
    if needs_ignore
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&gitignore_path)
    {
        use std::io::Write as _;
        let _ = f.write_all(b"CLAUDE.local.md\n");
    }

    if enabled {
        if has_block {
            return; // already injected
        }
        let content = if current.is_empty() {
            BLOCK.to_string()
        } else {
            format!("{}\n{}", current.trim_end(), BLOCK)
        };
        let _ = std::fs::write(&md_path, content);
    } else if has_block {
        let stripped = strip_between_markers(&current, BEGIN, END);
        if stripped.trim().is_empty() {
            let _ = std::fs::remove_file(&md_path);
        } else {
            let _ = std::fs::write(&md_path, format!("{}\n", stripped.trim_end()));
        }
    }
}
