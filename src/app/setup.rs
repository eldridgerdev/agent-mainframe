use std::path::{Path, PathBuf};

use crate::project::{AgentKind, ProjectStore, VibeMode};
use crate::theme::ThemeManager;

use super::{AppConfig, DiffReviewViewer};

const NOTIFY_SH: &str = include_str!("../../scripts/notify.sh");
const CLEAR_NOTIFY_SH: &str = include_str!("../../scripts/clear-notify.sh");
const SAVE_PROMPT_SH: &str = include_str!("../../scripts/save-prompt.sh");
const THINKING_START_SH: &str = include_str!("../../scripts/thinking-start.sh");
const THINKING_STOP_SH: &str = include_str!("../../scripts/thinking-stop.sh");
const TOOL_START_SH: &str = include_str!("../../scripts/tool-start.sh");
const TOOL_STOP_SH: &str = include_str!("../../scripts/tool-stop.sh");
const CODEX_NOTIFY_SH: &str = include_str!("../../scripts/codex-notify.sh");
const CODEX_DIFF_REVIEW_SH: &str = include_str!("../../scripts/codex-diff-review.sh");
const INPUT_REQUEST_JS: &str = include_str!("../../.opencode/plugins/input-request.js");
const CHANGE_TRACKER_JS: &str = include_str!("../../.opencode/plugins/change-tracker.js");
const CUSTOM_DIFF_REVIEW_SH: &str =
    include_str!("../../plugins/diff-review/scripts/custom-diff-review.sh");
const CLAUDE_SETTINGS_LOCAL_JSON: &str = "settings.local.json";
const CLAUDE_SETTINGS_JSON: &str = "settings.json";
const CLAUDE_STATE_JSON: &str = "amf-hook-state.json";
const CODEX_CONFIG_TOML: &str = "config.toml";

#[derive(Default)]
struct ClaudeSettingsState {
    permissions_added: Vec<String>,
}

impl ClaudeSettingsState {
    fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
            .and_then(|value| {
                value
                    .get("permissions_added")
                    .and_then(|v| v.as_array())
                    .map(|entries| Self {
                        permissions_added: entries
                            .iter()
                            .filter_map(|entry| entry.as_str().map(ToOwned::to_owned))
                            .collect(),
                    })
            })
            .unwrap_or_default()
    }

    fn save(&self, path: &Path) {
        if self.permissions_added.is_empty() {
            let _ = std::fs::remove_file(path);
            return;
        }

        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let _ = std::fs::write(
            path,
            serde_json::to_string_pretty(&serde_json::json!({
                "permissions_added": self.permissions_added,
            }))
            .unwrap_or_default()
                + "\n",
        );
    }
}

fn read_json_object(path: &Path) -> serde_json::Value {
    match std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    {
        Some(value) if value.is_object() => value,
        _ => serde_json::json!({}),
    }
}

fn write_json_object(path: &Path, value: &serde_json::Value) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        path,
        serde_json::to_string_pretty(value).unwrap_or_default() + "\n",
    );
}

fn ensure_gitignore_entry(path: &Path, entry: &str) {
    let needs_entry = std::fs::read_to_string(path)
        .map(|s| !s.lines().any(|line| line == entry))
        .unwrap_or(true);
    if needs_entry
        && let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
    {
        use std::io::Write as _;
        let _ = f.write_all(format!("{entry}\n").as_bytes());
    }
}

fn claude_managed_commands() -> Vec<String> {
    let config_dir = crate::project::amf_config_dir();
    [
        "notify.sh",
        "clear-notify.sh",
        "save-prompt.sh",
        "thinking-start.sh",
        "thinking-stop.sh",
        "tool-start.sh",
        "tool-stop.sh",
    ]
    .into_iter()
    .map(|name| config_dir.join(name).to_string_lossy().into_owned())
    .collect()
}

fn is_amf_claude_hook_entry(entry: &serde_json::Value, managed_commands: &[String]) -> bool {
    entry["hooks"].as_array().is_some_and(|hooks| {
        hooks.iter().any(|hook| {
            hook["command"]
                .as_str()
                .is_some_and(|command| managed_commands.iter().any(|managed| managed == command))
        })
    })
}

fn remove_amf_claude_hooks(
    settings: &mut serde_json::Value,
    managed_commands: &[String],
) -> bool {
    let Some(hooks_obj) = settings.get_mut("hooks").and_then(|value| value.as_object_mut()) else {
        return false;
    };

    let mut changed = false;
    for event in ["Stop", "Notification", "PreToolUse", "PostToolUse", "UserPromptSubmit"] {
        let Some(entries) = hooks_obj.get_mut(event).and_then(|value| value.as_array_mut()) else {
            continue;
        };
        let before = entries.len();
        entries.retain(|entry| !is_amf_claude_hook_entry(entry, managed_commands));
        changed |= entries.len() != before;
    }

    hooks_obj.retain(|_, value| value.as_array().is_none_or(|entries| !entries.is_empty()));

    if hooks_obj.is_empty() {
        settings.as_object_mut().unwrap().remove("hooks");
        changed = true;
    }

    changed
}

fn push_claude_hook_entry(settings: &mut serde_json::Value, event: &str, entry: serde_json::Value) {
    let root = settings.as_object_mut().unwrap();
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .unwrap();
    let entries = hooks
        .entry(event.to_string())
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .unwrap();
    entries.push(entry);
}

fn update_claude_permissions(
    settings: &mut serde_json::Value,
    state_path: &Path,
    wants_diff_review: bool,
) {
    let mut state = ClaudeSettingsState::load(state_path);

    if wants_diff_review {
        {
            let perms = settings
                .as_object_mut()
                .unwrap()
                .entry("permissions")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .unwrap();
            let allow = perms
                .entry("allow")
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .unwrap();

            for permission in ["Edit", "Write"] {
                if !allow.iter().any(|value| value.as_str() == Some(permission)) {
                    allow.push(serde_json::json!(permission));
                    if !state.permissions_added.iter().any(|value| value == permission) {
                        state.permissions_added.push(permission.to_string());
                    }
                }
            }
        }
    } else if let Some(allow) = settings
        .pointer_mut("/permissions/allow")
        .and_then(|value| value.as_array_mut())
    {
        allow.retain(|value| {
            value
                .as_str()
                .is_none_or(|permission| !state.permissions_added.iter().any(|added| added == permission))
        });
        state.permissions_added.clear();
    }

    if settings
        .pointer("/permissions/allow")
        .and_then(|value| value.as_array())
        .is_some_and(|allow| allow.is_empty())
        && let Some(permissions) = settings
            .get_mut("permissions")
            .and_then(|value| value.as_object_mut())
    {
        permissions.remove("allow");
    }

    if settings
        .get("permissions")
        .and_then(|value| value.as_object())
        .is_some_and(|permissions| permissions.is_empty())
    {
        settings.as_object_mut().unwrap().remove("permissions");
    }

    state.save(state_path);
}

fn cleanup_claude_settings_file(path: &Path, state_path: Option<&Path>) {
    if !path.exists() {
        return;
    }

    let managed_commands = claude_managed_commands();
    let mut settings = read_json_object(path);
    let had_amf_hooks = remove_amf_claude_hooks(&mut settings, &managed_commands);

    if let Some(state_path) = state_path {
        update_claude_permissions(&mut settings, state_path, false);
    } else if had_amf_hooks
        && let Some(allow) = settings
            .pointer_mut("/permissions/allow")
            .and_then(|value| value.as_array_mut())
    {
        allow.retain(|value| value.as_str() != Some("Edit") && value.as_str() != Some("Write"));

        if settings
            .pointer("/permissions/allow")
            .and_then(|value| value.as_array())
            .is_some_and(|entries| entries.is_empty())
            && let Some(permissions) = settings
                .get_mut("permissions")
                .and_then(|value| value.as_object_mut())
        {
            permissions.remove("allow");
        }

        if settings
            .get("permissions")
            .and_then(|value| value.as_object())
            .is_some_and(|permissions| permissions.is_empty())
        {
            settings.as_object_mut().unwrap().remove("permissions");
        }
    }

    if settings.as_object().is_some_and(|root| root.is_empty()) {
        let _ = std::fs::remove_file(path);
    } else {
        write_json_object(path, &settings);
    }
}

fn parse_codex_notify_commands(value: Option<&toml::Value>) -> Option<Vec<String>> {
    let Some(value) = value else {
        return Some(vec![]);
    };

    if let Some(arr) = value.as_array() {
        let values: Option<Vec<String>> = arr
            .iter()
            .map(|item| item.as_str().map(ToOwned::to_owned))
            .collect();
        return values;
    }

    value.as_str().map(|command| vec![command.to_string()])
}

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
    let thinking_start_path = config_dir.join("thinking-start.sh");
    let thinking_stop_path = config_dir.join("thinking-stop.sh");
    let tool_start_path = config_dir.join("tool-start.sh");
    let tool_stop_path = config_dir.join("tool-stop.sh");
    let _ = std::fs::write(&save_prompt_path, SAVE_PROMPT_SH);
    let _ = std::fs::write(&thinking_start_path, THINKING_START_SH);
    let _ = std::fs::write(&thinking_stop_path, THINKING_STOP_SH);
    let _ = std::fs::write(&tool_start_path, TOOL_START_SH);
    let _ = std::fs::write(&tool_stop_path, TOOL_STOP_SH);
    let codex_diff_review_path = config_dir.join("codex-diff-review.sh");
    let _ = std::fs::write(&codex_diff_review_path, CODEX_DIFF_REVIEW_SH);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&save_prompt_path, std::fs::Permissions::from_mode(0o755));
        let _ =
            std::fs::set_permissions(&thinking_start_path, std::fs::Permissions::from_mode(0o755));
        let _ =
            std::fs::set_permissions(&thinking_stop_path, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::set_permissions(&tool_start_path, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::set_permissions(&tool_stop_path, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::set_permissions(
            &codex_diff_review_path,
            std::fs::Permissions::from_mode(0o755),
        );
    }
    let plugins_dir = config_dir.join("plugins");
    let _ = std::fs::create_dir_all(&plugins_dir);
    let input_request_path = plugins_dir.join("input-request.js");
    let _ = std::fs::write(&input_request_path, INPUT_REQUEST_JS);
    let change_tracker_path = plugins_dir.join("change-tracker.js");
    let _ = std::fs::write(&change_tracker_path, CHANGE_TRACKER_JS);
    let custom_diff_review_path = plugins_dir.join("custom-diff-review.sh");
    let _ = std::fs::write(&custom_diff_review_path, CUSTOM_DIFF_REVIEW_SH);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            &custom_diff_review_path,
            std::fs::Permissions::from_mode(0o755),
        );
    }
}

/// Refresh opencode plugin files in all known opencode feature
/// workdirs, so existing sessions/worktrees pick up plugin fixes
/// without requiring feature recreation.
pub fn refresh_opencode_plugins_for_store(store: &ProjectStore) -> usize {
    let mut refreshed = 0usize;
    for project in &store.projects {
        for feature in &project.features {
            if !matches!(feature.agent, AgentKind::Opencode) {
                continue;
            }
            ensure_opencode_plugins(&feature.workdir, &project.repo, &feature.mode);
            refreshed += 1;
        }
    }
    refreshed
}

pub fn refresh_claude_hooks_for_store(store: &ProjectStore, config: &AppConfig) -> usize {
    let mut refreshed = 0usize;
    for project in &store.projects {
        for feature in &project.features {
            if !matches!(feature.agent, AgentKind::Claude) {
                continue;
            }
            ensure_notification_hooks_with_config(
                &feature.workdir,
                &project.repo,
                &feature.mode,
                &feature.agent,
                feature.is_worktree,
                config,
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
    let settings_path = repo.join(".claude").join(CLAUDE_SETTINGS_LOCAL_JSON);
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
    let _ = ThemeManager::inject_opencode_themes(workdir);

    let bundled_plugins_dir = crate::project::amf_config_dir().join("plugins");
    let bundled_input_request = bundled_plugins_dir.join("input-request.js");
    let bundled_change_tracker = bundled_plugins_dir.join("change-tracker.js");
    let dst_input_request = plugins_dir.join("input-request.js");

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

    if bundled_input_request.exists() {
        let _ = std::fs::copy(&bundled_input_request, &dst_input_request);
    }

    if matches!(mode, VibeMode::Vibeless) {
        let src_change_tracker = repo.join(".opencode").join("plugins").join("change-tracker.js");

        if bundled_change_tracker.exists() {
            let _ = std::fs::copy(&bundled_change_tracker, &dst_change_tracker);
        } else if src_change_tracker.exists() {
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
    let original_notify_path = codex_dir.join("amf-codex-notify-original.json");
    let _ = std::fs::write(&hook_path, CODEX_NOTIFY_SH);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755));
    }

    let config_path = codex_dir.join(CODEX_CONFIG_TOML);
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
    let Some(mut notify_entries) = parse_codex_notify_commands(table.get("notify")) else {
        return;
    };
    if !notify_entries.iter().any(|entry| entry == &hook_cmd) {
        notify_entries.push(hook_cmd);
    }
    let _ = std::fs::remove_file(&original_notify_path);
    table.insert(
        "notify".to_string(),
        toml::Value::Array(
            notify_entries
                .into_iter()
                .map(toml::Value::String)
                .collect(),
        ),
    );

    if let Ok(rendered) = toml::to_string_pretty(&config) {
        let _ = std::fs::write(&config_path, rendered);
    }
}

fn cleanup_claude_notification_hooks(workdir: &Path) {
    let claude_dir = workdir.join(".claude");
    cleanup_claude_settings_file(
        &claude_dir.join(CLAUDE_SETTINGS_LOCAL_JSON),
        Some(&claude_dir.join(CLAUDE_STATE_JSON)),
    );
    cleanup_claude_settings_file(&claude_dir.join(CLAUDE_SETTINGS_JSON), None);

    let _ = std::fs::remove_file(claude_dir.join("latest-prompt.txt"));
    let _ = std::fs::remove_dir_all(claude_dir.join("notifications"));
}

fn cleanup_opencode_plugins(workdir: &Path) {
    let plugins_dir = workdir.join(".opencode").join("plugins");
    let themes_dir = workdir.join(".opencode").join("themes");

    for file in [
        "input-request.js",
        "diff-review.js",
        "diff-review.sh",
        "change-tracker.js",
        "feedback-prompt.sh",
        "explain.sh",
    ] {
        let _ = std::fs::remove_file(plugins_dir.join(file));
    }

    for theme in ["amf.json", "amf-tokyonight.json", "amf-catppuccin.json"] {
        let _ = std::fs::remove_file(themes_dir.join(theme));
    }
}

fn cleanup_codex_notify_hook(workdir: &Path) {
    let codex_dir = workdir.join(".codex");
    let config_path = codex_dir.join(CODEX_CONFIG_TOML);
    let hook_path = codex_dir.join("amf-codex-notify.sh");
    let original_notify_path = codex_dir.join("amf-codex-notify-original.json");
    let hook_cmd = hook_path.to_string_lossy().to_string();

    if config_path.exists()
        && let Some(mut config) = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
        && let Some(table) = config.as_table_mut()
    {
        if let Some(mut notify_entries) = parse_codex_notify_commands(table.get("notify")) {
            notify_entries.retain(|entry| entry != &hook_cmd);
            if notify_entries.is_empty() {
                table.remove("notify");
            } else {
                table.insert(
                    "notify".to_string(),
                    toml::Value::Array(
                        notify_entries
                            .into_iter()
                            .map(toml::Value::String)
                            .collect(),
                    ),
                );
            }
        }

        if table.is_empty() {
            let _ = std::fs::remove_file(&config_path);
        } else if let Ok(rendered) = toml::to_string_pretty(&config) {
            let _ = std::fs::write(&config_path, rendered);
        }
    }

    let _ = std::fs::remove_file(&hook_path);
    let _ = std::fs::remove_file(&original_notify_path);
}

pub fn cleanup_agent_injected_files(workdir: &Path, agent: &AgentKind) {
    match agent {
        AgentKind::Claude => cleanup_claude_notification_hooks(workdir),
        AgentKind::Opencode => cleanup_opencode_plugins(workdir),
        AgentKind::Codex => cleanup_codex_notify_hook(workdir),
    }
}

fn resolve_diff_review_command(
    workdir: &Path,
    repo: &Path,
    viewer: &DiffReviewViewer,
) -> Option<String> {
    let script_name = match viewer {
        DiffReviewViewer::Amf => "custom-diff-review.sh",
        DiffReviewViewer::Nvim => "diff-review.sh",
    };
    let script_suffix = ["plugins", "diff-review", "scripts", script_name];
    let amf_root = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent()?.parent()?.parent().map(PathBuf::from));

    [
        Some(workdir.to_path_buf()),
        Some(repo.to_path_buf()),
        Some(crate::project::amf_config_dir().join("plugins")),
        amf_root,
    ]
    .into_iter()
    .flatten()
    .map(|base| {
        if base
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "plugins")
        {
            base.join(script_name)
        } else {
            script_suffix.iter().fold(base, |p, s| p.join(s))
        }
    })
    .find(|p| p.exists())
    .map(|p| p.to_string_lossy().into_owned())
}

pub fn ensure_notification_hooks(
    workdir: &Path,
    repo: &Path,
    mode: &VibeMode,
    agent: &AgentKind,
    is_worktree: bool,
) {
    let config = load_config();
    ensure_notification_hooks_with_config(workdir, repo, mode, agent, is_worktree, &config);
}

pub fn ensure_notification_hooks_with_config(
    workdir: &Path,
    repo: &Path,
    mode: &VibeMode,
    agent: &AgentKind,
    _is_worktree: bool,
    config: &AppConfig,
) {
    // Feature creation / restart should not depend on startup having
    // already staged the helper scripts into ~/.config/amf.
    ensure_notify_scripts();
    remove_old_diff_review_plugin(repo);

    if matches!(agent, AgentKind::Opencode) {
        ensure_opencode_plugins(workdir, repo, mode);
        return;
    }
    if matches!(agent, AgentKind::Codex) {
        ensure_codex_notify_hook(workdir);
        return;
    }

    let claude_dir = workdir.join(".claude");
    let settings_path = claude_dir.join(CLAUDE_SETTINGS_LOCAL_JSON);
    let state_path = claude_dir.join(CLAUDE_STATE_JSON);

    let config_dir = crate::project::amf_config_dir();
    let managed_commands = claude_managed_commands();
    let notify_cmd = config_dir.join("notify.sh").to_string_lossy().into_owned();
    let clear_cmd = config_dir.join("clear-notify.sh").to_string_lossy().into_owned();
    let save_prompt_cmd = config_dir.join("save-prompt.sh").to_string_lossy().into_owned();
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

    let wants_diff_review = matches!(mode, VibeMode::Vibeless);
    let diff_review_cmd = if wants_diff_review {
        resolve_diff_review_command(workdir, repo, &config.diff_review_viewer)
    } else {
        None
    };

    let mut settings = read_json_object(&settings_path);
    remove_amf_claude_hooks(&mut settings, &managed_commands);

    // Stop: clear active thinking + write stop notification.
    push_claude_hook_entry(
        &mut settings,
        "Stop",
        serde_json::json!({
            "matcher": "",
            "hooks": [
                { "type": "command", "command": thinking_stop_cmd },
                { "type": "command", "command": notify_cmd }
            ]
        }),
    );

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
    push_claude_hook_entry(
        &mut settings,
        "PreToolUse",
        serde_json::json!({
            "matcher": if wants_diff_review && diff_review_cmd.is_some() {
                "Edit|Write"
            } else {
                ""
            },
            "hooks": pre_tool_hooks
        }),
    );

    push_claude_hook_entry(
        &mut settings,
        "PostToolUse",
        serde_json::json!({
            "matcher": "",
            "hooks": [
                { "type": "command", "command": tool_stop_cmd }
            ]
        }),
    );

    // UserPromptSubmit: set thinking + persist latest prompt.
    push_claude_hook_entry(
        &mut settings,
        "UserPromptSubmit",
        serde_json::json!({
            "matcher": "",
            "hooks": [
                { "type": "command", "command": thinking_start_cmd },
                { "type": "command", "command": save_prompt_cmd }
            ]
        }),
    );

    update_claude_permissions(&mut settings, &state_path, wants_diff_review);

    let _ = std::fs::create_dir_all(&claude_dir);
    write_json_object(&settings_path, &settings);
    cleanup_claude_settings_file(&claude_dir.join(CLAUDE_SETTINGS_JSON), None);

    // Ensure notifications/ is gitignored within .claude/
    let claude_gitignore = claude_dir.join(".gitignore");
    ensure_gitignore_entry(&claude_gitignore, "notifications/");
    ensure_gitignore_entry(&claude_gitignore, "review-notes.md");
    ensure_gitignore_entry(&claude_gitignore, "latest-prompt.txt");
}

pub fn ensure_plan_mode_claude_md(workdir: &Path, repo: &Path, enabled: bool) {
    const BEGIN: &str = "<!-- AMF:plan-instructions:begin -->";
    const END: &str = "<!-- AMF:plan-instructions:end -->";

    // The shared plan file lives at the repo root so all worktrees
    // see the same file. Store it gitignored in a local-only location.
    let plan_file = repo.join("PLAN.md");
    let plan_path_str = plan_file.to_string_lossy();

    let block = format!(
        concat!(
            "<!-- AMF:plan-instructions:begin -->\n\n",
            "## Plan Mode\n\n",
            "You are in **PLAN MODE**. A shared plan file is at:\n\n",
            "```\n",
            "{plan_file}\n",
            "```\n\n",
            "**Before doing any implementation work:**\n\n",
            "1. Read the plan file to understand the current state\n",
            "2. Update the plan with your intended approach\n",
            "3. Keep the plan updated as you make progress\n\n",
            "**Plan file format:**\n\n",
            "```markdown\n",
            "# Plan\n\n",
            "## Goal\n",
            "<overall objective>\n\n",
            "## Tasks\n",
            "- [ ] Task 1\n",
            "- [x] Completed task\n\n",
            "## Notes\n",
            "<decisions, discoveries, blockers>\n",
            "```\n\n",
            "Update task checkboxes as you complete work. Other agents\n",
            "working in parallel will read this same file.\n\n",
            "<!-- AMF:plan-instructions:end -->\n",
        ),
        plan_file = plan_path_str,
    );

    // Ensure PLAN.md is gitignored at the repo root.
    let gitignore_path = repo.join(".gitignore");
    ensure_gitignore_entry(&gitignore_path, "PLAN.md");

    // Create a skeleton PLAN.md if enabling and file doesn't exist.
    if enabled && !plan_file.exists() {
        let _ = std::fs::write(
            &plan_file,
            "# Plan\n\n## Goal\n\n<describe the overall objective>\n\n\
             ## Tasks\n\n- [ ] Task 1\n\n## Notes\n\n",
        );
    }

    // Inject/remove plan instructions from workdir's CLAUDE.local.md.
    let md_path = workdir.join("CLAUDE.local.md");
    let current = std::fs::read_to_string(&md_path).unwrap_or_default();
    let has_block = current.contains(BEGIN);

    // Ensure CLAUDE.local.md is gitignored at the workdir root.
    let wt_gitignore = workdir.join(".gitignore");
    ensure_gitignore_entry(&wt_gitignore, "CLAUDE.local.md");

    if enabled {
        if has_block {
            return; // already injected
        }
        let content = if current.is_empty() {
            block.clone()
        } else {
            format!("{}\n{}", current.trim_end(), block)
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
    ensure_gitignore_entry(&gitignore_path, "CLAUDE.local.md");

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
