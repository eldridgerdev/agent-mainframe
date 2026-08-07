use std::path::{Path, PathBuf};

use crate::project::{AgentKind, ProjectStore, VibeMode};
use crate::theme::ThemeManager;

use super::AppConfig;

const NOTIFY_SH: &str = include_str!("../../scripts/notify.sh");
const CLEAR_NOTIFY_SH: &str = include_str!("../../scripts/clear-notify.sh");
const SAVE_PROMPT_SH: &str = include_str!("../../scripts/save-prompt.sh");
const THINKING_START_SH: &str = include_str!("../../scripts/thinking-start.sh");
const THINKING_STOP_SH: &str = include_str!("../../scripts/thinking-stop.sh");
const TOOL_START_SH: &str = include_str!("../../scripts/tool-start.sh");
const TOOL_STOP_SH: &str = include_str!("../../scripts/tool-stop.sh");
const CODEX_NOTIFY_SH: &str = include_str!("../../scripts/codex-notify.sh");
const CODEX_DIFF_REVIEW_SH: &str = include_str!("../../scripts/codex-diff-review.sh");
const SET_SESSION_STATUS_SH: &str = include_str!("../../scripts/set-session-status.sh");
const INPUT_REQUEST_JS: &str = include_str!("../../.opencode/plugins/input-request.js");
const CHANGE_TRACKER_JS: &str = include_str!("../../.opencode/plugins/change-tracker.js");
const SIDEBAR_STATE_JS: &str = include_str!("../../.opencode/plugins/sidebar-state.js");
const CUSTOM_DIFF_REVIEW_SH: &str =
    include_str!("../../plugins/diff-review/scripts/custom-diff-review.sh");

const AMF_SKILLS: &[(&str, &str)] = &[
    (
        "add-session",
        include_str!("../../skills/amf-add-session/SKILL.md"),
    ),
    (
        "add-hook",
        include_str!("../../skills/amf-add-hook/SKILL.md"),
    ),
    (
        "add-preset",
        include_str!("../../skills/amf-add-preset/SKILL.md"),
    ),
    (
        "add-prompt",
        include_str!("../../skills/amf-add-prompt/SKILL.md"),
    ),
    (
        "configure",
        include_str!("../../skills/amf-configure/SKILL.md"),
    ),
    (
        "release-notes",
        include_str!("../../skills/amf-release-notes/SKILL.md"),
    ),
];
const CODEX_AMF_SKILLS: &[(&str, &str)] = &[(
    "screenshot",
    include_str!("../../skills/codex/amf-screenshot/SKILL.md"),
)];
const CLAUDE_SETTINGS_LOCAL_JSON: &str = "settings.local.json";
const CLAUDE_SETTINGS_JSON: &str = "settings.json";
const CLAUDE_STATE_JSON: &str = "amf-hook-state.json";
const HOOK_REFRESH_STAMP: &str = concat!(env!("CARGO_PKG_VERSION"), ":quoted-claude-hooks-v2");
const CLAUDE_MANAGED_SCRIPT_NAMES: &[&str] = &[
    "notify.sh",
    "clear-notify.sh",
    "save-prompt.sh",
    "thinking-start.sh",
    "thinking-stop.sh",
    "tool-start.sh",
    "tool-stop.sh",
];

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
    CLAUDE_MANAGED_SCRIPT_NAMES
        .iter()
        .map(|name| config_dir.join(name).to_string_lossy().into_owned())
        .collect()
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn shell_unquote_single(value: &str) -> String {
    value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .map(|value| value.replace("'\\''", "'"))
        .unwrap_or_else(|| value.to_string())
}

pub(crate) fn claude_hook_command(path: &Path) -> String {
    shell_quote(&path.to_string_lossy())
}

fn is_amf_claude_hook_command(command: &str, managed_commands: &[String]) -> bool {
    let normalized = shell_unquote_single(command);
    if managed_commands
        .iter()
        .any(|managed| managed == command || managed == &normalized)
    {
        return true;
    }

    // The config-dir resolver depends on HOME/XDG_CONFIG_HOME and can differ
    // across platforms or across runs (e.g. macOS' Library/Application
    // Support path, or a custom XDG_CONFIG_HOME pointed at an arbitrary
    // directory). `amf_config_dir()` always resolves to `<config root>/amf`
    // regardless of what the config root itself is named, so the only
    // structural invariant we can rely on for a *previous* run's config root
    // is that the immediate parent directory is literally named `amf`.
    // Treat only AMF's known helper names under such a directory as managed
    // so unrelated user hooks with the same basename are preserved.
    let Some((parent, name)) = normalized.rsplit_once('/') else {
        return false;
    };
    CLAUDE_MANAGED_SCRIPT_NAMES.contains(&name)
        && Path::new(parent).file_name().and_then(|n| n.to_str()) == Some("amf")
}

fn is_unquoted_amf_claude_hook_command(command: &str, managed_commands: &[String]) -> bool {
    !(command.starts_with('\'') && command.ends_with('\''))
        && is_amf_claude_hook_command(command, managed_commands)
}

fn has_unquoted_amf_claude_hooks(
    settings: &serde_json::Value,
    managed_commands: &[String],
) -> bool {
    settings
        .get("hooks")
        .and_then(|value| value.as_object())
        .is_some_and(|hooks_obj| {
            hooks_obj.values().any(|entries| {
                entries.as_array().is_some_and(|entries| {
                    entries.iter().any(|entry| {
                        entry["hooks"].as_array().is_some_and(|hooks| {
                            hooks.iter().any(|hook| {
                                hook["command"].as_str().is_some_and(|command| {
                                    is_unquoted_amf_claude_hook_command(command, managed_commands)
                                })
                            })
                        })
                    })
                })
            })
        })
}

fn is_amf_claude_hook_entry(entry: &serde_json::Value, managed_commands: &[String]) -> bool {
    entry["hooks"].as_array().is_some_and(|hooks| {
        hooks.iter().any(|hook| {
            hook["command"]
                .as_str()
                .is_some_and(|command| is_amf_claude_hook_command(command, managed_commands))
        })
    })
}

fn remove_amf_claude_hooks(settings: &mut serde_json::Value, managed_commands: &[String]) -> bool {
    let Some(hooks_obj) = settings
        .get_mut("hooks")
        .and_then(|value| value.as_object_mut())
    else {
        return false;
    };

    let mut changed = false;
    for event in [
        "Stop",
        "Notification",
        "PreToolUse",
        "PostToolUse",
        "UserPromptSubmit",
    ] {
        let Some(entries) = hooks_obj
            .get_mut(event)
            .and_then(|value| value.as_array_mut())
        else {
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
                    if !state
                        .permissions_added
                        .iter()
                        .any(|value| value == permission)
                    {
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
            value.as_str().is_none_or(|permission| {
                !state
                    .permissions_added
                    .iter()
                    .any(|added| added == permission)
            })
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

/// Atomically write `content` to `path`: stage it in a temp file in the same
/// directory (so the final `rename` stays on one filesystem and is atomic),
/// optionally set its mode, then rename over the target.
///
/// This matters because these files are hook scripts that the agent harness
/// executes. A plain in-place `fs::write` truncates and rewrites the *same*
/// inode, so a shell that is mid-execution reading the script can observe a
/// half-rewritten file and fail with a spurious syntax error. Renaming a fully
/// written temp over the target means a running hook keeps reading the complete
/// old inode while new runs pick up the complete new one — never a mix.
fn write_atomic(path: &Path, content: &str, mode: Option<u32>) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("script");
    let tmp = dir.join(format!(".{file_name}.tmp.{}", std::process::id()));

    let write_result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()?;
        #[cfg(unix)]
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(mode))?;
        }
        #[cfg(not(unix))]
        let _ = mode;
        std::fs::rename(&tmp, path)
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    write_result
}

/// Write `content` to `path` only when the on-disk content differs.
/// Avoids unnecessary write syscalls when the file is already up to date,
/// which matters because `ensure_notify_scripts` is called once per feature
/// on startup. The write itself is atomic (see `write_atomic`).
fn write_if_changed(path: &Path, content: &str) {
    if std::fs::read_to_string(path).ok().as_deref() == Some(content) {
        return;
    }
    let _ = write_atomic(path, content, None);
}

/// Same as `write_if_changed` but also ensures the file is executable.
/// Permissions are only set when the file content was actually rewritten.
#[cfg(unix)]
fn write_executable_if_changed(path: &Path, content: &str) {
    if std::fs::read_to_string(path).ok().as_deref() == Some(content) {
        return;
    }
    let _ = write_atomic(path, content, Some(0o755));
}

#[cfg(not(unix))]
fn write_executable_if_changed(path: &Path, content: &str) {
    write_if_changed(path, content);
}

pub fn ensure_notify_scripts() {
    let config_dir = crate::project::amf_config_dir();
    let _ = std::fs::create_dir_all(&config_dir);
    write_executable_if_changed(&config_dir.join("notify.sh"), NOTIFY_SH);
    write_executable_if_changed(&config_dir.join("clear-notify.sh"), CLEAR_NOTIFY_SH);
    write_executable_if_changed(&config_dir.join("save-prompt.sh"), SAVE_PROMPT_SH);
    write_executable_if_changed(&config_dir.join("thinking-start.sh"), THINKING_START_SH);
    write_executable_if_changed(&config_dir.join("thinking-stop.sh"), THINKING_STOP_SH);
    write_executable_if_changed(&config_dir.join("tool-start.sh"), TOOL_START_SH);
    write_executable_if_changed(&config_dir.join("tool-stop.sh"), TOOL_STOP_SH);
    write_executable_if_changed(
        &config_dir.join("codex-diff-review.sh"),
        CODEX_DIFF_REVIEW_SH,
    );
    write_executable_if_changed(&config_dir.join("codex-notify.sh"), CODEX_NOTIFY_SH);
    write_executable_if_changed(
        &config_dir.join("set-session-status.sh"),
        SET_SESSION_STATUS_SH,
    );
    let plugins_dir = config_dir.join("plugins");
    let _ = std::fs::create_dir_all(&plugins_dir);
    write_if_changed(&plugins_dir.join("input-request.js"), INPUT_REQUEST_JS);
    write_if_changed(&plugins_dir.join("change-tracker.js"), CHANGE_TRACKER_JS);
    write_executable_if_changed(
        &plugins_dir.join("custom-diff-review.sh"),
        CUSTOM_DIFF_REVIEW_SH,
    );
}

/// Path of the version-stamp file used to skip hook/plugin refresh when
/// the AMF binary version has not changed since the last startup.
fn hook_refresh_stamp_path() -> std::path::PathBuf {
    crate::project::amf_config_dir().join("last_hook_refresh_version")
}

/// Returns `true` when the stamp matches the current binary version, meaning
/// every feature's hooks and plugins were already refreshed by this exact
/// version of AMF and nothing needs to be rewritten.
fn hooks_already_current() -> bool {
    std::fs::read_to_string(hook_refresh_stamp_path())
        .ok()
        .as_deref()
        == Some(HOOK_REFRESH_STAMP)
}

/// Record that hooks have been refreshed for the current binary version.
fn mark_hooks_current() {
    let _ = std::fs::write(hook_refresh_stamp_path(), HOOK_REFRESH_STAMP);
}

/// Refresh opencode plugin files in all known opencode feature
/// workdirs, so existing sessions/worktrees pick up plugin fixes
/// without requiring feature recreation.
///
/// Writes the version stamp after running so that subsequent startups
/// on the same binary version skip both this pass and the Claude hooks
/// pass (which runs first and also checks the stamp).
pub fn refresh_opencode_plugins_for_store(store: &ProjectStore) -> usize {
    if hooks_already_current() {
        return 0;
    }
    let mut refreshed = 0usize;
    for project in &store.projects {
        for feature in &project.features {
            if !matches!(feature.agent, AgentKind::Opencode) {
                continue;
            }
            ensure_opencode_plugins(&feature.workdir, &feature.mode);
            refreshed += 1;
        }
    }
    // Both refresh passes are now complete for this version; stamp so the
    // next startup skips both.
    mark_hooks_current();
    refreshed
}

pub fn refresh_claude_hooks_for_store(store: &ProjectStore) -> usize {
    if hooks_already_current() {
        return 0;
    }
    let mut refreshed = 0usize;
    for project in &store.projects {
        for feature in &project.features {
            if !matches!(feature.agent, AgentKind::Claude) {
                continue;
            }
            ensure_notification_hooks(
                &feature.workdir,
                &project.repo,
                &feature.mode,
                &feature.agent,
                feature.is_worktree,
            );
            refreshed += 1;
        }
    }
    // Do not write the stamp here; the opencode pass runs next and writes it
    // once both passes are done.  If this store has no opencode features the
    // opencode function still runs (empty loop) and stamps correctly.
    refreshed
}

pub fn repair_unquoted_claude_hooks_for_store(store: &ProjectStore) -> usize {
    let managed_commands = claude_managed_commands();
    let mut repaired = 0usize;
    for project in &store.projects {
        for feature in &project.features {
            if !matches!(feature.agent, AgentKind::Claude) {
                continue;
            }
            let settings_path = feature
                .workdir
                .join(".claude")
                .join(CLAUDE_SETTINGS_LOCAL_JSON);
            let settings = read_json_object(&settings_path);
            if !has_unquoted_amf_claude_hooks(&settings, &managed_commands) {
                continue;
            }
            ensure_notification_hooks(
                &feature.workdir,
                &project.repo,
                &feature.mode,
                &feature.agent,
                feature.is_worktree,
            );
            repaired += 1;
        }
    }
    repaired
}

/// Apply one-time, version-gated migrations to a freshly loaded config.
/// Returns `true` if anything changed and the file should be rewritten.
///
/// Each step only runs when `config_version` is below its version, and
/// only rewrites a value that still equals the old default — a user who
/// deliberately set the value after migrating is never clobbered, because
/// the version bump stops the step from firing twice.
pub(crate) fn migrate_app_config(config: &mut AppConfig) -> bool {
    let mut changed = false;

    if config.config_version < 1 {
        // v1: the persistent control-mode (-CC PTY) transport corrupts the
        // agent pane on Unix, so move pre-flip configs to the new direct
        // default. Also shorten the diff-review popup hold from its old 3.0s
        // default to the new 1.5s.
        if config.tmux_control_mode {
            config.tmux_control_mode = false;
            changed = true;
        }
        if (config.diff_review_popup_hold_secs - 3.0).abs() < f64::EPSILON {
            config.diff_review_popup_hold_secs = 1.5;
            changed = true;
        }
    }

    if config.config_version < crate::app::APP_CONFIG_VERSION {
        config.config_version = crate::app::APP_CONFIG_VERSION;
        changed = true;
    }

    changed
}

pub fn load_config() -> AppConfig {
    let config_path = crate::project::amf_config_dir().join("config.json");

    let config = if config_path.exists() {
        let mut config: AppConfig = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        if migrate_app_config(&mut config) {
            let _ = std::fs::write(
                &config_path,
                serde_json::to_string_pretty(&config).unwrap_or_default(),
            );
        }
        config
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

fn ensure_opencode_plugins(workdir: &Path, mode: &VibeMode) {
    let plugins_dir = workdir.join(".opencode").join("plugins");
    let _ = std::fs::create_dir_all(&plugins_dir);
    let _ = ThemeManager::inject_opencode_themes(workdir);

    let bundled_plugins_dir = crate::project::amf_config_dir().join("plugins");
    let bundled_input_request = bundled_plugins_dir.join("input-request.js");
    let dst_input_request = plugins_dir.join("input-request.js");

    let dst_change_tracker = plugins_dir.join("change-tracker.js");
    let dst_sidebar_state = plugins_dir.join("sidebar-state.js");

    // Remove stale script-based diff-review artifacts injected by older
    // versions (the native AMF diff viewer replaced them).
    for stale in [
        "diff-review.js",
        "diff-review.sh",
        "feedback-prompt.sh",
        "explain.sh",
    ] {
        let _ = std::fs::remove_file(plugins_dir.join(stale));
    }
    let _ = std::fs::remove_file(&dst_change_tracker);
    let _ = std::fs::remove_file(&dst_sidebar_state);

    if bundled_input_request.exists() {
        let _ = std::fs::copy(&bundled_input_request, &dst_input_request);
    }
    let _ = std::fs::write(&dst_sidebar_state, SIDEBAR_STATE_JS);

    if matches!(mode, VibeMode::Vibeless) {
        let _ = std::fs::write(&dst_change_tracker, CHANGE_TRACKER_JS);
    }
}

fn ensure_codex_notify_hook(workdir: &Path) {
    let codex_dir = workdir.join(".codex");
    let _ = std::fs::create_dir_all(&codex_dir);

    let hook_path = codex_dir.join("amf-codex-notify.sh");
    let _ = std::fs::write(&hook_path, CODEX_NOTIFY_SH);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755));
    }

    let global_hook_path = crate::project::amf_config_dir().join("codex-notify.sh");
    crate::codex_config::ensure_user_config_notify_hook(&global_hook_path);

    let project_config_path = codex_dir.join("config.toml");
    if project_config_path.exists() {
        let mut config = std::fs::read_to_string(&project_config_path)
            .ok()
            .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));

        if let Some(table) = config.as_table_mut() {
            table.remove("notify");
            if table.is_empty() {
                let _ = std::fs::remove_file(&project_config_path);
            } else if let Ok(rendered) = toml::to_string_pretty(&config) {
                let _ = std::fs::write(&project_config_path, rendered + "\n");
            }
        }
    }
}

fn agent_skills_dir(workdir: &Path, agent: &AgentKind) -> Option<PathBuf> {
    match agent {
        AgentKind::Claude => Some(workdir.join(".claude").join("skills")),
        AgentKind::Opencode => Some(workdir.join(".opencode").join("skills")),
        AgentKind::Codex => Some(workdir.join(".agents").join("skills")),
        AgentKind::Pi => None,
    }
}

fn agent_specific_amf_skills(agent: &AgentKind) -> &'static [(&'static str, &'static str)] {
    match agent {
        AgentKind::Codex => CODEX_AMF_SKILLS,
        _ => &[],
    }
}

fn ensure_amf_skills(workdir: &Path, agent: &AgentKind) {
    let Some(skills_dir) = agent_skills_dir(workdir, agent) else {
        return;
    };
    for (name, content) in AMF_SKILLS.iter().chain(agent_specific_amf_skills(agent)) {
        let skill_dir = skills_dir.join(format!("amf-{name}"));
        let skill_path = skill_dir.join("SKILL.md");
        if std::fs::read_to_string(&skill_path).ok().as_deref() == Some(content) {
            continue;
        }
        let _ = std::fs::create_dir_all(&skill_dir);
        let _ = std::fs::write(&skill_path, content);
    }
}

fn cleanup_amf_skills(workdir: &Path, agent: &AgentKind) {
    let Some(skills_dir) = agent_skills_dir(workdir, agent) else {
        return;
    };
    for (name, _) in AMF_SKILLS.iter().chain(agent_specific_amf_skills(agent)) {
        let _ = std::fs::remove_dir_all(skills_dir.join(format!("amf-{name}")));
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
    cleanup_amf_skills(workdir, &AgentKind::Claude);
}

fn cleanup_opencode_plugins(workdir: &Path) {
    let plugins_dir = workdir.join(".opencode").join("plugins");
    let themes_dir = workdir.join(".opencode").join("themes");
    let sidebar_dir = workdir.join(".amf").join("opencode-sidebar");

    for file in [
        "input-request.js",
        "diff-review.js",
        "diff-review.sh",
        "change-tracker.js",
        "sidebar-state.js",
        "feedback-prompt.sh",
        "explain.sh",
    ] {
        let _ = std::fs::remove_file(plugins_dir.join(file));
    }

    for theme in [
        "amf.json",
        "amf-tokyonight.json",
        "amf-catppuccin.json",
        "amf-gruvbox.json",
    ] {
        let _ = std::fs::remove_file(themes_dir.join(theme));
    }

    let _ = std::fs::remove_dir_all(sidebar_dir);
    cleanup_amf_skills(workdir, &AgentKind::Opencode);
}

fn cleanup_codex_notify_hook(workdir: &Path) {
    let codex_dir = workdir.join(".codex");
    let hook_path = codex_dir.join("amf-codex-notify.sh");

    let _ = std::fs::remove_file(&hook_path);
    let project_config_path = codex_dir.join("config.toml");
    if project_config_path.exists() {
        let mut config = std::fs::read_to_string(&project_config_path)
            .ok()
            .and_then(|s| toml::from_str::<toml::Value>(&s).ok())
            .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));

        if let Some(table) = config.as_table_mut() {
            table.remove("notify");
            if table.is_empty() {
                let _ = std::fs::remove_file(&project_config_path);
            } else if let Ok(rendered) = toml::to_string_pretty(&config) {
                let _ = std::fs::write(&project_config_path, rendered + "\n");
            }
        }
    }
    cleanup_amf_skills(workdir, &AgentKind::Codex);
}

pub fn cleanup_agent_injected_files(workdir: &Path, agent: &AgentKind) {
    match agent {
        AgentKind::Claude => cleanup_claude_notification_hooks(workdir),
        AgentKind::Opencode => cleanup_opencode_plugins(workdir),
        AgentKind::Codex => cleanup_codex_notify_hook(workdir),
        AgentKind::Pi => {}
    }
}

fn resolve_diff_review_command(workdir: &Path, repo: &Path) -> Option<String> {
    let script_name = "custom-diff-review.sh";
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
    _is_worktree: bool,
) {
    // Feature creation / restart should not depend on startup having
    // already staged the helper scripts into ~/.config/amf.
    ensure_notify_scripts();
    remove_old_diff_review_plugin(repo);

    if matches!(agent, AgentKind::Opencode) {
        ensure_opencode_plugins(workdir, mode);
        ensure_amf_skills(workdir, agent);
        return;
    }
    if matches!(agent, AgentKind::Codex) {
        ensure_codex_notify_hook(workdir);
        ensure_amf_skills(workdir, agent);
        return;
    }

    let claude_dir = workdir.join(".claude");
    let settings_path = claude_dir.join(CLAUDE_SETTINGS_LOCAL_JSON);
    let state_path = claude_dir.join(CLAUDE_STATE_JSON);

    let config_dir = crate::project::amf_config_dir();
    let managed_commands = claude_managed_commands();
    let notify_cmd = claude_hook_command(&config_dir.join("notify.sh"));
    let clear_cmd = claude_hook_command(&config_dir.join("clear-notify.sh"));
    let save_prompt_cmd = claude_hook_command(&config_dir.join("save-prompt.sh"));
    let thinking_start_cmd = claude_hook_command(&config_dir.join("thinking-start.sh"));
    let thinking_stop_cmd = claude_hook_command(&config_dir.join("thinking-stop.sh"));
    let tool_start_cmd = claude_hook_command(&config_dir.join("tool-start.sh"));
    let tool_stop_cmd = claude_hook_command(&config_dir.join("tool-stop.sh"));

    let wants_diff_review = matches!(mode, VibeMode::Vibeless);
    let diff_review_cmd = if wants_diff_review {
        resolve_diff_review_command(workdir, repo)
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
    if wants_diff_review && let Some(ref dr_cmd) = diff_review_cmd {
        let dr_cmd = shell_quote(dr_cmd);
        pre_tool_hooks.push(serde_json::json!({
            "type": "command",
            "command": dr_cmd,
            "timeout": 600
        }));
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
    ensure_gitignore_entry(&claude_gitignore, "review-notes-archive.md");
    ensure_gitignore_entry(&claude_gitignore, "final-review-feedback.md");
    ensure_gitignore_entry(&claude_gitignore, "final-review-feedback-archive.md");
    ensure_gitignore_entry(&claude_gitignore, "latest-prompt.txt");
    ensure_gitignore_entry(&claude_gitignore, "skills/amf-*/");

    ensure_amf_skills(workdir, agent);
}

pub fn ensure_plan_mode_instructions(workdir: &Path, agent: &AgentKind, enabled: bool) {
    const BEGIN: &str = "<!-- AMF:plan-instructions:begin -->";
    const END: &str = "<!-- AMF:plan-instructions:end -->";
    const BLOCK: &str = concat!(
        "<!-- AMF:plan-instructions:begin -->\n\n",
        "## Plan Mode\n\n",
        "This feature has a user-authored plan at `.claude/plan.md`.\n\n",
        "Before doing implementation work, read the plan. Treat its decisions ",
        "as settled unless the user says otherwise, and keep its task ",
        "checkboxes and notes current as work progresses.\n\n",
        "<!-- AMF:plan-instructions:end -->\n",
    );
    const AGENTS_IGNORE_BEGIN: &str = "# AMF:plan-instructions:begin";
    const AGENTS_IGNORE_END: &str = "# AMF:plan-instructions:end";

    let uses_claude_local = matches!(agent, AgentKind::Claude);
    let md_path = workdir.join(if uses_claude_local {
        "CLAUDE.local.md"
    } else {
        "AGENTS.md"
    });
    let instruction_file_existed = md_path.exists();
    let current = std::fs::read_to_string(&md_path).unwrap_or_default();
    let has_block = current.contains(BEGIN);
    let gitignore_path = workdir.join(".gitignore");

    if uses_claude_local && enabled {
        ensure_gitignore_entry(&gitignore_path, "CLAUDE.local.md");
    } else if enabled && !instruction_file_existed {
        let ignore = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
        if !ignore.lines().any(|line| line == "AGENTS.md") {
            let managed_ignore = format!("{AGENTS_IGNORE_BEGIN}\nAGENTS.md\n{AGENTS_IGNORE_END}\n");
            let content = if ignore.is_empty() {
                managed_ignore
            } else {
                format!("{}\n{}", ignore.trim_end(), managed_ignore)
            };
            let _ = std::fs::write(&gitignore_path, content);
        }
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
    } else {
        if has_block {
            let stripped = strip_between_markers(&current, BEGIN, END);
            if stripped.trim().is_empty() {
                let _ = std::fs::remove_file(&md_path);
            } else {
                let _ = std::fs::write(&md_path, format!("{}\n", stripped.trim_end()));
            }
        }

        if !uses_claude_local
            && let Ok(ignore) = std::fs::read_to_string(&gitignore_path)
            && ignore.contains(AGENTS_IGNORE_BEGIN)
        {
            let stripped = strip_between_markers(&ignore, AGENTS_IGNORE_BEGIN, AGENTS_IGNORE_END);
            if stripped.trim().is_empty() {
                let _ = std::fs::remove_file(&gitignore_path);
            } else {
                let _ = std::fs::write(&gitignore_path, format!("{}\n", stripped.trim_end()));
            }
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
        let begin_pos = if s[..bi].ends_with("\n\n") {
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
        "You are in **REVIEW MODE**. When you finish a logical ",
        "batch of related changes (not after every individual ",
        "Edit/Write — batch it), append one section per file you ",
        "touched in that batch to `.claude/review-notes.md`. Keep ",
        "each note to 1-2 sentences: what changed and why. Skip a ",
        "file if it already has a note from an earlier batch and ",
        "there is nothing new to add.\n\n",
        "AMF automatically moves older and superseded notes to ",
        "`.claude/review-notes-archive.md` after each turn. Only ",
        "inspect the bounded live file when deciding what to append; ",
        "the archive is reviewer history and does not need to be read.\n\n",
        "Use this exact format per file:\n\n",
        "```\n",
        "## <relative-file-path> — <brief title>\n\n",
        "<1-2 sentence explanation>\n\n",
        "---\n",
        "```\n\n",
        "Batching keeps the reviewer's context of your reasoning\n",
        "without a round-trip per edit.\n\n",
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
        let claude_dir = workdir.join(".claude");
        let _ = std::fs::create_dir_all(&claude_dir);
        let claude_gitignore = claude_dir.join(".gitignore");
        ensure_gitignore_entry(&claude_gitignore, "review-notes.md");
        ensure_gitignore_entry(&claude_gitignore, "review-notes-archive.md");
        if let Err(err) = super::review::archive_review_notes(workdir) {
            crate::debug::log_to_file(
                crate::debug::LogLevel::Warn,
                "review",
                &format!("failed to cap review notes during setup: {err}"),
            );
        }

        // Replace an older AMF-managed block as well as injecting a missing
        // one, so existing Review Mode features receive current retention
        // guidance after an AMF upgrade.
        let unmanaged = if has_block {
            strip_between_markers(&current, BEGIN, END)
        } else {
            current.clone()
        };
        let content = if unmanaged.trim().is_empty() {
            BLOCK.to_string()
        } else {
            format!("{}\n{}", unmanaged.trim_end(), BLOCK)
        };
        if content != current {
            let _ = std::fs::write(&md_path, content);
        }
    } else if has_block {
        let stripped = strip_between_markers(&current, BEGIN, END);
        if stripped.trim().is_empty() {
            let _ = std::fs::remove_file(&md_path);
        } else {
            let _ = std::fs::write(&md_path, format!("{}\n", stripped.trim_end()));
        }
    }
}

#[cfg(test)]
mod atomic_write_tests {
    use super::*;
    use tempfile::TempDir;

    /// The staged temp file must not be left behind after a successful write —
    /// a lingering `.name.tmp.<pid>` would clutter the config dir.
    fn temp_files_in(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| entry.file_name().into_string().ok())
            .filter(|name| name.contains(".tmp."))
            .collect()
    }

    #[test]
    fn write_atomic_writes_content_and_leaves_no_temp() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hook.sh");

        write_atomic(&path, "echo hi\n", None).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "echo hi\n");
        assert!(
            temp_files_in(dir.path()).is_empty(),
            "no staging temp file should remain"
        );
    }

    #[test]
    fn write_atomic_replaces_existing_content() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hook.sh");
        std::fs::write(&path, "old\n").unwrap();

        write_atomic(&path, "new\n", None).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_sets_executable_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hook.sh");

        write_atomic(&path, "#!/bin/sh\n", Some(0o755)).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755);
    }

    #[test]
    fn write_executable_if_changed_skips_when_unchanged() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hook.sh");
        write_executable_if_changed(&path, "same\n");
        let first = std::fs::metadata(&path).unwrap().modified().unwrap();

        // A second call with identical content must not rewrite the file.
        std::thread::sleep(std::time::Duration::from_millis(10));
        write_executable_if_changed(&path, "same\n");
        let second = std::fs::metadata(&path).unwrap().modified().unwrap();

        assert_eq!(first, second, "unchanged content should not be rewritten");
    }
}
