mod state;

use anyhow::Result;
use ratatui_explorer::FileExplorer;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;

const NOTIFY_SH: &str =
    include_str!("../../scripts/notify.sh");
const CLEAR_NOTIFY_SH: &str =
    include_str!("../../scripts/clear-notify.sh");
const INPUT_REQUEST_JS: &str =
    include_str!("../../.opencode/plugins/input-request.js");

use crate::extension::{
    merge_project_extension_config, ExtensionConfig,
    load_global_extension_config,
};
use crate::project::{
    AgentKind, Feature, FeatureSession, Project, ProjectStatus,
    ProjectStore, SessionKind, VibeMode,
};
use crate::tmux::TmuxManager;
use crate::traits::{TmuxOps, WorktreeOps};
use crate::usage::UsageManager;
use crate::worktree::WorktreeManager;

pub use state::*;

fn shorten_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

fn slugify(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

pub struct CommandEntry {
    pub name: String,
    pub source: String,
    pub path: PathBuf,
}

pub struct CommandPickerState {
    pub commands: Vec<CommandEntry>,
    pub selected: usize,
    pub from_view: Option<ViewState>,
}

pub struct SwitcherEntry {
    pub tmux_window: String,
    pub kind: SessionKind,
    pub label: String,
    pub icon: Option<String>,
    pub icon_nerd: Option<String>,
}

pub struct SessionSwitcherState {
    pub project_name: String,
    pub feature_name: String,
    pub tmux_session: String,
    pub sessions: Vec<SwitcherEntry>,
    pub selected: usize,
    pub return_window: String,
    pub return_label: String,
    pub vibe_mode: VibeMode,
    pub review: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ZaiPlanConfig {
    pub plan: String,
    pub monthly_token_limit: Option<u64>,
    pub weekly_token_limit: Option<u64>,
    pub five_hour_token_limit: Option<u64>,
}

impl Default for ZaiPlanConfig {
    fn default() -> Self {
        Self {
            plan: "free".to_string(),
            monthly_token_limit: None,
            weekly_token_limit: None,
            five_hour_token_limit: None,
        }
    }
}

impl ZaiPlanConfig {
    pub fn get_monthly_limit(&self) -> Option<u64> {
        self.monthly_token_limit.or(match self.plan.as_str() {
            "free" => Some(10_000_000),
            "coding-plan" => Some(500_000_000),
            "unlimited" => None,
            _ => None,
        })
    }

    pub fn get_weekly_limit(&self) -> Option<u64> {
        self.weekly_token_limit.or(match self.plan.as_str() {
            "free" => Some(2_500_000),
            "coding-plan" => Some(125_000_000),
            "unlimited" => None,
            _ => None,
        })
    }

    pub fn get_five_hour_limit(&self) -> Option<u64> {
        self.five_hour_token_limit.or(match self.plan.as_str() {
            "free" => Some(500_000),
            "coding-plan" => Some(25_000_000),
            "unlimited" => None,
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub nerd_font: bool,
    pub zai: ZaiPlanConfig,
    pub opencode_theme: Option<String>,
    pub extension: ExtensionConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            nerd_font: true,
            zai: ZaiPlanConfig::default(),
            opencode_theme: Some("catppuccin-frappe".to_string()),
            extension: ExtensionConfig::default(),
        }
    }
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
            serde_json::to_string_pretty(&config)
                .unwrap_or_default(),
        );
        config
    };

    if let Some(ref theme) = config.opencode_theme {
        let _ = update_opencode_theme(theme);
    }

    config
}

fn update_opencode_theme(theme: &str) -> anyhow::Result<()> {
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

fn remove_old_diff_review_plugin(repo: &Path) {
    let settings_path =
        repo.join(".claude").join("settings.local.json");
    if !settings_path.exists() {
        return;
    }

    let mut settings: serde_json::Value =
        match std::fs::read_to_string(&settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(v) => v,
            None => return,
        };

    let changed = settings
        .get_mut("enabledPlugins")
        .and_then(|p| p.as_object_mut())
        .map(|obj| {
            obj.remove("diff-review@claude_vibeless")
                .is_some()
        })
        .unwrap_or(false);

    if !changed {
        return;
    }

    if settings
        .get("enabledPlugins")
        .and_then(|p| p.as_object())
        .is_some_and(|obj| obj.is_empty())
    {
        settings
            .as_object_mut()
            .unwrap()
            .remove("enabledPlugins");
    }

    if settings.as_object().is_some_and(|obj| obj.is_empty())
    {
        let _ = std::fs::remove_file(&settings_path);
    } else {
        let _ = std::fs::write(
            &settings_path,
            serde_json::to_string_pretty(&settings)
                .unwrap_or_default()
                + "\n",
        );
    }
}

fn ensure_opencode_plugins(
    workdir: &Path,
    repo: &Path,
    mode: &VibeMode,
) {
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

        let src_explain = repo
            .join(".opencode")
            .join("plugins")
            .join("explain.sh");

        if src_explain.exists() {
            let _ = std::fs::copy(&src_explain, &dst_explain);
        }
    }
}

pub fn ensure_notification_hooks(
    workdir: &Path,
    repo: &Path,
    mode: &VibeMode,
    agent: &AgentKind,
) {
    remove_old_diff_review_plugin(repo);

    if matches!(agent, AgentKind::Opencode) {
        ensure_opencode_plugins(workdir, repo, mode);
        return;
    }

    let config_subdir = match agent {
        AgentKind::Claude => ".claude",
        AgentKind::Opencode => ".opencode",
    };
    let claude_dir = workdir.join(config_subdir);
    let settings_path = claude_dir.join("settings.json");

    let config_dir = crate::project::amf_config_dir();

    let notify_cmd =
        config_dir.join("notify.sh").to_string_lossy().to_string();
    let clear_cmd = config_dir
        .join("clear-notify.sh")
        .to_string_lossy()
        .to_string();
    let script_suffix = ["plugins", "diff-review", "scripts", "diff-review.sh"];
    let amf_root = std::env::current_exe().ok().and_then(|exe| {
        exe.parent()?.parent()?.parent().map(PathBuf::from)
    });
    let diff_review_path = [
        Some(workdir.to_path_buf()),
        Some(repo.to_path_buf()),
        amf_root,
    ]
    .into_iter()
    .flatten()
    .map(|base| script_suffix.iter().fold(base, |p, s| p.join(s)))
    .find(|p| p.exists());

    let diff_review_cmd = match diff_review_path {
        Some(p) => p.to_string_lossy().to_string(),
        None => return,
    };

    let wants_diff_review = matches!(mode, VibeMode::Vibeless);

    let mut settings: serde_json::Value =
        if settings_path.exists() {
            std::fs::read_to_string(&settings_path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
                .unwrap_or_else(|| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

    let has_notification = settings
        .get("hooks")
        .is_some_and(|h| h.get("Notification").is_some());
    let has_stop = settings
        .get("hooks")
        .is_some_and(|h| h.get("Stop").is_some());
    let has_pre_tool_use = settings
        .get("hooks")
        .is_some_and(|h| h.get("PreToolUse").is_some());
    let has_perms = settings
        .get("permissions")
        .and_then(|p| p.get("allow"))
        .and_then(|a| a.as_array())
        .is_some_and(|arr| {
            arr.iter().any(|v| v.as_str() == Some("Edit"))
                && arr
                    .iter()
                    .any(|v| v.as_str() == Some("Write"))
        });

    let already_correct = has_notification
        && has_stop
        && if wants_diff_review {
            has_pre_tool_use && has_perms
        } else {
            !has_pre_tool_use && !has_perms
        };
    if already_correct {
        return;
    }

    let hooks = settings
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));

    let notification_hook = serde_json::json!([{
        "matcher": "",
        "hooks": [{ "type": "command", "command": notify_cmd }]
    }]);
    let stop_hook = serde_json::json!([{
        "matcher": "",
        "hooks": [{ "type": "command", "command": clear_cmd }]
    }]);

    let hooks_obj = hooks.as_object_mut().unwrap();
    hooks_obj
        .entry("Notification")
        .or_insert(notification_hook);
    hooks_obj.entry("Stop").or_insert(stop_hook);

    if wants_diff_review {
        let pre_tool_use_hook = serde_json::json!([{
            "matcher": "Edit|Write",
            "hooks": [{
                "type": "command",
                "command": diff_review_cmd,
                "timeout": 600
            }]
        }]);
        hooks_obj
            .entry("PreToolUse")
            .or_insert(pre_tool_use_hook);
    } else {
        hooks_obj.remove("PreToolUse");
    }

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
        if !arr.iter().any(|v| v.as_str() == Some("Write"))
        {
            arr.push(serde_json::json!("Write"));
        }
    } else if let Some(arr) = settings
        .pointer_mut("/permissions/allow")
        .and_then(|v| v.as_array_mut())
    {
        arr.retain(|v| {
            v.as_str() != Some("Edit")
                && v.as_str() != Some("Write")
        });
    }

    let _ = std::fs::create_dir_all(&claude_dir);
    let _ = std::fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings)
            .unwrap_or_default(),
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
    let needs_review_entry =
        std::fs::read_to_string(&claude_gitignore)
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
}

fn strip_between_markers(s: &str, begin: &str, end: &str) -> String {
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

fn ensure_review_claude_md(workdir: &Path, enabled: bool) {
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
    let current =
        std::fs::read_to_string(&md_path).unwrap_or_default();
    let has_block = current.contains(BEGIN);

    // Ensure CLAUDE.local.md is gitignored at the workdir root.
    let gitignore_path = workdir.join(".gitignore");
    let needs_ignore =
        std::fs::read_to_string(&gitignore_path)
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
        let stripped =
            strip_between_markers(&current, BEGIN, END);
        if stripped.trim().is_empty() {
            let _ = std::fs::remove_file(&md_path);
        } else {
            let _ = std::fs::write(
                &md_path,
                format!("{}\n", stripped.trim_end()),
            );
        }
    }
}

fn detect_repo_path() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    WorktreeManager::repo_root(&cwd)
        .unwrap_or(cwd)
        .to_string_lossy()
        .into_owned()
}

fn detect_branch() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    WorktreeManager::current_branch(&cwd)
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub struct App {
    pub store: ProjectStore,
    pub store_path: PathBuf,
    pub config: AppConfig,
    pub active_extension: ExtensionConfig,
    pub selection: Selection,
    pub mode: AppMode,
    pub message: Option<String>,
    pub should_quit: bool,
    pub should_switch: Option<String>,
    pub pane_content: String,
    pub tmux_cursor: Option<(u16, u16)>,
    pub leader_active: bool,
    pub leader_activated_at: Option<Instant>,
    pub pending_inputs: Vec<PendingInput>,
    pub usage: UsageManager,
    pub scroll_offset: usize,
    pub session_filter: SessionFilter,
    pub throbber_state: throbber_widgets_tui::ThrobberState,
    pub thinking_features: std::collections::HashSet<String>,
    pub last_timer_values: std::collections::HashMap<String, String>,
    pub tmux: Box<dyn TmuxOps>,
    pub worktree: Box<dyn WorktreeOps>,
}

fn ensure_notify_scripts() {
    let config_dir = crate::project::amf_config_dir();
    let _ = std::fs::create_dir_all(&config_dir);
    let notify_path = config_dir.join("notify.sh");
    let clear_path = config_dir.join("clear-notify.sh");
    let _ = std::fs::write(&notify_path, NOTIFY_SH);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            &notify_path,
            std::fs::Permissions::from_mode(0o755),
        );
    }
    let _ = std::fs::write(&clear_path, CLEAR_NOTIFY_SH);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            &clear_path,
            std::fs::Permissions::from_mode(0o755),
        );
    }
    let plugins_dir = config_dir.join("plugins");
    let _ = std::fs::create_dir_all(&plugins_dir);
    let input_request_path = plugins_dir.join("input-request.js");
    let _ = std::fs::write(&input_request_path, INPUT_REQUEST_JS);
}

impl App {
    pub fn new(store_path: PathBuf) -> Result<Self> {
        ensure_notify_scripts();
        crate::project::migrate_from_old_path();
        let store = ProjectStore::load(&store_path)?;
        let config = load_config();
        let zai_monthly = config.zai.get_monthly_limit();
        let zai_weekly = config.zai.get_weekly_limit();
        let zai_five_hour = config.zai.get_five_hour_limit();
        let global_ext = load_global_extension_config();
        let active_extension = store
            .projects
            .first()
            .map(|p| {
                merge_project_extension_config(
                    &global_ext,
                    &p.repo,
                )
            })
            .unwrap_or(global_ext);
        Ok(Self {
            store,
            store_path,
            config,
            active_extension,
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            message: None,
            should_quit: false,
            should_switch: None,
            pane_content: String::new(),
            tmux_cursor: None,
            leader_active: false,
            leader_activated_at: None,
            pending_inputs: Vec::new(),
            usage: UsageManager::new(zai_monthly, zai_weekly, zai_five_hour),
            scroll_offset: 0,
            session_filter: SessionFilter::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            thinking_features: std::collections::HashSet::new(),
            last_timer_values: std::collections::HashMap::new(),
            tmux: Box::new(TmuxManager),
            worktree: Box::new(WorktreeManager),
        })
    }

    /// Lightweight constructor for unit/integration tests.
    ///
    /// Accepts a pre-built `ProjectStore` and injected trait objects so
    /// tests can drive App logic without touching the filesystem or
    /// spawning real tmux sessions.
    #[cfg(test)]
    pub fn new_for_test(
        store: ProjectStore,
        tmux: Box<dyn TmuxOps>,
        worktree: Box<dyn WorktreeOps>,
    ) -> Self {
        use crate::extension::ExtensionConfig;
        Self {
            store,
            store_path: PathBuf::new(),
            config: AppConfig::default(),
            active_extension: ExtensionConfig::default(),
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            message: None,
            should_quit: false,
            should_switch: None,
            pane_content: String::new(),
            tmux_cursor: None,
            leader_active: false,
            leader_activated_at: None,
            pending_inputs: Vec::new(),
            usage: UsageManager::new(None, None, None),
            scroll_offset: 0,
            session_filter: SessionFilter::default(),
            throbber_state:
                throbber_widgets_tui::ThrobberState::default(),
            thinking_features:
                std::collections::HashSet::new(),
            last_timer_values:
                std::collections::HashMap::new(),
            tmux,
            worktree,
        }
    }

    /// Re-merge extension config for the currently selected
    /// project. Call this whenever the selected project changes.
    pub fn reload_extension_config(&mut self) {
        let global_ext = load_global_extension_config();
        self.active_extension = match &self.selection {
            Selection::Project(pi)
            | Selection::Feature(pi, _)
            | Selection::Session(pi, _, _) => {
                if let Some(project) =
                    self.store.projects.get(*pi)
                {
                    merge_project_extension_config(
                        &global_ext,
                        &project.repo,
                    )
                } else {
                    global_ext
                }
            }
        };
    }

    /// Open the custom session picker for the currently
    pub fn open_session_picker(&mut self) -> Result<()> {
        use crate::app::BuiltinSessionOption;
        use crate::app::SessionPickerState;

        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => {
                self.message = Some("Select a feature first".into());
                return Ok(());
            }
        };

        if self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .is_none()
        {
            return Ok(());
        }

        let feature = self.store.projects[pi].features[fi].clone();
        let agent = feature.agent.clone();

        let builtin_sessions = vec![
            BuiltinSessionOption {
                kind: SessionKind::Claude,
                label: match agent {
                    AgentKind::Claude => "Claude".to_string(),
                    AgentKind::Opencode => "Opencode (Claude)".to_string(),
                },
            },
            BuiltinSessionOption {
                kind: SessionKind::Terminal,
                label: "Terminal".to_string(),
            },
            BuiltinSessionOption {
                kind: SessionKind::Nvim,
                label: "Neovim".to_string(),
            },
        ];

        let custom_sessions =
            self.active_extension.custom_sessions.clone();

        let total_sessions = builtin_sessions.len() + custom_sessions.len();
        if total_sessions == 0 {
            self.message =
                Some("No sessions available".into());
            return Ok(());
        }

        let from_view = if let AppMode::Viewing(ref view) = self.mode {
            Some((*view).clone())
        } else {
            None
        };

        self.mode = AppMode::SessionPicker(SessionPickerState {
            builtin_sessions,
            custom_sessions,
            selected: 0,
            pi,
            fi,
            from_view,
        });
        Ok(())
    }

    /// Add a custom session type as a tracked FeatureSession.
    /// If the feature's tmux session is already running, also
    /// creates the window and sends the command immediately.
    pub fn add_custom_session_type(
        &mut self,
        pi: usize,
        fi: usize,
        config: &crate::extension::CustomSessionConfig,
    ) -> Result<()> {
        let window_hint = config
            .window_name
            .clone()
            .unwrap_or_else(|| slugify(&config.name));

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        let tmux_session = feature.tmux_session.clone();
        let workdir = config
            .working_dir
            .as_ref()
            .map(|rel| feature.workdir.join(rel))
            .unwrap_or_else(|| feature.workdir.clone());

        let session = feature.add_custom_session_named(
            config.name.clone(),
            window_hint,
            config.command.clone(),
        );
        let window = session.tmux_window.clone();
        let command = session.command.clone();

        if TmuxManager::session_exists(&tmux_session) {
            TmuxManager::create_window(
                &tmux_session,
                &window,
                &workdir,
            )?;
            if let Some(ref cmd) = command {
                TmuxManager::send_literal(
                    &tmux_session,
                    &window,
                    cmd,
                )?;
                TmuxManager::send_key_name(
                    &tmux_session,
                    &window,
                    "Enter",
                )?;
            }
        }

        self.save()?;
        Ok(())
    }

    pub fn add_builtin_session(
        &mut self,
        pi: usize,
        fi: usize,
        kind: SessionKind,
    ) -> Result<()> {
        match kind {
            SessionKind::Terminal => {
                self.add_terminal_session_for_picker(pi, fi)
            }
            SessionKind::Nvim => {
                self.add_nvim_session_for_picker(pi, fi)
            }
            SessionKind::Claude => {
                self.add_claude_session_for_picker(pi, fi)
            }
            _ => {
                self.message =
                    Some("Unsupported session type".into());
                Ok(())
            }
        }
    }

    fn add_terminal_session_for_picker(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session) {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session = feature.add_session(SessionKind::Terminal);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    fn add_nvim_session_for_picker(&mut self, pi: usize, fi: usize) -> Result<()> {
        if std::process::Command::new("nvim")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            self.message = Some(
                "Error: nvim is not installed".into(),
            );
            return Ok(());
        }

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session) {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session = feature.add_session(SessionKind::Nvim);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    fn add_claude_session_for_picker(&mut self, pi: usize, fi: usize) -> Result<()> {
        let repo = self.store.projects[pi].repo.clone();

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session) {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let mode = feature.mode.clone();
        let extra_args: Vec<String> =
            feature.mode.cli_flags(feature.enable_chrome);
        let agent = feature.agent.clone();
        ensure_notification_hooks(
            &workdir,
            &repo,
            &mode,
            &agent,
        );
        ensure_review_claude_md(&workdir, feature.review);
        let session_kind = match feature.agent {
            AgentKind::Claude => SessionKind::Claude,
            AgentKind::Opencode => SessionKind::Opencode,
        };
        let session = feature.add_session(session_kind);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;
        let extra_refs: Vec<&str> =
            extra_args.iter().map(|s| s.as_str()).collect();
        match feature.agent {
            AgentKind::Claude => {
                TmuxManager::launch_claude(
                    &tmux_session,
                    &window,
                    None,
                    &extra_refs,
                )?;
            }
            AgentKind::Opencode => {
                TmuxManager::launch_opencode(
                    &tmux_session,
                    &window,
                )?;
            }
        }

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    /// Run a lifecycle hook script non-blocking.
    /// Expands leading `~/` to the home directory.
    /// If `choice` is provided it is set as `AMF_HOOK_CHOICE`
    /// in the child environment.
    pub fn run_lifecycle_hook(
        &self,
        script: &str,
        workdir: &Path,
        choice: Option<&str>,
    ) {
        let expanded = if script.starts_with("~/") {
            dirs::home_dir()
                .map(|h| {
                    format!(
                        "{}/{}",
                        h.display(),
                        &script[2..]
                    )
                })
                .unwrap_or_else(|| script.to_string())
        } else {
            script.to_string()
        };

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c").arg(&expanded).current_dir(workdir);
        if let Some(c) = choice {
            cmd.env("AMF_HOOK_CHOICE", c);
        }
        let _ = cmd.spawn();
    }

    /// Enter `HookPrompt` mode when the hook config has a
    /// `prompt` field. Does nothing (returns `false`) for
    /// plain `Script` configs so the caller can fall through
    /// to immediate execution.
    pub fn start_hook_prompt(
        &mut self,
        script: String,
        workdir: PathBuf,
        title: String,
        options: Vec<String>,
        next: HookNext,
    ) {
        self.mode = AppMode::HookPrompt(HookPromptState {
            script,
            workdir,
            title,
            options,
            selected: 0,
            next,
        });
    }

    /// Called when the user presses Enter in `HookPrompt` mode.
    pub fn confirm_hook_prompt(&mut self) -> Result<()> {
        let state = match std::mem::replace(
            &mut self.mode,
            AppMode::Normal,
        ) {
            AppMode::HookPrompt(s) => s,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        let choice = state
            .options
            .get(state.selected)
            .cloned()
            .unwrap_or_default();

        match state.next {
            HookNext::WorktreeCreated {
                project_name,
                branch,
                mode,
                review,
                agent,
                enable_chrome,
                enable_notes,
            } => {
                self.start_worktree_hook(
                    &state.script,
                    state.workdir,
                    project_name,
                    branch,
                    mode,
                    review,
                    agent,
                    enable_chrome,
                    enable_notes,
                    Some(choice),
                );
            }
            HookNext::StartFeature { pi, fi } => {
                self.run_lifecycle_hook(
                    &state.script,
                    &state.workdir,
                    Some(&choice),
                );
                self.do_start_feature(pi, fi)?;
            }
            HookNext::StopFeature { pi, fi } => {
                self.run_lifecycle_hook(
                    &state.script,
                    &state.workdir,
                    Some(&choice),
                );
                self.do_stop_feature(pi, fi)?;
            }
        }
        Ok(())
    }

    pub fn start_worktree_hook(
        &mut self,
        script: &str,
        workdir: PathBuf,
        project_name: String,
        branch: String,
        mode: VibeMode,
        review: bool,
        agent: AgentKind,
        enable_chrome: bool,
        enable_notes: bool,
        choice: Option<String>,
    ) {
        let expanded = if script.starts_with("~/") {
            dirs::home_dir()
                .map(|h| {
                    format!(
                        "{}/{}",
                        h.display(),
                        &script[2..]
                    )
                })
                .unwrap_or_else(|| script.to_string())
        } else {
            script.to_string()
        };

        let mut cmd = std::process::Command::new("sh");
        cmd.arg("-c")
            .arg(&expanded)
            .current_dir(&workdir)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        if let Some(ref c) = choice {
            cmd.env("AMF_HOOK_CHOICE", c);
        }
        let (tx, rx) =
            std::sync::mpsc::channel::<String>();
        let mut child = cmd.spawn().ok();

        if let Some(ref mut c) = child {
            if let Some(stdout) = c.stdout.take() {
                let tx2 = tx.clone();
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    for line in
                        std::io::BufReader::new(stdout).lines()
                    {
                        if let Ok(l) = line {
                            let _ = tx2.send(l);
                        }
                    }
                });
            }
            if let Some(stderr) = c.stderr.take() {
                std::thread::spawn(move || {
                    use std::io::BufRead;
                    for line in
                        std::io::BufReader::new(stderr).lines()
                    {
                        if let Ok(l) = line {
                            let _ = tx.send(l);
                        }
                    }
                });
            }
        }

        self.mode = AppMode::RunningHook(RunningHookState {
            script: script.to_string(),
            workdir,
            project_name,
            branch,
            mode,
            review,
            agent,
            enable_chrome,
            enable_notes,
            child,
            output: String::new(),
            success: None,
            output_rx: Some(rx),
        });
    }

    pub fn poll_running_hook(&mut self) -> Result<()> {
        let state = match &mut self.mode {
            AppMode::RunningHook(s) => s,
            _ => return Ok(()),
        };

        // Drain any lines the reader threads have sent.
        if let Some(ref rx) = state.output_rx {
            while let Ok(line) = rx.try_recv() {
                state.output.push_str(&line);
                state.output.push('\n');
            }
        }

        if let Some(ref mut child) = state.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    state.success = Some(status.success());
                    if let Some(code) = status.code() {
                        state.output.push_str(&format!(
                            "\nProcess exited with code: {}",
                            code
                        ));
                    }
                    state.child = None;
                }
                Ok(None) => {}
                Err(e) => {
                    state.success = Some(false);
                    state
                        .output
                        .push_str(&format!("\nError: {}", e));
                    state.child = None;
                }
            }
        }

        Ok(())
    }

    pub fn complete_running_hook(&mut self) -> Result<()> {
        let (workdir, project_name, branch, mode, review, agent, enable_chrome, enable_notes, success) = {
            match &self.mode {
                AppMode::RunningHook(s) => (
                    s.workdir.clone(),
                    s.project_name.clone(),
                    s.branch.clone(),
                    s.mode.clone(),
                    s.review,
                    s.agent.clone(),
                    s.enable_chrome,
                    s.enable_notes,
                    s.success,
                ),
                _ => return Ok(()),
            }
        };

        let is_worktree = workdir != self.store.find_project(&project_name)
            .map(|p| p.repo.clone())
            .unwrap_or_default();

        if enable_notes {
            let claude_dir = workdir.join(".claude");
            if !claude_dir.exists() {
                let _ = std::fs::create_dir_all(&claude_dir);
            }
            let notes_path = claude_dir.join("notes.md");
            if !notes_path.exists() {
                let _ = std::fs::write(
                    &notes_path,
                    "# Notes\n\nWrite instructions for Claude here.\n",
                );
            }
        }

        let feature = Feature::new(
            branch.clone(),
            branch.clone(),
            workdir.clone(),
            is_worktree,
            mode,
            review,
            agent,
            enable_chrome,
            enable_notes,
        );

        self.store.add_feature(&project_name, feature);
        self.save()?;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            let fi = self.store.projects[pi]
                .features
                .len()
                .saturating_sub(1);
            self.store.projects[pi].collapsed = false;
            self.selection = Selection::Feature(pi, fi);
        }

        self.mode = AppMode::Normal;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            let fi = self.store.projects[pi]
                .features
                .len()
                .saturating_sub(1);
            self.ensure_feature_running(pi, fi)?;
            self.save()?;
        }

        if success.unwrap_or(false) {
            self.message = Some(format!(
                "Created and started feature '{}' (hook succeeded)",
                branch
            ));
        } else {
            self.message = Some(format!(
                "Created and started feature '{}' (hook failed)",
                branch
            ));
        }

        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        self.store.save(&self.store_path)
    }

    pub fn visible_items(&self) -> Vec<VisibleItem> {
        let mut items = Vec::new();
        for (pi, project) in
            self.store.projects.iter().enumerate()
        {
            items.push(VisibleItem::Project(pi));
            if !project.collapsed {
                let mut feature_indices: Vec<usize> =
                    (0..project.features.len()).collect();
                feature_indices.sort_by(|&a, &b| {
                    project.features[b]
                        .created_at
                        .cmp(&project.features[a].created_at)
                });
                for fi in feature_indices {
                    let feature = &project.features[fi];
                    items.push(VisibleItem::Feature(pi, fi));
                    if !feature.collapsed {
                        for (si, session) in
                            feature.sessions.iter().enumerate()
                        {
                            if self.session_matches_filter(session) {
                                items.push(VisibleItem::Session(
                                    pi, fi, si,
                                ));
                            }
                        }
                    }
                }
            }
        }
        items
    }

    fn session_matches_filter(&self, session: &FeatureSession) -> bool {
        use crate::project::SessionKind;
        match &self.session_filter {
            SessionFilter::All => true,
            SessionFilter::Claude => session.kind == SessionKind::Claude,
            SessionFilter::Opencode => session.kind == SessionKind::Opencode,
            SessionFilter::Terminal => session.kind == SessionKind::Terminal,
            SessionFilter::Nvim => {
                session.kind == SessionKind::Nvim && session.label != "Memo"
            }
            SessionFilter::Memo => {
                session.kind == SessionKind::Nvim && session.label == "Memo"
            }
        }
    }

    fn selection_index(&self) -> Option<usize> {
        let items = self.visible_items();
        items.iter().position(|item| match (&self.selection, item)
        {
            (
                Selection::Project(a),
                VisibleItem::Project(b),
            ) => a == b,
            (
                Selection::Feature(a1, a2),
                VisibleItem::Feature(b1, b2),
            ) => a1 == b1 && a2 == b2,
            (
                Selection::Session(a1, a2, a3),
                VisibleItem::Session(b1, b2, b3),
            ) => a1 == b1 && a2 == b2 && a3 == b3,
            _ => false,
        })
    }

    pub fn select_next(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        let next = (current + 1) % items.len();
        self.selection = match items[next] {
            VisibleItem::Project(pi) => Selection::Project(pi),
            VisibleItem::Feature(pi, fi) => {
                Selection::Feature(pi, fi)
            }
            VisibleItem::Session(pi, fi, si) => {
                Selection::Session(pi, fi, si)
            }
        };
    }

    pub fn select_prev(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        let prev = if current == 0 {
            items.len() - 1
        } else {
            current - 1
        };
        self.selection = match items[prev] {
            VisibleItem::Project(pi) => Selection::Project(pi),
            VisibleItem::Feature(pi, fi) => {
                Selection::Feature(pi, fi)
            }
            VisibleItem::Session(pi, fi, si) => {
                Selection::Session(pi, fi, si)
            }
        };
    }

    pub fn ensure_selection_visible(&mut self, visible_height: usize) {
        let items = self.visible_items();
        if items.is_empty() || visible_height == 0 {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        if current < self.scroll_offset {
            self.scroll_offset = current;
        } else if current >= self.scroll_offset + visible_height {
            self.scroll_offset = current - visible_height + 1;
        }
    }

    pub fn select_next_feature(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        for offset in 1..=items.len() {
            let idx = (current + offset) % items.len();
            if matches!(items[idx], VisibleItem::Feature(..)) {
                self.selection = match items[idx] {
                    VisibleItem::Feature(pi, fi) => {
                        Selection::Feature(pi, fi)
                    }
                    _ => unreachable!(),
                };
                return;
            }
        }
    }

    pub fn select_prev_feature(&mut self) {
        let items = self.visible_items();
        if items.is_empty() {
            return;
        }
        let current = self.selection_index().unwrap_or(0);
        for offset in 1..=items.len() {
            let idx = if current >= offset {
                current - offset
            } else {
                items.len() - (offset - current)
            };
            if matches!(items[idx], VisibleItem::Feature(..)) {
                self.selection = match items[idx] {
                    VisibleItem::Feature(pi, fi) => {
                        Selection::Feature(pi, fi)
                    }
                    _ => unreachable!(),
                };
                return;
            }
        }
    }

    pub fn sync_statuses(&mut self) {
        let live_sessions =
            self.tmux.list_sessions().unwrap_or_default();
        for project in &mut self.store.projects {
            for feature in &mut project.features {
                if live_sessions
                    .contains(&feature.tmux_session)
                {
                    if feature.status == ProjectStatus::Stopped
                    {
                        feature.status = ProjectStatus::Idle;
                    }
                } else {
                    feature.status = ProjectStatus::Stopped;
                }
            }
        }
    }

    pub fn sync_thinking_status(&mut self) {
        use regex::Regex;
        let timer_re = Regex::new(r"\((\d+m\s+)?\d+s\)").unwrap();

        self.thinking_features.clear();
        for project in &self.store.projects {
            for feature in &project.features {
                if feature.status == ProjectStatus::Stopped {
                    continue;
                }
                let thinking = match feature.agent {
                    AgentKind::Claude => {
                        let session = feature.sessions.iter().find(
                            |s| s.kind == SessionKind::Claude,
                        );
                        let timer_changed = session
                            .and_then(|s| {
                                TmuxManager::capture_pane(
                                    &feature.tmux_session,
                                    &s.tmux_window,
                                )
                                .ok()
                            })
                            .and_then(|content| {
                                timer_re.find(&content).map(|m| {
                                    let current = m.as_str().to_string();
                                    let prev = self
                                        .last_timer_values
                                        .get(&feature.tmux_session)
                                        .cloned();
                                    self.last_timer_values.insert(
                                        feature.tmux_session.clone(),
                                        current.clone(),
                                    );
                                    prev.map(|p| p != current).unwrap_or(false)
                                })
                            })
                            .unwrap_or(false);
                        timer_changed || Self::is_claude_thinking(&feature.tmux_session)
                    }
                    AgentKind::Opencode => {
                        let session = feature.sessions.iter().find(
                            |s| s.kind == SessionKind::Opencode,
                        );
                        session
                            .and_then(|s| {
                                TmuxManager::capture_pane(
                                    &feature.tmux_session,
                                    &s.tmux_window,
                                )
                                .ok()
                            })
                            .map(|content| {
                                let lower = content.to_lowercase();
                                lower.contains("esc interrupt")
                            })
                            .unwrap_or(false)
                    }
                };
                if thinking {
                    self.thinking_features
                        .insert(feature.tmux_session.clone());
                }
            }
        }
    }

    fn is_claude_thinking(tmux_session: &str) -> bool {
        std::path::Path::new(&format!(
            "/tmp/amf-thinking/{}",
            tmux_session
        ))
        .exists()
    }

    pub fn is_feature_thinking(&self, tmux_session: &str) -> bool {
        self.thinking_features.contains(tmux_session)
    }

    pub fn is_feature_waiting_for_input(&self, feature_name: &str) -> bool {
        self.pending_inputs.iter().any(|input| {
            input.feature_name.as_deref() == Some(feature_name)
        })
    }

    pub fn selected_project(&self) -> Option<&Project> {
        match &self.selection {
            Selection::Project(pi)
            | Selection::Feature(pi, _)
            | Selection::Session(pi, _, _) => {
                self.store.projects.get(*pi)
            }
        }
    }

    pub fn selected_feature(
        &self,
    ) -> Option<(&Project, &Feature)> {
        match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => {
                let project = self.store.projects.get(*pi)?;
                let feature = project.features.get(*fi)?;
                Some((project, feature))
            }
            _ => None,
        }
    }

    pub fn selected_session(
        &self,
    ) -> Option<(&Project, &Feature, &FeatureSession)> {
        match &self.selection {
            Selection::Session(pi, fi, si) => {
                let project = self.store.projects.get(*pi)?;
                let feature = project.features.get(*fi)?;
                let session = feature.sessions.get(*si)?;
                Some((project, feature, session))
            }
            _ => None,
        }
    }

    pub fn toggle_collapse(&mut self) {
        match &self.selection {
            Selection::Project(pi) => {
                let pi = *pi;
                if let Some(project) =
                    self.store.projects.get_mut(pi)
                {
                    project.collapsed = !project.collapsed;
                }
            }
            Selection::Feature(pi, fi) => {
                let pi = *pi;
                let fi = *fi;
                if let Some(feature) = self
                    .store
                    .projects
                    .get_mut(pi)
                    .and_then(|p| p.features.get_mut(fi))
                {
                    feature.collapsed = !feature.collapsed;
                }
            }
            Selection::Session(pi, fi, _) => {
                let pi = *pi;
                let fi = *fi;
                if let Some(feature) = self
                    .store
                    .projects
                    .get_mut(pi)
                    .and_then(|p| p.features.get_mut(fi))
                {
                    feature.collapsed = true;
                }
                self.selection = Selection::Feature(pi, fi);
            }
        }
    }

    pub fn start_create_project(&mut self) {
        self.mode = AppMode::CreatingProject(
            CreateProjectState::auto_detect(),
        );
        self.message = None;
    }

    pub fn open_settings_project(&mut self) -> Result<()> {
        let settings_dir = crate::project::amf_config_dir();

        if !settings_dir.exists() {
            std::fs::create_dir_all(&settings_dir)?;
        }

        if let Some((pi, _)) = self.store.projects.iter().enumerate().find(|(_, p)| p.repo == settings_dir) {
            self.selection = Selection::Project(pi);
            self.store.projects[pi].collapsed = false;
            self.message = Some("Opened AMF settings project".into());
            return Ok(());
        }

        let project = Project::new("amf-settings".into(), settings_dir.clone(), false);
        self.store.add_project(project);
        self.save()?;

        let pi = self.store.projects.len().saturating_sub(1);
        self.selection = Selection::Project(pi);
        self.message = Some("Created AMF settings project".into());

        Ok(())
    }

    pub fn cancel_create(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn show_error(&mut self, error: anyhow::Error) {
        self.message = Some(format!("Error: {}", error));
        match &self.mode {
            AppMode::Normal
            | AppMode::Help
            | AppMode::Viewing(_) => {}
            _ => {
                self.mode = AppMode::Normal;
            }
        }
    }

    pub fn start_browse_path(
        &mut self,
        create_state: CreateProjectState,
    ) {
        let mut explorer = match FileExplorer::new() {
            Ok(e) => e,
            Err(_) => {
                self.message =
                    Some("Failed to open file browser".into());
                return;
            }
        };

        let start_dir = PathBuf::from(&create_state.path);
        let start_dir = if start_dir.is_dir() {
            start_dir
        } else {
            dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
        };

        let _ = explorer.set_cwd(start_dir);

        self.mode = AppMode::BrowsingPath(Box::new(
            BrowsePathState {
                explorer,
                create_state,
                new_folder_name: String::new(),
                creating_folder: false,
            },
        ));
        self.message = None;
    }

    pub fn confirm_browse_path(&mut self) {
        let path = match &self.mode {
            AppMode::BrowsingPath(state) => {
                state.explorer.cwd().to_string_lossy().into_owned()
            }
            _ => return,
        };

        let browse = std::mem::replace(
            &mut self.mode,
            AppMode::Normal,
        );
        if let AppMode::BrowsingPath(mut state) = browse {
            state.create_state.path = path;
            state.create_state.step = CreateProjectStep::Path;
            self.mode =
                AppMode::CreatingProject(state.create_state);
        }
    }

    pub fn cancel_browse_path(&mut self) {
        let browse = std::mem::replace(
            &mut self.mode,
            AppMode::Normal,
        );
        if let AppMode::BrowsingPath(state) = browse {
            self.mode =
                AppMode::CreatingProject(state.create_state);
        }
    }

    pub fn create_folder_in_browse(&mut self) -> Result<()> {
        let (cwd, folder_name) = match &self.mode {
            AppMode::BrowsingPath(state) => {
                (state.explorer.cwd().to_path_buf(), state.new_folder_name.clone())
            }
            _ => return Ok(()),
        };

        if folder_name.is_empty() {
            self.message = Some("Folder name cannot be empty".into());
            return Ok(());
        }

        let new_path = cwd.join(&folder_name);
        if let Err(e) = std::fs::create_dir_all(&new_path) {
            self.message = Some(format!(
                "Error: Failed to create folder: {}",
                e
            ));
            return Ok(());
        }

        if let AppMode::BrowsingPath(state) = &mut self.mode {
            state.creating_folder = false;
            state.new_folder_name.clear();
            let _ = state.explorer.set_cwd(new_path);
            state.create_state.path = state.explorer.cwd().to_string_lossy().into_owned();
        }

        Ok(())
    }

    pub fn create_project(&mut self) -> Result<()> {
        let state = match &self.mode {
            AppMode::CreatingProject(s) => s.clone(),
            _ => return Ok(()),
        };

        let name = state.name.clone();
        let path = PathBuf::from(&state.path);

        if name.is_empty() {
            self.message =
                Some("Error: Project name cannot be empty".into());
            return Ok(());
        }

        if !path.exists() {
            self.message = Some(format!(
                "Error: Path does not exist: {} (press Ctrl+B to browse and create folder)",
                path.display()
            ));
            return Ok(());
        }

        if self.store.find_project(&name).is_some() {
            self.message = Some(format!(
                "Error: Project '{}' already exists",
                name
            ));
            return Ok(());
        }

        let (project_path, is_git) =
            match WorktreeManager::repo_root(&path) {
                Ok(r) => (r, true),
                Err(_) => (path.clone(), false),
            };
        let project =
            Project::new(name.clone(), project_path, is_git);

        self.store.add_project(project);
        self.save()?;

        let pi =
            self.store.projects.len().saturating_sub(1);
        self.selection = Selection::Project(pi);
        self.mode = AppMode::Normal;
        self.message =
            Some(format!("Created project '{}'", name));

        Ok(())
    }

    pub fn delete_project(&mut self) -> Result<()> {
        let project_name = match &self.mode {
            AppMode::DeletingProject(name) => name.clone(),
            _ => return Ok(()),
        };

        if let Some(project) =
            self.store.find_project(&project_name)
        {
            let features: Vec<(String, PathBuf, bool)> = project
                .features
                .iter()
                .map(|f| {
                    (
                        f.tmux_session.clone(),
                        f.workdir.clone(),
                        f.is_worktree,
                    )
                })
                .collect();
            let repo = project.repo.clone();

            for (session, workdir, is_worktree) in features {
                let _ = TmuxManager::kill_session(&session);
                if is_worktree {
                    let _ =
                        WorktreeManager::remove(&repo, &workdir);
                }
            }
        }

        self.store.remove_project(&project_name);
        self.save()?;

        let items = self.visible_items();
        if items.is_empty() {
            self.selection = Selection::Project(0);
        } else {
            let idx = self.selection_index().unwrap_or(0);
            if idx >= items.len() {
                let last = &items[items.len() - 1];
                self.selection = match last {
                    VisibleItem::Project(pi) => {
                        Selection::Project(*pi)
                    }
                    VisibleItem::Feature(pi, fi) => {
                        Selection::Feature(*pi, *fi)
                    }
                    VisibleItem::Session(pi, fi, si) => {
                        Selection::Session(*pi, *fi, *si)
                    }
                };
            }
        }

        self.mode = AppMode::Normal;
        self.message = Some(format!(
            "Deleted project '{}'",
            project_name
        ));
        Ok(())
    }

    pub fn start_create_feature(&mut self) {
        let (project_name, project_repo, is_first, used_workdirs) =
            match &self.selection {
                Selection::Project(pi)
                | Selection::Feature(pi, _)
                | Selection::Session(pi, _, _) => {
                    if let Some(p) =
                        self.store.projects.get(*pi)
                    {
                        let used: Vec<PathBuf> = p
                            .features
                            .iter()
                            .map(|f| f.workdir.clone())
                            .collect();
                        (
                            p.name.clone(),
                            p.repo.clone(),
                            p.features.is_empty(),
                            used,
                        )
                    } else {
                        return;
                    }
                }
            };

        let worktrees = WorktreeManager::list(&project_repo)
            .unwrap_or_default()
            .into_iter()
            .filter(|wt| {
                wt.path != project_repo
                    && !used_workdirs.contains(&wt.path)
            })
            .collect();

        self.mode = AppMode::CreatingFeature(
            CreateFeatureState::new(
                project_name,
                project_repo,
                worktrees,
                is_first,
            ),
        );
        self.message = None;
    }

    pub fn create_feature(&mut self) -> Result<()> {
        let state = match &self.mode {
            AppMode::CreatingFeature(s) => s,
            _ => return Ok(()),
        };

        let project_name = state.project_name.clone();
        let project_repo = state.project_repo.clone();
        let branch = state.branch.clone();
        let mode = state.mode.clone();
        let review = state.review;
        let use_existing_worktree = state.source_index == 1
            && !state.worktrees.is_empty();
        let selected_worktree = if use_existing_worktree {
            state.worktrees.get(state.worktree_index).cloned()
        } else {
            None
        };
        let use_worktree = state.use_worktree;
        let enable_chrome = state.enable_chrome;
        let enable_notes = state.enable_notes;

        if branch.is_empty() {
            self.message =
                Some("Error: Branch name cannot be empty".into());
            return Ok(());
        }

        let stored_is_git = {
            let project =
                match self.store.find_project(&project_name) {
                    Some(p) => p,
                    None => {
                        self.message = Some(format!(
                            "Error: Project '{}' not found",
                            project_name
                        ));
                        return Ok(());
                    }
                };

            if project
                .features
                .iter()
                .any(|f| f.name == branch)
            {
                self.message = Some(format!(
                    "Error: Feature '{}' already exists in '{}'",
                    branch, project_name
                ));
                return Ok(());
            }

            if !use_worktree
                && selected_worktree.is_none()
                && project
                    .features
                    .iter()
                    .any(|f| !f.is_worktree)
            {
                self.message = Some(
                    "Error: Only one non-worktree feature \
                     allowed per project"
                        .into(),
                );
                return Ok(());
            }

            project.is_git
        };

        let is_git = stored_is_git
            || self.worktree.repo_root(&project_repo).is_ok();

        if is_git && !stored_is_git {
            if let Some(p) =
                self.store.find_project_mut(&project_name)
            {
                p.is_git = true;
            }
            self.save()?;
        }

        if (use_worktree || selected_worktree.is_some())
            && !is_git
        {
            self.message = Some(
                "Error: Worktrees require a git repository"
                    .into(),
            );
            return Ok(());
        }

        let (workdir, is_worktree) =
            if let Some(wt) = &selected_worktree {
                (wt.path.clone(), true)
            } else if use_worktree {
                let wt_path = self.worktree.create(
                    &project_repo,
                    &branch,
                    &branch,
                )?;

                let global_ext = load_global_extension_config();
                let ext = merge_project_extension_config(&global_ext, &project_repo);

                if let Some(ref hook_cfg) = ext.lifecycle_hooks.on_worktree_created {
                    if let Some(prompt) = hook_cfg.prompt() {
                        self.start_hook_prompt(
                            hook_cfg.script().to_string(),
                            wt_path.clone(),
                            prompt.title.clone(),
                            prompt.options.clone(),
                            HookNext::WorktreeCreated {
                                project_name,
                                branch,
                                mode,
                                review,
                                agent: state.agent.clone(),
                                enable_chrome,
                                enable_notes,
                            },
                        );
                    } else {
                        self.start_worktree_hook(
                            hook_cfg.script(),
                            wt_path.clone(),
                            project_name,
                            branch,
                            mode,
                            review,
                            state.agent.clone(),
                            enable_chrome,
                            enable_notes,
                            None,
                        );
                    }
                    return Ok(());
                }

                (wt_path, true)
            } else {
                (project_repo.clone(), false)
            };

        if enable_notes {
            let claude_dir = workdir.join(".claude");
            if !claude_dir.exists() {
                let _ = std::fs::create_dir_all(&claude_dir);
            }
            let notes_path = claude_dir.join("notes.md");
            if !notes_path.exists() {
                let _ = std::fs::write(
                    &notes_path,
                    "# Notes\n\nWrite instructions for Claude here.\n",
                );
            }
        }

        let feature = Feature::new(
            branch.clone(),
            branch.clone(),
            workdir,
            is_worktree,
            mode,
            review,
            state.agent.clone(),
            enable_chrome,
            enable_notes,
        );

        self.store.add_feature(&project_name, feature);
        self.save()?;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            let fi = self.store.projects[pi]
                .features
                .len()
                .saturating_sub(1);
            self.store.projects[pi].collapsed = false;
            self.selection = Selection::Feature(pi, fi);
        }

        self.mode = AppMode::Normal;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            let fi = self.store.projects[pi]
                .features
                .len()
                .saturating_sub(1);
            self.ensure_feature_running(pi, fi)?;
            self.save()?;
        }

        self.message = Some(format!(
            "Created and started feature '{}'",
            branch
        ));

        Ok(())
    }

    fn ensure_feature_running(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        let repo =
            self.store.projects[pi].repo.clone();
        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        ensure_notification_hooks(
            &feature.workdir,
            &repo,
            &feature.mode,
            &feature.agent,
        );
        ensure_review_claude_md(&feature.workdir, feature.review);

        if feature.sessions.is_empty() {
            let session_kind = match feature.agent {
                AgentKind::Claude => SessionKind::Claude,
                AgentKind::Opencode => SessionKind::Opencode,
            };
            feature.add_session(session_kind);
            feature.add_session(SessionKind::Terminal);
            if feature.has_notes {
                let s = feature.add_session(SessionKind::Nvim);
                s.label = "Memo".into();
            }
        }

        if self.tmux.session_exists(&feature.tmux_session) {
            return Ok(());
        }

        self.tmux.create_session_with_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
            &feature.workdir,
        )?;
        self.tmux.set_session_env(
            &feature.tmux_session,
            "AMF_SESSION",
            &feature.tmux_session,
        )?;

        for session in &feature.sessions[1..] {
            self.tmux.create_window(
                &feature.tmux_session,
                &session.tmux_window,
                &feature.workdir,
            )?;
        }

        let extra_args: Vec<String> =
            feature.mode.cli_flags(feature.enable_chrome);
        for session in &feature.sessions {
            match session.kind {
                SessionKind::Claude => {
                    self.tmux.launch_claude(
                        &feature.tmux_session,
                        &session.tmux_window,
                        session.claude_session_id.clone(),
                        extra_args.clone(),
                    )?;
                }
                SessionKind::Opencode => {
                    self.tmux.launch_opencode(
                        &feature.tmux_session,
                        &session.tmux_window,
                    )?;
                }
                SessionKind::Nvim => {
                    if feature.has_notes {
                        self.tmux.send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim .claude/notes.md",
                        )?;
                    } else {
                        self.tmux.send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim",
                        )?;
                    }
                }
                SessionKind::Terminal => {}
                SessionKind::Custom => {
                    if let Some(ref cmd) = session.command {
                        self.tmux.send_literal(
                            &feature.tmux_session,
                            &session.tmux_window,
                            cmd,
                        )?;
                        self.tmux.send_key_name(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "Enter",
                        )?;
                    }
                }
            }
        }

        self.tmux.select_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
        )?;

        feature.status = ProjectStatus::Idle;
        feature.touch();

        Ok(())
    }

    pub fn start_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let status = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.status.clone());

        if status != Some(ProjectStatus::Stopped) {
            if let Some(name) = self
                .store
                .projects
                .get(pi)
                .and_then(|p| p.features.get(fi))
                .map(|f| f.name.clone())
            {
                self.message = Some(format!(
                    "Error: '{}' is already running",
                    name
                ));
            }
            return Ok(());
        }

        // If on_start has a prompt, show the picker first.
        let on_start =
            self.active_extension.lifecycle_hooks.on_start.clone();
        if let Some(ref cfg) = on_start {
            if let Some(prompt) = cfg.prompt() {
                let workdir = self
                    .store
                    .projects
                    .get(pi)
                    .and_then(|p| p.features.get(fi))
                    .map(|f| f.workdir.clone())
                    .unwrap_or_default();
                self.start_hook_prompt(
                    cfg.script().to_string(),
                    workdir,
                    prompt.title.clone(),
                    prompt.options.clone(),
                    HookNext::StartFeature { pi, fi },
                );
                return Ok(());
            }
        }

        self.ensure_feature_running(pi, fi)?;

        // Fire on_start lifecycle hook (plain script) if configured.
        if let Some(ref cfg) = on_start {
            let workdir = self
                .store
                .projects
                .get(pi)
                .and_then(|p| p.features.get(fi))
                .map(|f| f.workdir.clone())
                .unwrap_or_default();
            self.run_lifecycle_hook(cfg.script(), &workdir, None);
        }

        let name = self.store.projects[pi].features[fi]
            .name
            .clone();
        self.save()?;
        self.message = Some(format!("Started '{}'", name));

        Ok(())
    }

    /// Inner start logic called after a hook prompt is confirmed.
    pub fn do_start_feature(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        self.ensure_feature_running(pi, fi)?;
        let name = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.name.clone())
            .unwrap_or_default();
        self.save()?;
        self.message = Some(format!("Started '{}'", name));
        Ok(())
    }

    pub fn stop_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if feature.status == ProjectStatus::Stopped {
            self.message = Some(format!(
                "Error: '{}' is already stopped",
                feature.name
            ));
            return Ok(());
        }

        // Fire on_stop lifecycle hook before killing session.
        // Clone hook and workdir data before mutable borrow.
        let on_stop_hook =
            self.active_extension.lifecycle_hooks.on_stop.clone();
        let workdir_for_hook = feature.workdir.clone();

        // If on_stop has a prompt, show the picker first.
        if let Some(ref cfg) = on_stop_hook {
            if let Some(prompt) = cfg.prompt() {
                self.start_hook_prompt(
                    cfg.script().to_string(),
                    workdir_for_hook,
                    prompt.title.clone(),
                    prompt.options.clone(),
                    HookNext::StopFeature { pi, fi },
                );
                return Ok(());
            }
        }

        if let Some(ref cfg) = on_stop_hook {
            self.run_lifecycle_hook(
                cfg.script(),
                &workdir_for_hook,
                None,
            );
        }

        self.do_stop_feature(pi, fi)?;

        Ok(())
    }

    /// Inner stop logic called after a hook prompt is confirmed.
    pub fn do_stop_feature(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        let tmux_session = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
        {
            Some(f) => f.tmux_session.clone(),
            None => return Ok(()),
        };

        self.tmux.kill_session(&tmux_session)?;

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };
        feature.status = ProjectStatus::Stopped;
        let name = feature.name.clone();
        self.save()?;
        self.message = Some(format!("Stopped '{}'", name));

        Ok(())
    }

    pub fn delete_feature(&mut self) -> Result<()> {
        let (project_name, feature_name) = match &self.mode {
            AppMode::DeletingFeature(pn, fn_) => {
                (pn.clone(), fn_.clone())
            }
            _ => return Ok(()),
        };

        let (tmux_session, is_worktree, repo, workdir) =
            if let Some(project) =
                self.store.find_project(&project_name)
                && let Some(feature) = project
                    .features
                    .iter()
                    .find(|f| f.name == feature_name)
            {
                (
                    feature.tmux_session.clone(),
                    feature.is_worktree,
                    project.repo.clone(),
                    feature.workdir.clone(),
                )
            } else {
                return Ok(());
            };

        let child = TmuxManager::spawn_kill_session(&tmux_session)?;

        self.mode = AppMode::DeletingFeatureInProgress(DeletingFeatureState {
            project_name,
            feature_name,
            tmux_session,
            is_worktree,
            repo,
            workdir,
            stage: DeleteStage::KillingTmux,
            child,
            error: None,
        });

        Ok(())
    }

    pub fn poll_deleting_feature(&mut self) -> Result<()> {
        let state = match &mut self.mode {
            AppMode::DeletingFeatureInProgress(s) => s,
            _ => return Ok(()),
        };

        if let Some(ref mut child) = state.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        state.error = Some(format!(
                            "Command failed with code: {:?}",
                            status.code()
                        ));
                    }
                    state.child = None;
                }
                Ok(None) => return Ok(()),
                Err(e) => {
                    state.error = Some(e.to_string());
                    state.child = None;
                }
            }
        }

        match state.stage {
            DeleteStage::KillingTmux => {
                if state.is_worktree {
                    match WorktreeManager::spawn_remove(
                        &state.repo,
                        &state.workdir,
                    ) {
                        Ok(child) => {
                            state.child = Some(child);
                            state.stage = DeleteStage::RemovingWorktree;
                        }
                        Err(e) => {
                            state.error = Some(e.to_string());
                        }
                    }
                } else {
                    state.stage = DeleteStage::Completed;
                }
            }
            DeleteStage::RemovingWorktree => {
                state.stage = DeleteStage::Completed;
            }
            DeleteStage::Completed => {}
        }

        Ok(())
    }

    pub fn complete_deleting_feature(&mut self) -> Result<()> {
        let (project_name, feature_name, had_error, error_msg) = {
            match &self.mode {
                AppMode::DeletingFeatureInProgress(s) => (
                    s.project_name.clone(),
                    s.feature_name.clone(),
                    s.error.is_some(),
                    s.error.clone(),
                ),
                _ => return Ok(()),
            }
        };

        if had_error {
            self.mode = AppMode::Normal;
            self.message = Some(format!(
                "Error deleting feature '{}': {}",
                feature_name,
                error_msg.unwrap_or_else(|| "Unknown error".to_string())
            ));
            return Ok(());
        }

        self.store
            .remove_feature(&project_name, &feature_name);
        self.save()?;

        if let Some(pi) = self
            .store
            .projects
            .iter()
            .position(|p| p.name == project_name)
        {
            self.selection = Selection::Project(pi);
        }

        self.mode = AppMode::Normal;
        self.message = Some(format!(
            "Deleted feature '{}'",
            feature_name
        ));
        Ok(())
    }

    pub fn cancel_deleting_feature(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn add_terminal_session(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session)
        {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session =
            feature.add_session(SessionKind::Terminal);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    pub fn add_nvim_session(&mut self) -> Result<()> {
        if std::process::Command::new("nvim")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_err()
        {
            self.message = Some(
                "Error: nvim is not installed".into(),
            );
            return Ok(());
        }

        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session)
        {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let session =
            feature.add_session(SessionKind::Nvim);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;
        TmuxManager::send_keys(
            &tmux_session,
            &window,
            "nvim",
        )?;

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    pub fn create_memo(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if feature.has_notes {
            self.message =
                Some("Memo already exists".into());
            return Ok(());
        }

        let claude_dir = feature.workdir.join(".claude");
        if !claude_dir.exists() {
            let _ = std::fs::create_dir_all(&claude_dir);
        }
        let notes_path = claude_dir.join("notes.md");
        if !notes_path.exists() {
            let _ = std::fs::write(
                &notes_path,
                "# Notes\n\nWrite instructions for Claude here.\n",
            );
        }

        feature.has_notes = true;

        if TmuxManager::session_exists(
            &feature.tmux_session,
        ) {
            let workdir = feature.workdir.clone();
            let tmux_session =
                feature.tmux_session.clone();
            let session =
                feature.add_session(SessionKind::Nvim);
            session.label = "Memo".into();
            let window = session.tmux_window.clone();

            TmuxManager::create_window(
                &tmux_session,
                &window,
                &workdir,
            )?;
            TmuxManager::send_keys(
                &tmux_session,
                &window,
                "nvim .claude/notes.md",
            )?;

            feature.collapsed = false;
        }

        self.save()?;
        self.message = Some("Created memo".into());

        Ok(())
    }

    pub fn add_claude_session(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let repo =
            self.store.projects[pi].repo.clone();

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        if !TmuxManager::session_exists(&feature.tmux_session)
        {
            self.message = Some(
                "Error: Feature must be running to add a session"
                    .into(),
            );
            return Ok(());
        }

        let workdir = feature.workdir.clone();
        let tmux_session = feature.tmux_session.clone();
        let mode = feature.mode.clone();
        let extra_args: Vec<String> = feature.mode.cli_flags(feature.enable_chrome);
        let agent = feature.agent.clone();
        ensure_notification_hooks(
            &workdir,
            &repo,
            &mode,
            &agent,
        );
        ensure_review_claude_md(&workdir, feature.review);
        let session_kind = match feature.agent {
            AgentKind::Claude => SessionKind::Claude,
            AgentKind::Opencode => SessionKind::Opencode,
        };
        let session = feature.add_session(session_kind);
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        TmuxManager::create_window(
            &tmux_session,
            &window,
            &workdir,
        )?;
        let extra_refs: Vec<&str> =
            extra_args.iter().map(|s| s.as_str()).collect();
        match feature.agent {
            AgentKind::Claude => {
                TmuxManager::launch_claude(
                    &tmux_session,
                    &window,
                    None,
                    &extra_refs,
                )?;
            }
            AgentKind::Opencode => {
                TmuxManager::launch_opencode(
                    &tmux_session,
                    &window,
                )?;
            }
        }

        feature.collapsed = false;
        let si = feature.sessions.len() - 1;
        self.selection = Selection::Session(pi, fi, si);
        self.save()?;
        self.message = Some(format!("Added '{}'", label));

        Ok(())
    }

    pub fn remove_session(&mut self) -> Result<()> {
        let (pi, fi, si) = match &self.selection {
            Selection::Session(pi, fi, si) => {
                (*pi, *fi, *si)
            }
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        let tmux_session = feature.tmux_session.clone();
        let session = match feature.sessions.get(si) {
            Some(s) => s,
            None => return Ok(()),
        };
        let window = session.tmux_window.clone();
        let label = session.label.clone();

        if TmuxManager::session_exists(&tmux_session) {
            let _ = TmuxManager::kill_window(
                &tmux_session,
                &window,
            );
        }

        feature.sessions.remove(si);

        if feature.sessions.is_empty() {
            let _ = TmuxManager::kill_session(&tmux_session);
            feature.status = ProjectStatus::Stopped;
        }

        self.selection = Selection::Feature(pi, fi);
        self.save()?;
        self.message = Some(format!("Removed '{}'", label));

        Ok(())
    }

    pub fn enter_view(&mut self) -> Result<()> {
        let (pi, fi, target_si) = match &self.selection {
            Selection::Session(pi, fi, si) => {
                (*pi, *fi, Some(*si))
            }
            Selection::Feature(pi, fi) => (*pi, *fi, None),
            _ => return Ok(()),
        };

        self.ensure_feature_running(pi, fi)?;

        let (
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
            review,
        ) = {
            let project = &self.store.projects[pi];
            let feature = &project.features[fi];

            let si = target_si.unwrap_or_else(|| {
                feature
                    .sessions
                    .iter()
                    .position(|s| {
                        s.kind == SessionKind::Claude
                    })
                    .unwrap_or(0)
            });

            let session = &feature.sessions[si];
            (
                project.name.clone(),
                feature.name.clone(),
                feature.tmux_session.clone(),
                session.tmux_window.clone(),
                session.label.clone(),
                feature.mode.clone(),
                feature.review,
            )
        };

        let feature = self.store.projects[pi]
            .features
            .get_mut(fi)
            .unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;

        // Clear pending input notifications for this feature
        self.pending_inputs.retain(|input| {
            if input.project_name.as_deref()
                == Some(&project_name)
                && input.feature_name.as_deref()
                    == Some(&feature_name)
                && input.notification_type != "diff-review"
            {
                let _ =
                    std::fs::remove_file(&input.file_path);
                false
            } else {
                true
            }
        });

        let view = ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
            review,
        );

        self.save()?;
        self.pane_content.clear();

        self.mode = AppMode::Viewing(view);

        Ok(())
    }

    pub fn exit_view(&mut self) {
        self.mode = AppMode::Normal;
        self.pane_content.clear();
        self.tmux_cursor = None;
        self.message = Some("Returned to dashboard".into());
    }

    pub fn switch_to_selected(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let window = match &self.selection {
            Selection::Session(_, _, si) => self
                .store
                .projects
                .get(pi)
                .and_then(|p| p.features.get(fi))
                .and_then(|f| f.sessions.get(*si))
                .map(|s| s.tmux_window.clone()),
            _ => None,
        };

        self.ensure_feature_running(pi, fi)?;

        let feature = self.store.projects[pi]
            .features
            .get_mut(fi)
            .unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;
        let session = feature.tmux_session.clone();
        self.save()?;

        if let Some(window) = &window {
            let _ =
                TmuxManager::select_window(&session, window);
        }

        if TmuxManager::is_inside_tmux() {
            TmuxManager::switch_client(&session)?;
            self.message =
                Some("Switched back from project".into());
        } else {
            self.should_switch = Some(session);
        }

        Ok(())
    }

    pub fn activate_leader(&mut self) {
        self.leader_active = true;
        self.leader_activated_at = Some(Instant::now());
    }

    pub fn deactivate_leader(&mut self) {
        self.leader_active = false;
        self.leader_activated_at = None;
    }

    pub fn leader_timed_out(&self) -> bool {
        self.leader_activated_at
            .map(|t| {
                t.elapsed()
                    >= std::time::Duration::from_secs(2)
            })
            .unwrap_or(false)
    }

    pub fn toggle_scroll_mode(&mut self, visible_rows: u16) {
        if let AppMode::Viewing(ref mut view) = self.mode {
            view.scroll_mode = !view.scroll_mode;
            if view.scroll_mode {
                let is_alternate = TmuxManager::is_alternate_screen(&view.session, &view.window);
                view.scroll_passthrough = is_alternate;
                
                if !is_alternate {
                    let (content, lines) = TmuxManager::capture_pane_with_history(
                        &view.session,
                        &view.window,
                        10000,
                    )
                    .unwrap_or((String::new(), 0));
                    view.scroll_content = content;
                    view.scroll_total_lines = lines;
                    let max_offset = lines.saturating_sub(visible_rows as usize);
                    view.scroll_offset = max_offset;
                } else {
                    view.scroll_content.clear();
                    view.scroll_total_lines = 0;
                    view.scroll_offset = 0;
                }
            } else {
                view.scroll_content.clear();
                view.scroll_offset = 0;
            }
        }
    }

    pub fn scroll_up(&mut self, amount: usize) {
        if let AppMode::Viewing(ref mut view) = self.mode
            && view.scroll_mode
            && !view.scroll_passthrough
        {
            view.scroll_offset = view.scroll_offset.saturating_sub(amount);
        }
    }

    pub fn scroll_down(&mut self, amount: usize, visible_rows: u16) {
        if let AppMode::Viewing(ref mut view) = self.mode
            && view.scroll_mode
            && !view.scroll_passthrough
        {
            let max_offset = view.scroll_total_lines.saturating_sub(visible_rows as usize);
            view.scroll_offset = (view.scroll_offset + amount).min(max_offset);
        }
    }

    pub fn scroll_to_top(&mut self) {
        if let AppMode::Viewing(ref mut view) = self.mode
            && view.scroll_mode
            && !view.scroll_passthrough
        {
            view.scroll_offset = 0;
        }
    }

    pub fn scroll_to_bottom(&mut self, visible_rows: u16) {
        if let AppMode::Viewing(ref mut view) = self.mode
            && view.scroll_mode
            && !view.scroll_passthrough
        {
            let max_offset = view.scroll_total_lines.saturating_sub(visible_rows as usize);
            view.scroll_offset = max_offset;
        }
    }

    pub fn view_next_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| {
                        p.name == view.project_name
                    });
                let pi = match pi {
                    Some(pi) => pi,
                    None => return Ok(()),
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| {
                        f.name == view.feature_name
                    });
                let fi = match fi {
                    Some(fi) => fi,
                    None => return Ok(()),
                };
                (pi, fi)
            }
            _ => return Ok(()),
        };

        let project = &self.store.projects[pi];
        let len = project.features.len();
        if len <= 1 {
            return Ok(());
        }

        for offset in 1..len {
            let candidate = (fi + offset) % len;
            if project.features[candidate].status != ProjectStatus::Stopped {
                return self.switch_view_to_feature(pi, candidate);
            }
        }
        Ok(())
    }

    pub fn view_prev_feature(&mut self) -> Result<()> {
        let (pi, fi) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| {
                        p.name == view.project_name
                    });
                let pi = match pi {
                    Some(pi) => pi,
                    None => return Ok(()),
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| {
                        f.name == view.feature_name
                    });
                let fi = match fi {
                    Some(fi) => fi,
                    None => return Ok(()),
                };
                (pi, fi)
            }
            _ => return Ok(()),
        };

        let project = &self.store.projects[pi];
        let len = project.features.len();
        if len <= 1 {
            return Ok(());
        }

        for offset in 1..len {
            let candidate = (fi + len - offset) % len;
            if project.features[candidate].status != ProjectStatus::Stopped {
                return self.switch_view_to_feature(pi, candidate);
            }
        }
        Ok(())
    }

    fn switch_view_to_feature(
        &mut self,
        pi: usize,
        fi: usize,
    ) -> Result<()> {
        self.ensure_feature_running(pi, fi)?;

        let project = &self.store.projects[pi];
        let feature = &project.features[fi];
        let project_name = project.name.clone();
        let feature_name = feature.name.clone();
        let tmux_session = feature.tmux_session.clone();
        let vibe_mode = feature.mode.clone();
        let review = feature.review;

        let si = feature
            .sessions
            .iter()
            .position(|s| s.kind == SessionKind::Claude)
            .unwrap_or(0);
        let (session_window, session_label) =
            if let Some(s) = feature.sessions.get(si) {
                (s.tmux_window.clone(), s.label.clone())
            } else {
                ("claude".into(), "Claude 1".into())
            };

        let feature = self.store.projects[pi]
            .features
            .get_mut(fi)
            .unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;

        self.selection = Selection::Feature(pi, fi);
        self.pane_content.clear();
        self.mode = AppMode::Viewing(ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
            review,
        ));
        self.save()?;

        Ok(())
    }

    pub fn view_next_session(&mut self) {
        let (pi, fi, current_window) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| {
                        p.name == view.project_name
                    });
                let pi = match pi {
                    Some(pi) => pi,
                    None => return,
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| {
                        f.name == view.feature_name
                    });
                let fi = match fi {
                    Some(fi) => fi,
                    None => return,
                };
                (pi, fi, view.window.clone())
            }
            _ => return,
        };

        let feature = &self.store.projects[pi].features[fi];
        if feature.sessions.len() <= 1 {
            return;
        }

        let current_si = feature
            .sessions
            .iter()
            .position(|s| s.tmux_window == current_window)
            .unwrap_or(0);
        let next_si =
            (current_si + 1) % feature.sessions.len();
        let next = &feature.sessions[next_si];

        if let AppMode::Viewing(ref mut view) = self.mode {
            view.window = next.tmux_window.clone();
            view.session_label = next.label.clone();
        }
        self.pane_content.clear();
    }

    pub fn view_prev_session(&mut self) {
        let (pi, fi, current_window) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| {
                        p.name == view.project_name
                    });
                let pi = match pi {
                    Some(pi) => pi,
                    None => return,
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| {
                        f.name == view.feature_name
                    });
                let fi = match fi {
                    Some(fi) => fi,
                    None => return,
                };
                (pi, fi, view.window.clone())
            }
            _ => return,
        };

        let feature = &self.store.projects[pi].features[fi];
        if feature.sessions.len() <= 1 {
            return;
        }

        let current_si = feature
            .sessions
            .iter()
            .position(|s| s.tmux_window == current_window)
            .unwrap_or(0);
        let prev_si = if current_si == 0 {
            feature.sessions.len() - 1
        } else {
            current_si - 1
        };
        let prev = &feature.sessions[prev_si];

        if let AppMode::Viewing(ref mut view) = self.mode {
            view.window = prev.tmux_window.clone();
            view.session_label = prev.label.clone();
        }
        self.pane_content.clear();
    }

    pub fn open_session_switcher(&mut self) {
        let (project_name, feature_name, tmux_session, current_window, current_label, sessions, vibe_mode, review) =
            match &self.mode {
                AppMode::Viewing(view) => {
                    let pi = self
                        .store
                        .projects
                        .iter()
                        .position(|p| {
                            p.name == view.project_name
                        });
                    let pi = match pi {
                        Some(pi) => pi,
                        None => return,
                    };
                    let fi = self.store.projects[pi]
                        .features
                        .iter()
                        .position(|f| {
                            f.name == view.feature_name
                        });
                    let fi = match fi {
                        Some(fi) => fi,
                        None => return,
                    };
                    let feature =
                        &self.store.projects[pi].features[fi];
                    let entries: Vec<SwitcherEntry> = feature
                        .sessions
                        .iter()
                        .map(|s| {
                            let cfg = self
                                .active_extension
                                .custom_sessions
                                .iter()
                                .find(|c| c.name == s.label);
                            SwitcherEntry {
                                tmux_window: s
                                    .tmux_window
                                    .clone(),
                                kind: s.kind.clone(),
                                label: s.label.clone(),
                                icon: cfg.and_then(|c| {
                                    c.icon.clone()
                                }),
                                icon_nerd: cfg.and_then(|c| {
                                    c.icon_nerd.clone()
                                }),
                            }
                        })
                        .collect();
                    (
                        view.project_name.clone(),
                        view.feature_name.clone(),
                        view.session.clone(),
                        view.window.clone(),
                        view.session_label.clone(),
                        entries,
                        view.vibe_mode.clone(),
                        view.review,
                    )
                }
                _ => return,
            };

        if sessions.is_empty() {
            return;
        }

        let selected = sessions
            .iter()
            .position(|s| s.tmux_window == current_window)
            .unwrap_or(0);

        self.mode =
            AppMode::SessionSwitcher(SessionSwitcherState {
                project_name,
                feature_name,
                tmux_session,
                sessions,
                selected,
                return_window: current_window,
                return_label: current_label,
                vibe_mode,
                review,
            });
    }

    pub fn switch_from_switcher(&mut self) {
        let (
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
            vibe_mode,
            review,
        ) = match &self.mode {
            AppMode::SessionSwitcher(state) => {
                let entry =
                    match state.sessions.get(state.selected)
                    {
                        Some(e) => e,
                        None => return,
                    };
                (
                    state.project_name.clone(),
                    state.feature_name.clone(),
                    state.tmux_session.clone(),
                    entry.tmux_window.clone(),
                    entry.label.clone(),
                    state.vibe_mode.clone(),
                    state.review,
                )
            }
            _ => return,
        };

        self.pane_content.clear();
        self.mode = AppMode::Viewing(ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
            vibe_mode,
            review,
        ));
    }

    pub fn cancel_session_switcher(&mut self) {
        let (
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
            vibe_mode,
            review,
        ) = match &self.mode {
            AppMode::SessionSwitcher(state) => (
                state.project_name.clone(),
                state.feature_name.clone(),
                state.tmux_session.clone(),
                state.return_window.clone(),
                state.return_label.clone(),
                state.vibe_mode.clone(),
                state.review,
            ),
            _ => return,
        };

        self.pane_content.clear();
        self.mode = AppMode::Viewing(ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            window,
            label,
            vibe_mode,
            review,
        ));
    }

    pub fn scan_notifications(&mut self) {
        #[derive(Deserialize)]
        struct NotificationJson {
            session_id: Option<String>,
            cwd: Option<String>,
            message: Option<String>,
            #[serde(alias = "type")]
            notification_type: Option<String>,
            proceed_signal: Option<String>,
            file_path: Option<String>,
            relative_path: Option<String>,
            tool: Option<String>,
            change_id: Option<String>,
            old_snippet: Option<String>,
            new_snippet: Option<String>,
            content_preview: Option<String>,
            response_file: Option<String>,
            reason: Option<String>,
        }

        let mut inputs = Vec::new();

        for project in &self.store.projects {
            for feature in &project.features {
                let notify_dir = feature
                    .workdir
                    .join(".claude")
                    .join("notifications");

                let entries =
                    match std::fs::read_dir(&notify_dir) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                for entry in entries.flatten() {
                    let path = entry.path();
                    if path
                        .extension()
                        .and_then(|e| e.to_str())
                        != Some("json")
                    {
                        continue;
                    }

                    let data =
                        match std::fs::read_to_string(&path)
                        {
                            Ok(d) => d,
                            Err(_) => continue,
                        };

                    let notif: NotificationJson =
                        match serde_json::from_str(&data) {
                            Ok(n) => n,
                            Err(_) => continue,
                        };

                    inputs.push(PendingInput {
                        session_id: notif
                            .session_id
                            .unwrap_or_default(),
                        cwd: notif.cwd.unwrap_or_default(),
                        message: notif
                            .message
                            .unwrap_or_default(),
                        notification_type: notif
                            .notification_type
                            .unwrap_or_default(),
                        file_path: path,
                        project_name: Some(
                            project.name.clone(),
                        ),
                        feature_name: Some(
                            feature.name.clone(),
                        ),
                        proceed_signal: notif.proceed_signal,
                    });
                }
            }
        }

        let global_notify_dir = crate::project::amf_config_dir()
            .join("notifications");

        if let Ok(entries) = std::fs::read_dir(&global_notify_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str())
                    != Some("json")
                {
                    continue;
                }

                let data = match std::fs::read_to_string(&path) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                let notif: NotificationJson =
                    match serde_json::from_str(&data) {
                        Ok(n) => n,
                        Err(_) => continue,
                    };

                let session_id =
                    notif.session_id.unwrap_or_default();
                let cwd = notif.cwd.unwrap_or_default();
                let notification_type =
                    notif.notification_type.unwrap_or_default();
                let proceed_signal_val = notif.proceed_signal.clone();

                if notification_type == "change-reason"
                    && let AppMode::Viewing(ref view) = self.mode
                {
                    let mut found_feature_name = None;
                    let cwd_path = PathBuf::from(&cwd);
                    for project in &self.store.projects {
                        for feature in &project.features {
                            if cwd_path.starts_with(&feature.workdir)
                                || feature.workdir.starts_with(&cwd_path)
                            {
                                found_feature_name = Some(feature.name.clone());
                            }
                        }
                    }

                    if found_feature_name.as_deref() == Some(&view.feature_name) {
                        let response_file = notif
                            .response_file
                            .unwrap_or_default();
                        let proceed_signal_path = proceed_signal_val
                            .unwrap_or_default();

                        self.mode = AppMode::ChangeReasonPrompt(
                            ChangeReasonState {
                                session_id,
                                file_path: notif
                                    .file_path
                                    .unwrap_or_default(),
                                relative_path: notif
                                    .relative_path
                                    .unwrap_or_default(),
                                change_id: notif
                                    .change_id
                                    .unwrap_or_default(),
                                tool: notif.tool.unwrap_or_default(),
                                old_snippet: notif
                                    .old_snippet
                                    .unwrap_or_default(),
                                new_snippet: notif
                                    .new_snippet
                                    .unwrap_or_default(),
                                reason: notif.reason.unwrap_or_default(),
                                response_file: PathBuf::from(response_file),
                                proceed_signal: PathBuf::from(proceed_signal_path),
                            },
                        );
                        let _ = std::fs::remove_file(&path);
                        return;
                    }
                }

                let mut project_name = None;
                let mut feature_name = None;
                let mut best_len: usize = 0;
                let cwd_path = PathBuf::from(&cwd);
                for project in &self.store.projects {
                    for feature in &project.features {
                        let wlen = feature
                            .workdir
                            .as_os_str()
                            .len();
                        if (cwd_path
                            .starts_with(&feature.workdir)
                            || feature
                                .workdir
                                .starts_with(&cwd_path))
                            && wlen > best_len
                        {
                            project_name =
                                Some(project.name.clone());
                            feature_name =
                                Some(feature.name.clone());
                            best_len = wlen;
                        }
                    }
                }

                inputs.push(PendingInput {
                    session_id,
                    cwd,
                    message: notif.message.unwrap_or_default(),
                    notification_type,
                    file_path: path,
                    project_name,
                    feature_name,
                    proceed_signal: notif.proceed_signal,
                });
            }
        }

        self.pending_inputs = inputs;

        if let AppMode::Viewing(ref view) = self.mode {
            let feat_name = view.feature_name.clone();
            for input in &self.pending_inputs {
                if input.notification_type == "diff-review"
                    && input.feature_name.as_deref()
                        == Some(&feat_name)
                    && let Some(signal_path) =
                        &input.proceed_signal
                {
                    let p = Path::new(signal_path);
                    if let Some(parent) = p.parent() {
                        let _ =
                            std::fs::create_dir_all(parent);
                    }
                    let _ = std::fs::write(p, "");
                }
            }
        }
    }

    pub fn handle_notification_select(
        &mut self,
    ) -> Result<()> {
        let idx = match &self.mode {
            AppMode::NotificationPicker(i) => *i,
            _ => return Ok(()),
        };

        let input = match self.pending_inputs.get(idx) {
            Some(i) => i.clone(),
            None => {
                self.mode = AppMode::Normal;
                return Ok(());
            }
        };

        if input.notification_type != "diff-review"
            && input.notification_type != "input-request"
        {
            let _ = std::fs::remove_file(&input.file_path);
        }

        if let (Some(proj_name), Some(feat_name)) =
            (&input.project_name, &input.feature_name)
        {
            let pi = self
                .store
                .projects
                .iter()
                .position(|p| &p.name == proj_name);
            if let Some(pi) = pi {
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| &f.name == feat_name);
                if let Some(fi) = fi {
                    if input.notification_type == "diff-review"
                        && let Some(signal_path) =
                            &input.proceed_signal
                    {
                        let p = Path::new(signal_path);
                        if let Some(parent) = p.parent() {
                            let _ =
                                std::fs::create_dir_all(
                                    parent,
                                );
                        }
                        let _ = std::fs::write(p, "");
                    }
                    self.selection =
                        Selection::Feature(pi, fi);
                    self.pending_inputs.remove(idx);
                    return self.enter_view();
                }
            }
        }

        self.pending_inputs.remove(idx);
        let _ = std::fs::remove_file(&input.file_path);
        self.mode = AppMode::Normal;
        self.message = Some(
            "Notification cleared (no matching feature)"
                .into(),
        );
        Ok(())
    }

    pub fn start_rename_session(&mut self) {
        let (pi, fi, si) = match &self.selection {
            Selection::Session(pi, fi, si) => {
                (*pi, *fi, *si)
            }
            _ => return,
        };

        let label = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .and_then(|f| f.sessions.get(si))
        {
            Some(s) => s.label.clone(),
            None => return,
        };

        self.mode =
            AppMode::RenamingSession(RenameSessionState {
                project_idx: pi,
                feature_idx: fi,
                session_idx: si,
                input: label,
                return_to: RenameReturnTo::Dashboard,
            });
    }

    pub fn start_rename_from_switcher(&mut self) {
        let (pi, fi, si, switcher_state) = match &self.mode
        {
            AppMode::SessionSwitcher(state) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| {
                        p.name == state.project_name
                    });
                let pi = match pi {
                    Some(pi) => pi,
                    None => return,
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| {
                        f.name == state.feature_name
                    });
                let fi = match fi {
                    Some(fi) => fi,
                    None => return,
                };
                let si = state.selected;
                (pi, fi, si, state)
            }
            _ => return,
        };

        let label = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .and_then(|f| f.sessions.get(si))
        {
            Some(s) => s.label.clone(),
            None => return,
        };

        let saved_switcher = SessionSwitcherState {
            project_name: switcher_state
                .project_name
                .clone(),
            feature_name: switcher_state
                .feature_name
                .clone(),
            tmux_session: switcher_state
                .tmux_session
                .clone(),
            sessions: switcher_state
                .sessions
                .iter()
                .map(|s| SwitcherEntry {
                    tmux_window: s.tmux_window.clone(),
                    kind: s.kind.clone(),
                    label: s.label.clone(),
                    icon: s.icon.clone(),
                    icon_nerd: s.icon_nerd.clone(),
                })
                .collect(),
            selected: switcher_state.selected,
            return_window: switcher_state
                .return_window
                .clone(),
            return_label: switcher_state
                .return_label
                .clone(),
            vibe_mode: switcher_state.vibe_mode.clone(),
            review: switcher_state.review,
        };

        self.mode =
            AppMode::RenamingSession(RenameSessionState {
                project_idx: pi,
                feature_idx: fi,
                session_idx: si,
                input: label,
                return_to: RenameReturnTo::SessionSwitcher(
                    saved_switcher,
                ),
            });
    }

    pub fn apply_rename_session(&mut self) -> Result<()> {
        let (pi, fi, si, input) = match &self.mode {
            AppMode::RenamingSession(state) => (
                state.project_idx,
                state.feature_idx,
                state.session_idx,
                state.input.clone(),
            ),
            _ => return Ok(()),
        };

        if input.is_empty() {
            self.message =
                Some("Name cannot be empty".into());
            return Ok(());
        }

        if let Some(session) = self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
            .and_then(|f| f.sessions.get_mut(si))
        {
            session.label = input.clone();
        }
        self.save()?;

        let old_mode = std::mem::replace(
            &mut self.mode,
            AppMode::Normal,
        );
        if let AppMode::RenamingSession(rename_state) =
            old_mode
        {
            match rename_state.return_to {
                RenameReturnTo::Dashboard => {
                    self.mode = AppMode::Normal;
                }
                RenameReturnTo::SessionSwitcher(
                    mut switcher,
                ) => {
                    let feature = &self.store.projects[pi]
                        .features[fi];
                    switcher.sessions = feature
                        .sessions
                        .iter()
                        .map(|s| {
                            let cfg = self
                                .active_extension
                                .custom_sessions
                                .iter()
                                .find(|c| c.name == s.label);
                            SwitcherEntry {
                                tmux_window: s
                                    .tmux_window
                                    .clone(),
                                kind: s.kind.clone(),
                                label: s.label.clone(),
                                icon: cfg.and_then(|c| {
                                    c.icon.clone()
                                }),
                                icon_nerd: cfg.and_then(|c| {
                                    c.icon_nerd.clone()
                                }),
                            }
                        })
                        .collect();
                    self.mode =
                        AppMode::SessionSwitcher(switcher);
                }
            }
        }

        self.message =
            Some(format!("Renamed to '{}'", input));
        Ok(())
    }

    pub fn cancel_rename_session(&mut self) {
        let old_mode = std::mem::replace(
            &mut self.mode,
            AppMode::Normal,
        );
        if let AppMode::RenamingSession(state) = old_mode {
            match state.return_to {
                RenameReturnTo::Dashboard => {
                    self.mode = AppMode::Normal;
                }
                RenameReturnTo::SessionSwitcher(
                    switcher,
                ) => {
                    self.mode =
                        AppMode::SessionSwitcher(switcher);
                }
            }
        }
    }

    pub fn open_command_picker(
        &mut self,
        from_view: Option<ViewState>,
    ) {
        let (repo, workdir) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => {
                let p = self.store.projects.get(*pi);
                (
                    p.map(|p| p.repo.clone()),
                    p.and_then(|p| {
                        p.features
                            .get(*fi)
                            .map(|f| f.workdir.clone())
                    }),
                )
            }
            Selection::Project(pi) => {
                (
                    self.store
                        .projects
                        .get(*pi)
                        .map(|p| p.repo.clone()),
                    None,
                )
            }
        };

        let mut commands = Vec::new();

        if let Some(home) = dirs::home_dir() {
            let global_cmd_dir =
                home.join(".claude").join("commands");
            scan_commands_recursive(
                &global_cmd_dir,
                &global_cmd_dir,
                "Global",
                &mut commands,
            );
        }
        commands.sort_by(|a, b| a.name.cmp(&b.name));

        let mut project_cmds = Vec::new();
        let mut scanned_repo = false;
        if let Some(ref wd) = workdir {
            let workdir_cmd_dir =
                wd.join(".claude").join("commands");
            if workdir_cmd_dir.exists() {
                scan_commands_recursive(
                    &workdir_cmd_dir,
                    &workdir_cmd_dir,
                    "Project",
                    &mut project_cmds,
                );
                scanned_repo = true;
            }
        }

        if !scanned_repo
            && let Some(ref repo) = repo
        {
            let project_cmd_dir =
                repo.join(".claude").join("commands");
            scan_commands_recursive(
                &project_cmd_dir,
                &project_cmd_dir,
                "Project",
                &mut project_cmds,
            );
        }

        project_cmds.sort_by(|a, b| a.name.cmp(&b.name));
        commands.extend(project_cmds);

        self.mode =
            AppMode::CommandPicker(CommandPickerState {
                commands,
                selected: 0,
                from_view,
            });
    }

    pub fn start_search(&mut self) {
        self.mode = AppMode::Searching(SearchState {
            query: String::new(),
            matches: Vec::new(),
            selected_match: 0,
        });
        self.message = None;
    }

    pub fn perform_search(&mut self) {
        let query = match &self.mode {
            AppMode::Searching(state) => state.query.to_lowercase(),
            _ => return,
        };

        let mut matches = Vec::new();

        for (pi, project) in self.store.projects.iter().enumerate() {
            if project.name.to_lowercase().contains(&query) {
                matches.push(SearchMatch {
                    item: VisibleItem::Project(pi),
                    label: project.name.clone(),
                    context: shorten_path(&project.repo),
                });
            }

            for (fi, feature) in project.features.iter().enumerate() {
                if feature.name.to_lowercase().contains(&query) {
                    matches.push(SearchMatch {
                        item: VisibleItem::Feature(pi, fi),
                        label: feature.name.clone(),
                        context: format!("{} / {}", project.name, shorten_path(&feature.workdir)),
                    });
                }

                for (si, session) in feature.sessions.iter().enumerate() {
                    if session.label.to_lowercase().contains(&query) {
                        matches.push(SearchMatch {
                            item: VisibleItem::Session(pi, fi, si),
                            label: session.label.clone(),
                            context: format!("{} / {}", project.name, feature.name),
                        });
                    }
                }
            }
        }

        if let AppMode::Searching(state) = &mut self.mode {
            state.matches = matches;
            if state.selected_match >= state.matches.len() {
                state.selected_match = 0;
            }
        }
    }

    pub fn jump_to_search_match(&mut self) {
        let match_item = match &self.mode {
            AppMode::Searching(state) => {
                state.matches.get(state.selected_match).cloned()
            }
            _ => return,
        };

        if let Some(m) = match_item {
            self.selection = match m.item {
                VisibleItem::Project(pi) => Selection::Project(pi),
                VisibleItem::Feature(pi, fi) => Selection::Feature(pi, fi),
                VisibleItem::Session(pi, fi, si) => Selection::Session(pi, fi, si),
            };
            self.mode = AppMode::Normal;
            self.message = None;
        }
    }

    pub fn cancel_search(&mut self) {
        self.mode = AppMode::Normal;
        self.message = None;
    }

    pub fn select_next_search_match(&mut self) {
        if let AppMode::Searching(state) = &mut self.mode
            && !state.matches.is_empty()
        {
            state.selected_match = (state.selected_match + 1) % state.matches.len();
        }
    }

    pub fn select_prev_search_match(&mut self) {
        if let AppMode::Searching(state) = &mut self.mode
            && !state.matches.is_empty()
        {
            state.selected_match = if state.selected_match == 0 {
                state.matches.len() - 1
            } else {
                state.selected_match - 1
            };
        }
    }

    pub fn pick_session(&mut self) {
        let workdir = match &self.selection {
            Selection::Feature(pi, fi) => {
                self.store
                    .projects
                    .get(*pi)
                    .and_then(|p| p.features.get(*fi))
                    .map(|f| f.workdir.clone())
            }
            Selection::Session(pi, fi, _) => {
                self.store
                    .projects
                    .get(*pi)
                    .and_then(|p| p.features.get(*fi))
                    .map(|f| f.workdir.clone())
            }
            _ => None,
        };
        let workdir = match workdir {
            Some(w) => w,
            None => {
                self.message =
                    Some("Select a feature or session first".into());
                return;
            }
        };

        let sessions = match fetch_opencode_sessions(&workdir) {
            Ok(s) => s,
            Err(e) => {
                self.message =
                    Some(format!("Failed to fetch sessions: {}", e));
                return;
            }
        };

        if sessions.is_empty() {
            self.message =
                Some("No opencode sessions for this worktree".into());
            return;
        }

        self.mode = AppMode::OpencodeSessionPicker(
            OpencodeSessionPickerState {
                sessions,
                selected: 0,
                workdir,
            },
        );
    }

    pub fn cancel_opencode_session_picker(&mut self) {
        self.mode = AppMode::Normal;
    }

    pub fn confirm_opencode_session(&mut self) {
        let session_id = match &self.mode {
            AppMode::OpencodeSessionPicker(state) => {
                state
                    .sessions
                    .get(state.selected)
                    .map(|s| s.id.clone())
            }
            _ => return,
        };

        let session_id = match session_id {
            Some(id) => id,
            None => return,
        };

        let feature_running = self.selected_feature().is_some_and(|(_, f)| {
            f.status != ProjectStatus::Stopped
                && TmuxManager::session_exists(&f.tmux_session)
        });

        if feature_running {
            let workdir = match &self.mode {
                AppMode::OpencodeSessionPicker(state) => {
                    state.workdir.clone()
                }
                _ => return,
            };
            self.mode = AppMode::ConfirmingOpencodeSession {
                session_id,
                workdir,
            };
        } else {
            self.mode = AppMode::Normal;
            if let Err(e) = self.restart_feature_with_opencode_session(&session_id) {
                self.message = Some(format!("Error: {}", e));
            }
        }
    }

    pub fn cancel_opencode_session_confirm(&mut self) {
        let workdir = match &self.mode {
            AppMode::ConfirmingOpencodeSession { workdir, .. } => {
                workdir.clone()
            }
            _ => return,
        };

        self.mode = AppMode::OpencodeSessionPicker(
            OpencodeSessionPickerState {
                sessions: fetch_opencode_sessions(&workdir).unwrap_or_default(),
                selected: 0,
                workdir,
            },
        );
    }

    pub fn confirm_and_start_opencode(&mut self) -> Result<()> {
        let session_id = match &self.mode {
            AppMode::ConfirmingOpencodeSession { session_id, .. } => {
                session_id.clone()
            }
            _ => return Ok(()),
        };

        self.mode = AppMode::Normal;
        self.restart_feature_with_opencode_session(&session_id)
    }

    fn restart_feature_with_opencode_session(
        &mut self,
        opencode_session_id: &str,
    ) -> Result<()> {
        let (pi, fi) = match self.selection {
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => (pi, fi),
            _ => return Ok(()),
        };

        let tmux_session = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.tmux_session.clone());

        let tmux_session = match tmux_session {
            Some(s) => s,
            None => return Ok(()),
        };

        if TmuxManager::session_exists(&tmux_session) {
            TmuxManager::kill_session(&tmux_session)?;
        }

        self.ensure_feature_running_with_opencode_session(pi, fi, opencode_session_id)?;

        let (
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
            review,
        ) = {
            let project = &self.store.projects[pi];
            let feature = &project.features[fi];

            let si = feature
                .sessions
                .iter()
                .position(|s| s.kind == SessionKind::Opencode)
                .unwrap_or(0);

            let session = &feature.sessions[si];
            self.selection = Selection::Session(pi, fi, si);
            (
                project.name.clone(),
                feature.name.clone(),
                feature.tmux_session.clone(),
                session.tmux_window.clone(),
                session.label.clone(),
                feature.mode.clone(),
                feature.review,
            )
        };

        let feature = self.store.projects[pi]
            .features
            .get_mut(fi)
            .unwrap();
        feature.touch();
        feature.status = ProjectStatus::Active;

        let view = ViewState::new(
            project_name,
            feature_name,
            tmux_session,
            session_window,
            session_label,
            vibe_mode,
            review,
        );

        self.save()?;
        self.pane_content.clear();
        self.mode = AppMode::Viewing(view);
        self.message = Some("Restored opencode session".into());

        Ok(())
    }

    fn ensure_feature_running_with_opencode_session(
        &mut self,
        pi: usize,
        fi: usize,
        opencode_session_id: &str,
    ) -> Result<()> {
        let repo = self.store.projects[pi].repo.clone();
        let feature = match self
            .store
            .projects
            .get_mut(pi)
            .and_then(|p| p.features.get_mut(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        ensure_notification_hooks(
            &feature.workdir,
            &repo,
            &feature.mode,
            &feature.agent,
        );
        ensure_review_claude_md(&feature.workdir, feature.review);

        if feature.sessions.is_empty() {
            feature.add_session(SessionKind::Opencode);
            feature.add_session(SessionKind::Terminal);
            if feature.has_notes {
                let s = feature.add_session(SessionKind::Nvim);
                s.label = "Memo".into();
            }
        }

        if TmuxManager::session_exists(&feature.tmux_session) {
            return Ok(());
        }

        TmuxManager::create_session_with_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
            &feature.workdir,
        )?;
        TmuxManager::set_session_env(
            &feature.tmux_session,
            "AMF_SESSION",
            &feature.tmux_session,
        )?;

        for session in &feature.sessions[1..] {
            TmuxManager::create_window(
                &feature.tmux_session,
                &session.tmux_window,
                &feature.workdir,
            )?;
        }

        for session in &feature.sessions {
            match session.kind {
                SessionKind::Opencode => {
                    TmuxManager::launch_opencode_with_session(
                        &feature.tmux_session,
                        &session.tmux_window,
                        Some(opencode_session_id),
                    )?;
                }
                SessionKind::Claude => {
                    let extra_args: Vec<String> = feature.mode.cli_flags(feature.enable_chrome);
                    let extra_refs: Vec<&str> = extra_args.iter().map(|s| s.as_str()).collect();
                    TmuxManager::launch_claude(
                        &feature.tmux_session,
                        &session.tmux_window,
                        session.claude_session_id.as_deref(),
                        &extra_refs,
                    )?;
                }
                SessionKind::Nvim => {
                    if feature.has_notes {
                        TmuxManager::send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim .claude/notes.md",
                        )?;
                    } else {
                        TmuxManager::send_keys(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "nvim",
                        )?;
                    }
                }
                SessionKind::Terminal => {}
                SessionKind::Custom => {
                    if let Some(ref cmd) = session.command {
                        TmuxManager::send_literal(
                            &feature.tmux_session,
                            &session.tmux_window,
                            cmd,
                        )?;
                        TmuxManager::send_key_name(
                            &feature.tmux_session,
                            &session.tmux_window,
                            "Enter",
                        )?;
                    }
                }
            }
        }

        TmuxManager::select_window(
            &feature.tmux_session,
            &feature.sessions[0].tmux_window,
        )?;

        feature.status = ProjectStatus::Idle;
        feature.touch();

        Ok(())
    }

    pub fn open_terminal(&mut self) -> Result<()> {
        let (pi, fi) = match &self.selection {
            Selection::Feature(pi, fi)
            | Selection::Session(pi, fi, _) => (*pi, *fi),
            _ => return Ok(()),
        };

        let feature = match self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
        {
            Some(f) => f,
            None => return Ok(()),
        };

        let session = feature.tmux_session.clone();
        if TmuxManager::session_exists(&session) {
            if let Some(terminal_session) = feature
                .sessions
                .iter()
                .find(|s| s.kind == SessionKind::Terminal)
            {
                let _ = TmuxManager::select_window(
                    &session,
                    &terminal_session.tmux_window,
                );
            }
            if TmuxManager::is_inside_tmux() {
                TmuxManager::switch_client(&session)?;
                self.message = Some(
                    "Switched back from terminal".into(),
                );
            } else {
                self.should_switch = Some(session);
            }
        }

        Ok(())
    }

    pub fn trigger_final_review(&mut self) -> Result<()> {
        // Extract everything we need before mutating self.
        let (workdir, repo, session, feature_name) = match &self.mode {
            AppMode::Viewing(view) => {
                let pi = self
                    .store
                    .projects
                    .iter()
                    .position(|p| p.name == view.project_name);
                let pi = match pi {
                    Some(pi) => pi,
                    None => {
                        return Ok(());
                    }
                };
                let fi = self.store.projects[pi]
                    .features
                    .iter()
                    .position(|f| f.name == view.feature_name);
                let fi = match fi {
                    Some(fi) => fi,
                    None => {
                        return Ok(());
                    }
                };
                let feature =
                    &self.store.projects[pi].features[fi];
                let repo =
                    self.store.projects[pi].repo.clone();
                (
                    feature.workdir.clone(),
                    repo,
                    view.session.clone(),
                    feature.name.clone(),
                )
            }
            _ => return Ok(()),
        };

        // Exit view now so any error messages are visible in list mode.
        self.exit_view();

        // Look in workdir (feature worktree), then repo root, then
        // the directory of the running AMF binary (handles the case
        // where final-review.sh hasn't been committed yet but exists
        // in the worktree AMF was built from).
        let script_suffix = ["plugins", "diff-review", "scripts", "final-review.sh"];
        let amf_root = std::env::current_exe().ok().and_then(|exe| {
            // exe is at <root>/target/{debug,release}/amf — go up 3
            exe.parent()?.parent()?.parent().map(PathBuf::from)
        });
        let script_path = [
            Some(workdir.clone()),
            Some(repo.clone()),
            amf_root,
        ]
        .into_iter()
        .flatten()
        .map(|base| script_suffix.iter().fold(base, |p, s| p.join(s)))
        .find(|p| p.exists());

        let script = match script_path {
            Some(p) => p,
            None => {
                self.message = Some(format!(
                    "final-review.sh not found in {}, {}, or AMF binary dir",
                    workdir.display(),
                    repo.display(),
                ));
                return Ok(());
            }
        };

        // Check if the "terminal" window exists in the current session.
        // If not, create a new "Review" session for this feature.
        let windows = TmuxManager::list_windows(&session).unwrap_or_default();
        let has_terminal = windows.iter().any(|w| w == "terminal");

        let (target_session, target_window) = if has_terminal {
            (session.clone(), "terminal".to_string())
        } else {
            let review_session = format!("amf-{}-Review", feature_name);
            if !TmuxManager::session_exists(&review_session) {
                TmuxManager::create_session_with_window(
                    &review_session,
                    "review",
                    &workdir,
                )?;
            }
            (review_session, "review".to_string())
        };

        // Run the script directly in the feature's terminal pane.
        // Wrapping in display-popup would cause nested-popup failures
        // since final-review.sh opens its own popups for vimdiff/notes.
        // After the script exits, switch back to the AMF session so the
        // user doesn't get stranded in the feature's terminal.
        let amf_session = TmuxManager::current_session()
            .unwrap_or_default();
        let switch_back = if amf_session.is_empty() {
            String::new()
        } else {
            format!("; tmux switch-client -t '{}'", amf_session)
        };
        let cmd = format!(
            "bash '{}' '{}'{}",
            script.to_string_lossy(),
            workdir.to_string_lossy(),
            switch_back,
        );
        if let Err(e) =
            TmuxManager::send_literal(&target_session, &target_window, &cmd)
        {
            self.message =
                Some(format!("Failed to send review command: {e}"));
            return Ok(());
        }
        if let Err(e) =
            TmuxManager::send_key_name(&target_session, &target_window, "Enter")
        {
            self.message =
                Some(format!("Failed to start review: {e}"));
            return Ok(());
        }

        // Switch to the session so the popup is visible.
        if TmuxManager::is_inside_tmux() {
            TmuxManager::switch_client(&target_session)?;
        } else {
            self.should_switch = Some(target_session);
        }

        Ok(())
    }
}

fn scan_commands_recursive(
    base: &Path,
    dir: &Path,
    source: &str,
    out: &mut Vec<CommandEntry>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_commands_recursive(
                base, &path, source, out,
            );
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            == Some("md")
            && let Some(stem) =
                path.file_stem().and_then(|s| s.to_str())
        {
            let name = if let Ok(rel) = dir.strip_prefix(base)
                && !rel.as_os_str().is_empty()
            {
                format!(
                    "{}:{}",
                    rel.to_string_lossy(),
                    stem,
                )
            } else {
                stem.to_string()
            };

            out.push(CommandEntry {
                name,
                source: source.into(),
                path,
            });
        }
    }
}

fn fetch_opencode_sessions(
    workdir: &Path,
) -> Result<Vec<OpencodeSessionInfo>> {
    use std::process::Command;

    let output = Command::new("opencode")
        .args(["session", "list", "--format", "json"])
        .output()?;

    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "opencode session list failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let json_str = String::from_utf8_lossy(&output.stdout);
    let sessions: Vec<serde_json::Value> =
        serde_json::from_str(&json_str)?;

    let dir_str = workdir.to_string_lossy();
    let filtered: Vec<OpencodeSessionInfo> = sessions
        .into_iter()
        .filter(|s| {
            s.get("directory")
                .and_then(|d| d.as_str())
                .map(|d| d == dir_str)
                .unwrap_or(false)
        })
        .filter_map(|s| {
            let id = s.get("id")?.as_str()?.to_string();
            let title = s
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("Untitled")
                .to_string();
            let updated = s
                .get("updated")
                .and_then(|t| t.as_i64())
                .unwrap_or(0);
            Some(OpencodeSessionInfo {
                id,
                title,
                updated,
            })
        })
        .collect();

    Ok(filtered)
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
