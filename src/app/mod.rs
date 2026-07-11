mod automation;
mod bookmarks;
mod claude_session_picker;
mod claude_sessions;
mod codex_live;
mod codex_session_picker;
mod codex_sessions;
pub mod commands;
mod compose;
mod config_wizard;
mod diff;
mod feature_ops;
mod hooks;
mod navigation;
mod notifications;
mod opencode;
pub(crate) mod opencode_storage;
pub(crate) mod pr_review;
mod project_ops;
mod prompt_library;
pub mod remote_control;
mod rename;
pub(crate) mod review;
pub(crate) mod review_memory;
mod search;
mod session_config;
mod session_ops;
mod session_titles;
pub mod setup;
mod skill_picker;
mod state;
mod steering;
mod switcher;
mod sync;
mod syntax;
pub(crate) mod toast;
mod todos;
pub mod util;
mod view;

#[cfg(test)]
mod tests;

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Condvar as StdCondvar, Mutex as StdMutex};

use anyhow::{Context, Result};
use ratatui::text::Line;
use serde::{Deserialize, Serialize};
use std::os::unix::io::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crate::debug::{DebugLog, LogEntry};
use crate::extension::{
    ExtensionConfig, load_global_extension_config, merge_project_extension_config,
};
use crate::perf::PerfCollector;
use crate::project::{
    AgentKind, Feature, FeatureSession, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode,
};
use crate::tmux::TmuxManager;
use crate::token_tracking::{SessionTokenTracker, TokenPricingConfig};
use crate::traits::{TmuxOps, WorktreeOps};
use crate::ui::{render_ansi_lines, render_vt100_screen};
use crate::usage::UsageManager;
use crate::worktree::WorktreeManager;

pub use self::prompt_library::PromptExportTarget;
pub use self::setup::load_config;
pub use codex_live::CodexLiveThreadState;
pub use codex_sessions::sidebar_metadata_for_session_id as codex_sidebar_metadata_for_session_id;
pub(crate) use config_wizard::agent_toggles_to_allowed;
pub use state::*;
pub use steering::{PromptAnalysis, analyze_prompt};
pub use toast::Toast;

pub const VIEW_PANE_REFRESH_INTERVAL: Duration = Duration::from_millis(75);
/// Cadence for the main loop's idle drift-correction reseed. The view
/// workers are event-driven (control-mode %output / pipe-pane FIFO), so
/// this only exists to fix rare parser/pane drift — it must stay slow.
pub const VIEW_DRIFT_RESEED_INTERVAL: Duration = Duration::from_secs(3);
pub const VIEW_CURSOR_REFRESH_INTERVAL: Duration = Duration::from_millis(125);
/// How often AMF re-anchors a live Claude pane with a SIGWINCH bounce.
/// Claude Code's incremental renderer drifts its input-box anchor over
/// time and leaves stale cells in the real tmux grid (the input box
/// bleeds into the divider above it), which AMF can only display
/// faithfully — the corruption is in the pane grid, not our render. A
/// height bounce forces a full agent repaint. Runs on a timer regardless
/// of activity (user-chosen mitigation); the brief flicker is the cost.
pub const VIEW_REANCHOR_BOUNCE_INTERVAL: Duration = Duration::from_secs(3);
/// Minimum dwell at the shrunk height before restoring, so tmux delivers
/// two distinct SIGWINCHes instead of coalescing them into a no-op.
pub const VIEW_REANCHOR_BOUNCE_DWELL: Duration = Duration::from_millis(60);
/// How long to keep the displayed pane frozen after the restore SIGWINCH
/// so Claude Code's full repaint lands before the first frame is
/// revealed — this is what hides the bounce's flicker.
pub const VIEW_REANCHOR_REVEAL_DELAY: Duration = Duration::from_millis(140);
pub const VIEW_STARTUP_WARM_DURATION: Duration = Duration::from_millis(2500);
pub const VIEW_STARTUP_PANE_REFRESH_INTERVAL: Duration = Duration::from_millis(125);
pub const VIEW_STARTUP_CURSOR_REFRESH_INTERVAL: Duration = Duration::from_millis(350);
pub const VIEW_BURST_DURATION: Duration = Duration::from_millis(175);
pub const VIEW_BURST_PANE_REFRESH_INTERVAL: Duration = Duration::from_millis(16);
pub const VIEW_BURST_CURSOR_REFRESH_INTERVAL: Duration = Duration::from_millis(40);
pub const VIEW_BACKGROUND_SYNC_DEFER_INTERVAL: Duration = Duration::from_millis(1500);

/// Minimum spacing between control-mode streaming snapshots (keystroke
/// bursts bypass this); each snapshot costs a full-frame redraw on the
/// main thread.
const VIEW_CONTROL_SNAPSHOT_MIN_INTERVAL: Duration = Duration::from_millis(150);

/// Unconditional self-heal cadence for the control-mode view worker. The
/// control stream is only a change-notifier, so a missed or coalesced
/// redraw (scroll regions, another client repainting, a dropped %output,
/// copy-mode) would otherwise leave a stale/garbled frame on screen until
/// the 3s drift reseed. Re-capturing the full pane on this floor — even
/// when no %output arrived — restores the pre-perf behavior where the
/// view healed almost immediately. Typing latency is no longer a reason
/// to stay event-driven now that the composer batches input, so
/// correctness wins over idle subprocess savings while a pane is open.
const VIEW_CONTROL_HEAL_INTERVAL: Duration = Duration::from_millis(250);

const VIEW_SNAPSHOT_REFRESH_NONE: u8 = 0;
const VIEW_SNAPSHOT_REFRESH_NORMAL: u8 = 1;
const VIEW_SNAPSHOT_REFRESH_BURST: u8 = 2;
const VIEW_SNAPSHOT_REFRESH_PANE_BURST: u8 = 3;

#[derive(Debug, Clone)]
pub struct CodexSidebarMetadataResult {
    pub cache_key: String,
    pub title: Option<String>,
    pub prompt: Option<String>,
    pub model_text: Option<String>,
}

#[derive(Debug)]
pub struct ViewSnapshot {
    pub session: String,
    pub window: String,
    pub pane_content: Option<String>,
    pub rendered_lines: Option<Vec<Line<'static>>>,
    pub cursor: Option<Option<(u16, u16)>>,
    pub capture_duration: Option<Duration>,
    pub render_duration: Option<Duration>,
    pub cursor_duration: Option<Duration>,
    pub pipe_read_duration: Option<Duration>,
}

fn sanitize_tmux_control_line(line: &str) -> &str {
    let line = line.trim_end_matches(['\r', '\n']);
    let line = line.strip_prefix("\u{1b}P1000p").unwrap_or(line);
    line.strip_suffix("\u{1b}\\").unwrap_or(line)
}

fn parse_tmux_output_notification(line: &str) -> Option<(&str, &str)> {
    if let Some(rest) = line.strip_prefix("%output ") {
        return rest.split_once(' ');
    }

    let rest = line.strip_prefix("%extended-output ")?;
    let (metadata, payload) = rest.split_once(" : ")?;
    let pane_id = metadata.split_whitespace().next()?;
    Some((pane_id, payload))
}

fn parser_cursor(parser: &vt100::Parser) -> Option<(u16, u16)> {
    let (row, col) = parser.screen().cursor_position();
    Some((col, row))
}

fn position_parser_cursor(parser: &mut vt100::Parser, cursor: (u16, u16), cols: u16, rows: u16) {
    let (col, row) = cursor;
    let row = row.min(rows.saturating_sub(1)).saturating_add(1);
    let col = col.min(cols.saturating_sub(1)).saturating_add(1);
    parser.process(format!("\x1b[{row};{col}H").as_bytes());
}

#[allow(clippy::too_many_arguments)]
fn snapshot_from_parser(
    session: &str,
    window: &str,
    cols: u16,
    rows: u16,
    parser: &vt100::Parser,
    cursor_override: Option<(u16, u16)>,
    include_content: bool,
    capture_duration: Option<Duration>,
    read_duration: Option<Duration>,
) -> ViewSnapshot {
    let screen = parser.screen();
    // `contents_formatted()` builds the full screen as an escaped string;
    // it is only consumed by selection mode, so skip it on the hot
    // incremental path unless explicitly requested.
    let pane_content =
        include_content.then(|| String::from_utf8_lossy(&screen.contents_formatted()).into_owned());

    ViewSnapshot {
        session: session.to_string(),
        window: window.to_string(),
        pane_content,
        rendered_lines: Some(render_vt100_screen(screen, cols, rows)),
        cursor: Some(cursor_override.or_else(|| parser_cursor(parser))),
        capture_duration,
        render_duration: None,
        cursor_duration: None,
        pipe_read_duration: read_duration,
    }
}

fn reseed_control_view_parser(
    session: &str,
    window: &str,
    cols: u16,
    rows: u16,
    parser: &mut vt100::Parser,
) -> ViewSnapshot {
    *parser = vt100::Parser::new(rows, cols, 0);
    let capture_started_at = Instant::now();
    let captured = TmuxManager::capture_pane_ansi(session, window).unwrap_or_default();
    let capture_duration = capture_started_at.elapsed();
    let normalized = crate::ui::normalize_captured_pane(&captured);
    parser.process(normalized.as_bytes());
    let cursor = TmuxManager::cursor_position(session, window).ok();
    if let Some(cursor) = cursor {
        position_parser_cursor(parser, cursor, cols, rows);
    }

    snapshot_from_parser(
        session,
        window,
        cols,
        rows,
        parser,
        cursor,
        true,
        Some(capture_duration),
        None,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Every variant is a demo injection; the `Demo` postfix is the point.
#[allow(clippy::enum_variant_names)]
pub enum CodexDebugCommand {
    PlanDemo,
    WorkChangeReasonDemo,
    WorkDiffReviewDemo,
    WorkCommandDemo,
    WorkFileDemo,
    WorkInputDemo,
    ClearInputDemo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalCommand {
    OpenDebugLog,
    RefreshNotifications,
    CheckPendingDiffReview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    SlashCommand,
    Local { command: LocalCommand },
    CodexLiveDemo(CodexDebugCommand),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEntry {
    pub name: String,
    pub source: String,
    pub path: Option<PathBuf>,
    pub action: CommandAction,
}

pub struct CommandPickerState {
    pub commands: Vec<CommandEntry>,
    pub selected: usize,
    pub from_view: Option<ViewState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandPickerFocus {
    Default,
    Local,
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

/// On-disk schema version for [`AppConfig`]. Bump this whenever a new
/// migration step is added to `migrate_app_config`; pre-existing config
/// files have no `config_version` field and so read as 0.
pub const APP_CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Schema version, used to gate one-time migrations. Missing in
    /// configs written before versioning, which deserialize it as 0.
    #[serde(default)]
    pub config_version: u32,
    pub nerd_font: bool,
    pub leader_timeout_seconds: u64,
    pub input_request_wait_seconds: f64,
    pub tmux_control_mode: bool,
    pub diff_review_viewer: DiffReviewViewer,
    pub diff_viewer_layout: DiffViewerLayout,
    pub diff_review_popup_hold_secs: f64,
    pub zai: Option<ZaiPlanConfig>,
    pub opencode_theme: Option<String>,
    pub projects: ProjectsConfig,
    pub extension: ExtensionConfig,
    pub theme: crate::theme::ThemeName,
    pub transparent_background: bool,
    #[serde(default)]
    pub token_pricing: TokenPricingConfig,
    /// Default state of the Remote Control toggle for new Claude features.
    /// Still subject to the availability guard (z.ai / provider / version),
    /// so enabling this never forces RC onto an incompatible session.
    #[serde(default)]
    pub remote_control_default: bool,
    /// Periodically repairs embedded tmux views while they are idle. This
    /// can hide Claude Code rendering bugs, but costs extra refresh work and
    /// is opt-in unless the workaround is needed locally.
    #[serde(default)]
    pub view_auto_refresh: bool,
    /// Maximum number of saved agent harness panes AMF auto-launches when a
    /// stopped feature is started. Extra saved agent panes are recreated as
    /// tmux windows but left at the shell prompt to avoid process stampedes.
    /// Set to 0 to restore the previous unlimited behavior.
    #[serde(default = "default_agent_restart_limit")]
    pub max_agent_autostart_sessions: usize,
    /// When finishing a final review, whether to auto-submit the
    /// "address the feedback" prompt to the feature's agent (paste +
    /// Enter). When false, AMF pastes the prompt but does not send Enter,
    /// so the reviewer can eyeball/edit it before submitting.
    #[serde(default = "default_true")]
    pub final_review_submit_prompt: bool,
    /// When finishing a final review, also post the feedback to the branch's
    /// GitHub PR (if one exists) as a review: line comments inline, whole-file
    /// rejections and general feedback in the review summary. Best-effort and
    /// off by default — the local `.claude/final-review-feedback.md` is always
    /// written regardless. Requires an authenticated `gh` CLI.
    #[serde(default)]
    pub final_review_post_to_pr: bool,
    /// Repo-relative (or absolute) path to the review-findings memory doc
    /// (Epic E of `pr-comment-review-plan.md`). Defaults to
    /// [`review_memory::DEFAULT_REVIEW_MEMORY_PATH`] when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_memory_path: Option<String>,
    /// Name of an existing Claude Code skill/command (without the leading `/`,
    /// e.g. `"review"`) to lead the AI PR-review prompt (`A` in the PR-review
    /// pane, Epic E of `pr-comment-review-plan.md`) with, so its review
    /// methodology runs before AMF's own structured-findings instructions.
    /// `None` (default) skips this — AMF ships no bundled skill for reviewing a
    /// PR diff, so the feature must work without assuming one is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_review_skill: Option<String>,
}

fn default_true() -> bool {
    true
}

fn default_agent_restart_limit() -> usize {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum DiffReviewViewer {
    // The native AMF diff viewer is the only supported reviewer. The legacy
    // vimdiff/neovim popup (`nvim`/`legacy`) has been retired; those values
    // are still accepted and map to the AMF viewer so old configs keep loading.
    #[default]
    #[serde(rename = "amf", alias = "custom", alias = "nvim", alias = "legacy")]
    Amf,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ProjectsConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_preferred_agent: Option<AgentKind>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            config_version: APP_CONFIG_VERSION,
            nerd_font: true,
            leader_timeout_seconds: 5,
            input_request_wait_seconds: 1.5,
            // Direct tmux transport by default: the persistent control-mode
            // (-CC PTY) clients desync Claude Code's incremental renderer and
            // cause the long-standing mangled-pane corruption on Linux/macOS.
            // Control mode stays available as an opt-in (config /
            // AMF_TMUX_INPUT_TRANSPORT / AMF_EXPERIMENTAL_PERSISTENT_TMUX_INPUT).
            tmux_control_mode: false,
            diff_review_viewer: DiffReviewViewer::default(),
            diff_viewer_layout: DiffViewerLayout::Unified,
            diff_review_popup_hold_secs: 1.5,
            zai: None,
            opencode_theme: Some("catppuccin-frappe".to_string()),
            projects: ProjectsConfig::default(),
            extension: ExtensionConfig::default(),
            theme: crate::theme::ThemeName::default(),
            transparent_background: false,
            token_pricing: TokenPricingConfig::default(),
            remote_control_default: false,
            view_auto_refresh: false,
            max_agent_autostart_sessions: default_agent_restart_limit(),
            final_review_submit_prompt: true,
            final_review_post_to_pr: false,
            review_memory_path: None,
            ai_review_skill: None,
        }
    }
}

impl AppConfig {
    pub fn input_request_wait_duration(&self) -> Duration {
        let seconds = if self.input_request_wait_seconds.is_finite()
            && self.input_request_wait_seconds >= 0.0
        {
            self.input_request_wait_seconds
        } else {
            AppConfig::default().input_request_wait_seconds
        };

        Duration::from_secs_f64(seconds)
    }
}

/// Latest-wins snapshot mailbox: the renderer only ever needs the newest
/// frame, so a single merged slot replaces an unbounded channel — backlog
/// cannot form if the main thread stalls. Every delivery also writes to
/// the wakeup pipe so the main loop leaves `poll()` immediately.
#[derive(Clone)]
struct SnapshotSender {
    slot: Arc<StdMutex<Option<ViewSnapshot>>>,
    wakeup_fd: Arc<OwnedFd>,
}

impl SnapshotSender {
    fn send(&self, snap: ViewSnapshot) {
        {
            let mut slot = self.slot.lock().unwrap();
            merge_snapshot_into_slot(&mut slot, snap);
        }
        let byte = 1u8;
        unsafe {
            libc::write(
                self.wakeup_fd.as_raw_fd(),
                &byte as *const u8 as *const _,
                1,
            )
        };
    }
}

/// Merge an undelivered snapshot with a newer one for the same pane:
/// newest value wins per field, so a cursor-only update cannot wipe out
/// pane content the main loop has not consumed yet.
fn merge_snapshot_into_slot(slot: &mut Option<ViewSnapshot>, new: ViewSnapshot) {
    match slot {
        Some(old) if old.session == new.session && old.window == new.window => {
            if new.pane_content.is_some() {
                old.pane_content = new.pane_content;
            }
            if new.rendered_lines.is_some() {
                old.rendered_lines = new.rendered_lines;
            }
            if new.cursor.is_some() {
                old.cursor = new.cursor;
            }
            if new.capture_duration.is_some() {
                old.capture_duration = new.capture_duration;
            }
            if new.render_duration.is_some() {
                old.render_duration = new.render_duration;
            }
            if new.cursor_duration.is_some() {
                old.cursor_duration = new.cursor_duration;
            }
            if new.pipe_read_duration.is_some() {
                old.pipe_read_duration = new.pipe_read_duration;
            }
        }
        _ => *slot = Some(new),
    }
}

pub struct App {
    pub store: ProjectStore,
    pub store_path: PathBuf,
    pub db: Option<crate::db::AmfDb>,
    pub config: AppConfig,
    pub active_extension: ExtensionConfig,
    pub theme: crate::theme::Theme,
    pub selection: Selection,
    pub mode: AppMode,
    pub message: Option<String>,
    pub toasts: Vec<Toast>,
    pub should_quit: bool,
    pub pane_content: String,
    pub pane_lines: Vec<Line<'static>>,
    pub pane_content_cols: u16,
    pub pane_content_rows: u16,
    pub viewport_cols: u16,
    pub viewport_rows: u16,
    /// Full terminal height; viewport_rows is the dashboard list area
    /// (height minus chrome), while the view content area is
    /// viewport_total_rows - 1 (header only).
    pub viewport_total_rows: u16,
    pub tmux_cursor: Option<(u16, u16)>,
    pub leader_active: bool,
    pub leader_activated_at: Option<Instant>,
    pub last_view_activity_at: Option<Instant>,
    pub view_input_batch: Option<ViewInputBatch>,
    pub pending_inputs: Vec<PendingInput>,
    /// Tmux "session:window" targets where compose interception is
    /// disabled and keys pass straight through to Claude Code.
    pub compose_direct_targets: std::collections::HashSet<String>,
    /// Unsent compose drafts keyed by tmux "session:window" target.
    pub compose_drafts: HashMap<String, ComposeDraft>,
    compose_clipboard_paste: Option<ComposeClipboardPaste>,
    next_compose_clipboard_paste_id: u64,
    pub latest_prompt_cache: HashMap<String, String>,
    pub sidebar_model_cache: HashMap<String, String>,
    pub sidebar_plan_cache: HashMap<String, String>,
    pub codex_session_title_cache: HashMap<String, Option<String>>,
    pub codex_session_prompt_cache: HashMap<String, Option<String>>,
    pub codex_session_model_cache: HashMap<String, Option<String>>,
    pub codex_live_threads: HashMap<String, CodexLiveThreadState>,
    pub codex_sidebar_metadata_tx: std::sync::mpsc::Sender<CodexSidebarMetadataResult>,
    pub codex_sidebar_metadata_rx: std::sync::mpsc::Receiver<CodexSidebarMetadataResult>,
    pub codex_sidebar_metadata_inflight: std::collections::HashSet<String>,
    pub opencode_sidebar_cache: HashMap<String, opencode_storage::OpencodeSidebarData>,
    sidebar_load_tx: Sender<SidebarLoadResult>,
    sidebar_load_rx: Receiver<SidebarLoadResult>,
    sidebar_load_executor: Option<SidebarLoadExecutor>,
    sidebar_load_signatures: HashMap<String, u64>,
    pending_sidebar_loads: std::collections::HashSet<String>,
    pub usage: UsageManager,
    pub token_tracker: SessionTokenTracker,
    pub session_status_bg: Option<Receiver<sync::SessionStatusBgResult>>,
    /// Receiver for the background PR-comment fetch (see `app::pr_review`).
    pub pr_review_bg: Option<Receiver<Result<pr_review::PrReview>>>,
    /// A review pane stashed by `pr_review_toggle_to_session` (`P`) while the
    /// user watches the linked fix session; `leader+P` pops it back without a
    /// re-fetch. See [`PrReviewReturn`].
    pub pr_review_return: Option<PrReviewReturn>,
    /// Receiver for the background review-memory lookback bootstrap (fetch +
    /// distill pass). See `app::pr_review::run_review_memory_bootstrap`.
    pub review_memory_bootstrap_bg: Option<Receiver<pr_review::BootstrapProgress>>,
    /// Receiver for the background AI code review of the current PR's diff
    /// (the `A` action). See `app::pr_review::run_ai_pr_review`.
    pub ai_review_bg: Option<Receiver<pr_review::AiReviewProgress>>,
    pub scroll_offset: usize,
    pub session_filter: SessionFilter,
    pub throbber_state: throbber_widgets_tui::ThrobberState,
    pub thinking_features: std::collections::HashSet<String>,
    /// Rate-limits the opencode `capture-pane` thinking fallback: the
    /// 500ms thinking tick must not spawn a subprocess per feature.
    pub(crate) opencode_thinking_pane_cache: HashMap<String, (Instant, Option<bool>)>,
    pub ipc_thinking_sessions: std::collections::HashSet<String>,
    pub ipc_tool_sessions: std::collections::HashSet<String>,
    pub summary_state: SummaryState,
    pub summary_rx: Option<std::sync::mpsc::Receiver<(String, Result<String, anyhow::Error>)>>,
    pub tmux: Box<dyn TmuxOps>,
    pub worktree: Box<dyn WorktreeOps>,
    pub debug_log: DebugLog,
    pending_debug_log_entries: VecDeque<LogEntry>,
    last_debug_log_flush_at: Instant,
    pub perf: PerfCollector,
    pub background_deletions: HashMap<String, BackgroundDeletion>,
    pub background_hooks: HashMap<String, BackgroundHook>,
    pub ipc: Option<crate::ipc::IpcGuard>,
    /// Single inotify worker replacing periodic filesystem scans;
    /// None when the watcher could not start (timers remain fallbacks).
    pub fs_watcher: Option<crate::fswatch::FsWatcher>,
    /// Persistent tmux control-mode client observing session lifecycle;
    /// None in tests (status polling falls back to its interval).
    pub tmux_observer: Option<crate::tmux_observer::TmuxObserver>,
    pub(crate) ipc_chatty_log_count: u64,
    pub(crate) ipc_chatty_log_window_start: Instant,
    pub last_file_notification_count: usize,
    pub last_file_notification_fingerprint: Option<u64>,
    pub vscode_available: bool,
    view_snapshot_tx: SnapshotSender,
    view_snapshot_wakeup_rx: Option<OwnedFd>,
    view_snapshot_stop: Option<Arc<AtomicBool>>,
    view_snapshot_refresh: Option<Arc<AtomicU8>>,
    view_snapshot_include_content: Option<Arc<AtomicBool>>,
    view_snapshot_condvar: Option<Arc<(StdMutex<()>, StdCondvar)>>,
    view_snapshot_target: Option<(String, String, u16, u16)>,
    /// While set and in the future, `drain_view_snapshots` leaves the
    /// displayed pane frozen on its last good frame. Used to hide the
    /// re-anchor bounce: the shrink/restore SIGWINCH pair and Claude
    /// Code's full repaint happen off-screen, and only the first clean
    /// frame after the pane is back to full height is revealed — so a
    /// clean pane shows no wobble and a corrupted one just resolves.
    view_display_frozen_until: Option<Instant>,
    pub harness_check_tx: Sender<HarnessCheckResult>,
    harness_check_rx: Receiver<HarnessCheckResult>,
}

pub(crate) struct HarnessCheckResult {
    pub kind: AgentKind,
    pub result: Result<()>,
}

struct ComposeClipboardPaste {
    id: u64,
    target: String,
    rx: Receiver<Result<util::ClipboardContent, String>>,
}

struct SidebarLoadResult {
    tmux_session: String,
    signature: u64,
    changed: bool,
    latest_prompt: Option<String>,
    model_text: Option<String>,
    opencode_sidebar: Option<opencode_storage::OpencodeSidebarData>,
}

type SidebarLoadTask = Box<dyn FnOnce() + Send + 'static>;

struct SidebarLoadQueue {
    tasks: VecDeque<SidebarLoadTask>,
    stopping: bool,
}

struct SidebarLoadExecutor {
    queue: Arc<(StdMutex<SidebarLoadQueue>, StdCondvar)>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl SidebarLoadExecutor {
    const WORKER_COUNT: usize = 4;

    fn new() -> Self {
        Self::with_worker_count(Self::WORKER_COUNT)
    }

    fn with_worker_count(worker_count: usize) -> Self {
        assert!(worker_count > 0);
        let queue = Arc::new((
            StdMutex::new(SidebarLoadQueue {
                tasks: VecDeque::new(),
                stopping: false,
            }),
            StdCondvar::new(),
        ));
        let workers = (0..worker_count)
            .map(|index| {
                let queue = Arc::clone(&queue);
                std::thread::Builder::new()
                    .name(format!("sidebar-loader-{index}"))
                    .spawn(move || {
                        loop {
                            let task = {
                                let (lock, ready) = &*queue;
                                let mut state = lock.lock().unwrap();
                                while state.tasks.is_empty() && !state.stopping {
                                    state = ready.wait(state).unwrap();
                                }
                                if state.tasks.is_empty() && state.stopping {
                                    return;
                                }
                                state.tasks.pop_front()
                            };
                            if let Some(task) = task {
                                task();
                            }
                        }
                    })
                    .expect("failed to start sidebar loader worker")
            })
            .collect();
        Self { queue, workers }
    }

    fn submit(&self, task: SidebarLoadTask) {
        let (lock, ready) = &*self.queue;
        let mut state = lock.lock().unwrap();
        state.tasks.push_back(task);
        ready.notify_one();
    }
}

impl Drop for SidebarLoadExecutor {
    fn drop(&mut self) {
        let (lock, ready) = &*self.queue;
        let mut state = lock.lock().unwrap();
        state.stopping = true;
        state.tasks.clear();
        drop(state);
        ready.notify_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

pub struct ViewInputBatch {
    pub session: String,
    pub window: String,
    pub text: String,
}

impl App {
    pub fn note_view_activity(&mut self) {
        self.last_view_activity_at = Some(Instant::now());
    }

    fn has_dashboard_animation(&self) -> bool {
        !self.thinking_features.is_empty()
            || !self.background_hooks.is_empty()
            || !self.background_deletions.is_empty()
            || !self.summary_state.generating.is_empty()
    }

    pub(crate) fn has_visible_animation(&self) -> bool {
        let base = match &self.mode {
            AppMode::Normal => self.has_dashboard_animation(),
            AppMode::RunningHook(state) => state.child.is_some(),
            AppMode::DeletingFeatureInProgress(state) => state.child.is_some(),
            AppMode::SyntaxLanguagePicker(state) => state.operation.is_some(),
            AppMode::DiffReviewPrompt(state) => {
                state.explanation_child.is_some() || state.hold_active()
            }
            AppMode::DiffViewerLoading(_) => true,
            AppMode::MarkdownLoading(_) => true,
            AppMode::HarnessSetup(state) => state
                .harnesses
                .iter()
                .any(|h| h.status == HarnessCheckStatus::Checking),
            // The AI review keeps running in the background after `esc`
            // returns here; animate the header's throbber for as long as it
            // is in flight (see `ui::dialogs::draw_pr_review`).
            AppMode::PrReview(_) => self.ai_review_bg.is_some(),
            _ => false,
        };
        base || self.has_active_toasts()
    }

    pub fn should_defer_view_background_sync(&self) -> bool {
        matches!(self.mode, AppMode::Viewing(_))
            && self
                .last_view_activity_at
                .is_some_and(|instant| instant.elapsed() < VIEW_BACKGROUND_SYNC_DEFER_INTERVAL)
    }

    pub fn redraw_signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();

        std::mem::discriminant(&self.mode).hash(&mut hasher);
        std::mem::discriminant(&self.selection).hash(&mut hasher);
        self.should_quit.hash(&mut hasher);
        self.leader_active.hash(&mut hasher);
        self.message.hash(&mut hasher);
        self.toasts.len().hash(&mut hasher);
        self.pending_inputs.len().hash(&mut hasher);
        self.tmux_cursor.hash(&mut hasher);

        match &self.selection {
            Selection::Project(pi) => {
                0usize.hash(&mut hasher);
                pi.hash(&mut hasher);
            }
            Selection::Feature(pi, fi) => {
                1usize.hash(&mut hasher);
                pi.hash(&mut hasher);
                fi.hash(&mut hasher);
            }
            Selection::Session(pi, fi, si) => {
                2usize.hash(&mut hasher);
                pi.hash(&mut hasher);
                fi.hash(&mut hasher);
                si.hash(&mut hasher);
            }
        }

        if let AppMode::Viewing(view) = &self.mode {
            view.project_name.hash(&mut hasher);
            view.feature_name.hash(&mut hasher);
            view.session.hash(&mut hasher);
            view.window.hash(&mut hasher);
            view.session_label.hash(&mut hasher);
            std::mem::discriminant(&view.session_kind).hash(&mut hasher);
            std::mem::discriminant(&view.vibe_mode).hash(&mut hasher);
            view.review.hash(&mut hasher);
            view.scroll_offset.hash(&mut hasher);
            view.scroll_mode.hash(&mut hasher);
            view.scroll_total_lines.hash(&mut hasher);
            view.scroll_passthrough.hash(&mut hasher);
            view.selection.start_row.hash(&mut hasher);
            view.selection.start_col.hash(&mut hasher);
            view.selection.end_row.hash(&mut hasher);
            view.selection.end_col.hash(&mut hasher);
            view.selection.is_selecting.hash(&mut hasher);
            view.selection.has_selection.hash(&mut hasher);
            view.sidebar_visible.hash(&mut hasher);
            view.todos_expanded.hash(&mut hasher);
        }

        if let AppMode::Compose(state) = &self.mode {
            state.view.project_name.hash(&mut hasher);
            state.view.feature_name.hash(&mut hasher);
            state.view.session.hash(&mut hasher);
            state.view.window.hash(&mut hasher);
            state.view.session_label.hash(&mut hasher);
            state.editor.text().hash(&mut hasher);
            state.scroll_offset.hash(&mut hasher);
            state.images.len().hash(&mut hasher);
            state.clipboard_paste_id.hash(&mut hasher);
        }

        hasher.finish()
    }

    pub fn has_pending_view_input(&self) -> bool {
        self.view_input_batch.is_some()
    }

    pub fn pending_view_input_len(&self) -> usize {
        self.view_input_batch
            .as_ref()
            .map(|batch| batch.text.len())
            .unwrap_or(0)
    }

    pub fn pending_view_input_targets(&self, session: &str, window: &str) -> bool {
        self.view_input_batch
            .as_ref()
            .is_some_and(|batch| batch.session == session && batch.window == window)
    }

    pub fn queue_view_literal_input(&mut self, session: &str, window: &str, text: &str) {
        match &mut self.view_input_batch {
            Some(batch) if batch.session == session && batch.window == window => {
                batch.text.push_str(text);
            }
            _ => {
                self.view_input_batch = Some(ViewInputBatch {
                    session: session.to_string(),
                    window: window.to_string(),
                    text: text.to_string(),
                });
            }
        }
    }

    pub fn flush_view_input_batch(&mut self) -> Result<bool> {
        let Some(batch) = self.view_input_batch.take() else {
            return Ok(false);
        };

        let started_at = Instant::now();
        let result = self
            .tmux
            .send_literal(&batch.session, &batch.window, &batch.text);
        self.perf
            .record_duration("view.send_literal", started_at.elapsed());
        if result.is_ok() {
            if TmuxManager::uses_control_pty_input() {
                self.request_view_snapshot_pane_burst();
            } else {
                self.request_view_snapshot_burst();
            }
        }
        result.map(|_| true)
    }

    fn current_view_snapshot_target(&self) -> Option<(String, String, u16, u16)> {
        // The compose box keeps the live pane visible above it, so the
        // snapshot worker must stay on the viewed session while typing.
        let view = match &self.mode {
            AppMode::Viewing(view) => view,
            AppMode::Compose(state) => &state.view,
            _ => return None,
        };

        if self.pane_content_cols == 0 || self.pane_content_rows == 0 {
            return None;
        }

        Some((
            view.session.clone(),
            view.window.clone(),
            self.pane_content_cols,
            self.pane_content_rows,
        ))
    }

    fn stop_view_snapshot_worker(&mut self) {
        if let Some(stop) = self.view_snapshot_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        // Wake the worker so it notices the stop flag immediately.
        if let Some(cv) = &self.view_snapshot_condvar {
            cv.1.notify_one();
        }
        self.view_snapshot_refresh = None;
        self.view_snapshot_include_content = None;
        self.view_snapshot_condvar = None;
        self.view_snapshot_target = None;
    }

    /// Keep the worker's include-content flag in sync with whether
    /// selection currently needs `pane_content`. On the rising edge,
    /// request a refresh so full content arrives promptly.
    fn sync_view_snapshot_content_flag(&self) {
        let Some(flag) = &self.view_snapshot_include_content else {
            return;
        };
        let want = match &self.mode {
            AppMode::Viewing(view) => view.selection.is_selecting || view.selection.has_selection,
            _ => false,
        };
        if flag.swap(want, Ordering::Relaxed) != want && want {
            self.request_view_snapshot_refresh();
        }
    }

    pub fn request_view_snapshot_refresh(&self) {
        self.request_view_snapshot_refresh_kind(VIEW_SNAPSHOT_REFRESH_NORMAL);
    }

    pub fn request_view_snapshot_burst(&self) {
        self.request_view_snapshot_refresh_kind(VIEW_SNAPSHOT_REFRESH_BURST);
    }

    pub fn request_view_snapshot_pane_burst(&self) {
        self.request_view_snapshot_refresh_kind(VIEW_SNAPSHOT_REFRESH_PANE_BURST);
    }

    /// Returns the read end of the wakeup pipe so the main loop can use
    /// `libc::poll()` to wake immediately when a snapshot is ready.
    pub fn view_wakeup_rx_fd(&self) -> Option<RawFd> {
        self.view_snapshot_wakeup_rx
            .as_ref()
            .map(|fd| fd.as_raw_fd())
    }

    /// Write end of the wakeup pipe. Other event sources (e.g. the IPC
    /// listener) write a byte to it so the main loop's `poll()` returns
    /// immediately instead of waiting out its timeout.
    pub fn view_wakeup_tx(&self) -> Arc<OwnedFd> {
        self.view_snapshot_tx.wakeup_fd.clone()
    }

    /// Consume the watcher's thinking-dirty flag. False when no watcher
    /// is running (callers then rely on their interval fallback).
    pub fn take_thinking_dirty(&self) -> bool {
        self.fs_watcher
            .as_ref()
            .is_some_and(|watcher| watcher.flags.take_thinking_dirty())
    }

    /// Consume the watcher's notifications-dirty flag. False when no
    /// watcher is running (file-based fallback notifications are then
    /// only surfaced manually via the V keybind).
    pub fn take_notifications_dirty(&self) -> bool {
        self.fs_watcher
            .as_ref()
            .is_some_and(|watcher| watcher.flags.take_notifications_dirty())
    }

    /// Usage-stats refresh hint: `None` when no watcher is running
    /// (interval-only behavior), otherwise whether transcripts changed
    /// since the last check.
    pub fn usage_stats_hint(&self) -> Option<bool> {
        self.fs_watcher
            .as_ref()
            .map(|watcher| watcher.flags.take_usage_dirty())
    }

    /// Schedule sidebar reloads for features whose latest-prompt file
    /// changed on disk. Returns whether any reload was scheduled.
    pub fn handle_prompt_file_events(&mut self) -> bool {
        let Some(watcher) = &self.fs_watcher else {
            return false;
        };
        let paths = watcher.flags.take_prompt_paths();
        if paths.is_empty() {
            return false;
        }

        let mut handled = false;
        for path in paths {
            // {workdir}/.claude/latest-prompt.txt → workdir
            let Some(workdir) = path.parent().and_then(|dir| dir.parent()) else {
                continue;
            };
            let target = self
                .store
                .projects
                .iter()
                .enumerate()
                .find_map(|(pi, project)| {
                    project
                        .features
                        .iter()
                        .position(|feature| feature.workdir == workdir)
                        .map(|fi| (pi, fi))
                });
            if let Some((pi, fi)) = target {
                self.schedule_sidebar_load_for_feature(pi, fi);
                handled = true;
            }
        }
        handled
    }

    /// Consume the tmux observer's sessions-dirty flag. False when no
    /// observer is running (status sync then uses its legacy interval).
    pub fn take_sessions_dirty(&self) -> bool {
        self.tmux_observer
            .as_ref()
            .is_some_and(|observer| observer.flags.take_sessions_dirty())
    }

    /// Reconcile per-feature prompt-file watches with the current store.
    pub fn refresh_fs_watch_paths(&mut self) {
        let Some(watcher) = &mut self.fs_watcher else {
            return;
        };
        let workdirs: Vec<PathBuf> = self
            .store
            .projects
            .iter()
            .flat_map(|project| project.features.iter().map(|f| f.workdir.clone()))
            .collect();
        watcher.sync_prompt_watches(workdirs);
    }

    fn request_view_snapshot_refresh_kind(&self, kind: u8) {
        if let Some(refresh) = &self.view_snapshot_refresh {
            let mut current = refresh.load(Ordering::Relaxed);
            while current < kind {
                match refresh.compare_exchange_weak(
                    current,
                    kind,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(observed) => current = observed,
                }
            }
        }
        if let Some(cv) = &self.view_snapshot_condvar {
            cv.1.notify_one();
        }
    }

    /// Fallback worker: polls capture-pane at regular intervals.
    // Worker wiring: every arg is a distinct channel/flag shared with the
    // spawning thread.
    #[allow(clippy::too_many_arguments)]
    fn run_capture_pane_worker(
        session: &str,
        window: &str,
        cols: u16,
        rows: u16,
        stop: &AtomicBool,
        refresh: &AtomicU8,
        condvar: &(StdMutex<()>, StdCondvar),
        tx: &SnapshotSender,
    ) {
        let mut next_pane_refresh = Instant::now() - VIEW_PANE_REFRESH_INTERVAL;
        let mut next_cursor_refresh = Instant::now() - VIEW_CURSOR_REFRESH_INTERVAL;
        let mut burst_until = Instant::now();
        let worker_started_at = Instant::now();

        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            let refresh_kind = refresh.swap(VIEW_SNAPSHOT_REFRESH_NONE, Ordering::Relaxed);
            let refresh_requested = refresh_kind != VIEW_SNAPSHOT_REFRESH_NONE;
            let cursor_refresh_requested = matches!(
                refresh_kind,
                VIEW_SNAPSHOT_REFRESH_NORMAL | VIEW_SNAPSHOT_REFRESH_BURST
            );
            if matches!(
                refresh_kind,
                VIEW_SNAPSHOT_REFRESH_BURST | VIEW_SNAPSHOT_REFRESH_PANE_BURST
            ) {
                burst_until = now + VIEW_BURST_DURATION;
            }
            let burst_active = now < burst_until;
            let startup_active = now.duration_since(worker_started_at) < VIEW_STARTUP_WARM_DURATION;
            let pane_interval = if burst_active {
                VIEW_BURST_PANE_REFRESH_INTERVAL
            } else if startup_active {
                VIEW_STARTUP_PANE_REFRESH_INTERVAL
            } else {
                VIEW_PANE_REFRESH_INTERVAL
            };
            let cursor_interval = if burst_active {
                VIEW_BURST_CURSOR_REFRESH_INTERVAL
            } else if startup_active {
                VIEW_STARTUP_CURSOR_REFRESH_INTERVAL
            } else {
                VIEW_CURSOR_REFRESH_INTERVAL
            };
            let pane_due =
                refresh_requested || now.duration_since(next_pane_refresh) >= pane_interval;
            let cursor_due = cursor_refresh_requested
                || now.duration_since(next_cursor_refresh) >= cursor_interval;

            if pane_due || cursor_due {
                let mut pane_content = None;
                let mut rendered_lines = None;
                let mut capture_duration = None;
                let mut render_duration = None;
                let mut cursor = None;
                let mut cursor_duration = None;

                if pane_due {
                    let started_at = Instant::now();
                    let captured =
                        TmuxManager::capture_pane_ansi(session, window).unwrap_or_default();
                    capture_duration = Some(started_at.elapsed());
                    let render_started_at = Instant::now();
                    rendered_lines = Some(render_ansi_lines(&captured, cols, rows));
                    render_duration = Some(render_started_at.elapsed());
                    pane_content = Some(captured);
                    next_pane_refresh = Instant::now();
                }

                if cursor_due {
                    let cursor_started_at = Instant::now();
                    cursor = Some(TmuxManager::cursor_position(session, window).ok());
                    cursor_duration = Some(cursor_started_at.elapsed());
                    next_cursor_refresh = Instant::now();
                }

                tx.send(ViewSnapshot {
                    session: session.to_string(),
                    window: window.to_string(),
                    pane_content,
                    rendered_lines,
                    cursor,
                    capture_duration,
                    render_duration,
                    cursor_duration,
                    pipe_read_duration: None,
                });
                continue;
            }

            let guard = condvar.0.lock().unwrap();
            let _ = condvar.1.wait_timeout(guard, Duration::from_millis(10));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_control_mode_view_worker(
        session: &str,
        window: &str,
        cols: u16,
        rows: u16,
        stop: &AtomicBool,
        refresh: &AtomicU8,
        _include_content: &AtomicBool,
        auto_refresh: bool,
        condvar: &(StdMutex<()>, StdCondvar),
        tx: &SnapshotSender,
    ) -> anyhow::Result<()> {
        let (target_window_id, mut target_pane_id) =
            TmuxManager::resolve_view_target_ids(session, window)?;
        let mut client = TmuxManager::spawn_control_mode_view_client(
            session,
            window,
            &target_pane_id,
            cols,
            rows,
        )?;
        let mut parser = vt100::Parser::new(rows, cols, 0);
        let mut debug_lines_remaining = 20usize;

        crate::debug::log_to_file(
            crate::debug::LogLevel::Info,
            "perf",
            &format!(
                "control-mode worker started for {session}:{window} window_id={target_window_id} pane_id={target_pane_id}"
            ),
        );

        let mut debug_initial_lines_remaining = 10usize;
        while let Some(raw_line) = client.try_recv()? {
            if debug_initial_lines_remaining > 0 {
                let line = sanitize_tmux_control_line(&raw_line);
                if !line.is_empty() {
                    crate::debug::log_to_file(
                        crate::debug::LogLevel::Debug,
                        "tmux",
                        &format!("control view initial recv: {line:?}"),
                    );
                    debug_initial_lines_remaining -= 1;
                }
            }
        }

        tx.send(reseed_control_view_parser(
            session,
            window,
            cols,
            rows,
            &mut parser,
        ));

        // The control stream is used purely as a change notifier;
        // content always comes from capture-pane reseeds. Feeding
        // %output into a persistent parser drifts whenever the agent
        // emits sequences vt100 interprets differently than tmux
        // (scroll regions, insert/delete line), which lands typed echo
        // on the wrong row. A reseed costs ~5ms in this worker thread,
        // so correctness wins: keystroke bursts reseed immediately for
        // low-latency echo, streaming output is coalesced so the main
        // thread is not saturated with full-frame redraws.
        let mut burst_until = Instant::now();
        let mut last_snapshot = Instant::now();
        let mut pane_dirty = false;

        while !stop.load(Ordering::Relaxed) {
            match refresh.swap(VIEW_SNAPSHOT_REFRESH_NONE, Ordering::Relaxed) {
                VIEW_SNAPSHOT_REFRESH_NORMAL => {
                    // Periodic drift-correction reseed.
                    tx.send(reseed_control_view_parser(
                        session,
                        window,
                        cols,
                        rows,
                        &mut parser,
                    ));
                    pane_dirty = false;
                    last_snapshot = Instant::now();
                }
                VIEW_SNAPSHOT_REFRESH_BURST | VIEW_SNAPSHOT_REFRESH_PANE_BURST => {
                    burst_until = Instant::now() + VIEW_BURST_DURATION;
                    pane_dirty = true;
                }
                _ => {}
            }

            // Non-blocking recv first; wait on condvar so burst signals from
            // request_view_snapshot_refresh_kind() wake this loop immediately.
            let line = match client.try_recv()? {
                Some(line) => Some(line),
                None => {
                    let guard = condvar.0.lock().unwrap();
                    let _ = condvar.1.wait_timeout(guard, Duration::from_millis(5));
                    client.try_recv()?
                }
            };

            if let Some(line) = line {
                let mut pending_lines = vec![line];
                while let Some(extra_line) = client.try_recv()? {
                    pending_lines.push(extra_line);
                }

                for raw_line in pending_lines {
                    let line = sanitize_tmux_control_line(&raw_line);
                    if line.is_empty() {
                        continue;
                    }
                    if debug_lines_remaining > 0 {
                        crate::debug::log_to_file(
                            crate::debug::LogLevel::Debug,
                            "tmux",
                            &format!("control view recv: {line:?}"),
                        );
                        debug_lines_remaining -= 1;
                    }

                    if let Some((pane_id, payload)) = parse_tmux_output_notification(line) {
                        if pane_id == target_pane_id && !payload.is_empty() {
                            pane_dirty = true;
                        }
                        continue;
                    }

                    if let Some(rest) = line.strip_prefix("%window-pane-changed ") {
                        let mut parts = rest.split_whitespace();
                        if parts.next() == Some(target_window_id.as_str())
                            && let Some(new_pane_id) = parts.next()
                        {
                            target_pane_id = new_pane_id.to_string();
                            pane_dirty = true;
                        }
                        continue;
                    }

                    // The window was resized (by us or by another client).
                    if let Some(rest) = line.strip_prefix("%layout-change ") {
                        if rest.split_whitespace().next() == Some(target_window_id.as_str()) {
                            pane_dirty = true;
                        }
                        continue;
                    }

                    if let Some(rest) = line.strip_prefix("%pane-mode-changed ")
                        && rest.trim() == target_pane_id
                    {
                        pane_dirty = true;
                        continue;
                    }

                    if let Some(rest) = line.strip_prefix("%pause ") {
                        let paused_pane = rest.trim();
                        if paused_pane == target_pane_id {
                            let _ = client.send_command(&format!(
                                "refresh-client -A {paused_pane}:continue\n"
                            ));
                            pane_dirty = true;
                        }
                        continue;
                    }
                }
            }

            if pane_dirty {
                let now = Instant::now();
                if now < burst_until
                    || now.duration_since(last_snapshot) >= VIEW_CONTROL_SNAPSHOT_MIN_INTERVAL
                {
                    tx.send(reseed_control_view_parser(
                        session,
                        window,
                        cols,
                        rows,
                        &mut parser,
                    ));
                    pane_dirty = false;
                    last_snapshot = Instant::now();
                }
            } else if auto_refresh
                && Instant::now().duration_since(last_snapshot) >= VIEW_CONTROL_HEAL_INTERVAL
            {
                // Unconditional self-heal: the change-notifier can miss a
                // redraw, so re-capture the full pane on a steady floor to
                // keep a stale frame from persisting until the 3s drift
                // reseed. See VIEW_CONTROL_HEAL_INTERVAL.
                tx.send(reseed_control_view_parser(
                    session,
                    window,
                    cols,
                    rows,
                    &mut parser,
                ));
                last_snapshot = Instant::now();
            }
        }

        let _ = client.child.kill();
        Ok(())
    }

    /// Primary worker: uses pipe-pane to stream output and a persistent
    /// vt100 parser for incremental screen updates. No capture-pane
    /// subprocess is spawned after the initial seed.
    /// Primary worker: uses pipe-pane as a change-notification mechanism.
    /// When data arrives on the pipe, we know the pane has new output and
    /// run capture-pane to get the current screen state. When nothing
    /// arrives, we skip capture-pane entirely (zero subprocess overhead
    /// while idle). This avoids the persistent-parser state-mismatch
    /// problems while still eliminating polling when the pane is quiet.
    #[allow(clippy::too_many_arguments)]
    fn run_pipe_pane_worker(
        session: &str,
        window: &str,
        cols: u16,
        rows: u16,
        stop: &AtomicBool,
        refresh: &AtomicU8,
        condvar: &(StdMutex<()>, StdCondvar),
        tx: &SnapshotSender,
    ) -> anyhow::Result<()> {
        use std::fs;
        use std::io::Read as _;
        use std::os::unix::fs::OpenOptionsExt;

        // Slug the session name for a safe FIFO path.
        let slug: String = session
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let fifo_dir = std::path::PathBuf::from("/tmp/amf-pipes");
        fs::create_dir_all(&fifo_dir)?;
        let fifo_path = fifo_dir.join(format!("{slug}-{window}.pipe"));

        // Clean up any stale FIFO, then create a fresh one.
        let _ = fs::remove_file(&fifo_path);
        let c_path = std::ffi::CString::new(fifo_path.to_string_lossy().as_bytes())?;
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        if rc != 0 {
            anyhow::bail!(
                "mkfifo failed for {}: {}",
                fifo_path.display(),
                std::io::Error::last_os_error()
            );
        }

        // Start pipe-pane: tmux streams pane output into our FIFO.
        TmuxManager::start_pipe_pane(session, window, &fifo_path)?;

        // Open FIFO with O_RDWR | O_NONBLOCK. O_RDWR keeps the FIFO open
        // (no EOF) even before the pipe-pane writer connects, because
        // we act as both reader and writer. O_NONBLOCK makes reads
        // return WouldBlock instead of blocking when no data is ready.
        let fifo_fd = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo_path)?;
        let mut fifo = std::io::BufReader::with_capacity(64 * 1024, fifo_fd);

        let mut read_buf = [0u8; 16384];
        let mut next_cursor_refresh = Instant::now() - VIEW_CURSOR_REFRESH_INTERVAL;
        let mut last_capture = Instant::now() - VIEW_PANE_REFRESH_INTERVAL;
        let mut pane_has_new_output = true; // Capture immediately on start.
        let mut burst_until = Instant::now();

        crate::debug::log_to_file(
            crate::debug::LogLevel::Info,
            "perf",
            &format!("pipe-pane worker started for {session}:{window}"),
        );

        while !stop.load(Ordering::Relaxed) {
            let read_started = Instant::now();

            // Drain all available data from the FIFO (non-blocking).
            // We don't parse it — we only care that *something* changed.
            loop {
                match fifo.read(&mut read_buf) {
                    Ok(0) => {
                        // With O_RDWR this shouldn't happen, but handle it.
                        break;
                    }
                    Ok(_) => {
                        pane_has_new_output = true;
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(e) => {
                        let _ = TmuxManager::stop_pipe_pane(session, window);
                        let _ = fs::remove_file(&fifo_path);
                        return Err(e.into());
                    }
                }
            }

            let pipe_read_duration = if pane_has_new_output {
                Some(read_started.elapsed())
            } else {
                None
            };

            let now = Instant::now();
            let refresh_kind = refresh.swap(VIEW_SNAPSHOT_REFRESH_NONE, Ordering::Relaxed);
            let refresh_requested = refresh_kind != VIEW_SNAPSHOT_REFRESH_NONE;
            let pane_refresh_requested = refresh_requested;
            if refresh_requested {
                pane_has_new_output = true;
            }
            let cursor_refresh_requested = matches!(
                refresh_kind,
                VIEW_SNAPSHOT_REFRESH_NORMAL | VIEW_SNAPSHOT_REFRESH_BURST
            );
            if matches!(
                refresh_kind,
                VIEW_SNAPSHOT_REFRESH_BURST | VIEW_SNAPSHOT_REFRESH_PANE_BURST
            ) {
                burst_until = now + VIEW_BURST_DURATION;
            }
            let burst_active = now < burst_until;

            // Throttle captures: use burst interval after input,
            // normal interval otherwise.
            let pane_interval = if burst_active {
                VIEW_BURST_PANE_REFRESH_INTERVAL
            } else {
                VIEW_PANE_REFRESH_INTERVAL
            };
            let pane_due = pane_refresh_requested
                || (pane_has_new_output && now.duration_since(last_capture) >= pane_interval);

            let cursor_interval = if burst_active {
                VIEW_BURST_CURSOR_REFRESH_INTERVAL
            } else {
                VIEW_CURSOR_REFRESH_INTERVAL
            };
            // Only poll the cursor (a `display-message` subprocess) when a
            // pane capture happens anyway — cursor and content move
            // together, and an idle pane must spawn no subprocesses.
            let cursor_due = cursor_refresh_requested
                || (pane_due && now.duration_since(next_cursor_refresh) >= cursor_interval);

            if pane_due || cursor_due {
                let mut pane_content = None;
                let mut rendered_lines = None;
                let mut capture_duration = None;
                let mut render_duration = None;
                let mut cursor = None;
                let mut cursor_duration = None;

                if pane_due {
                    pane_has_new_output = false;
                    let started_at = Instant::now();
                    let captured =
                        TmuxManager::capture_pane_ansi(session, window).unwrap_or_default();
                    capture_duration = Some(started_at.elapsed());
                    let render_started = Instant::now();
                    rendered_lines = Some(render_ansi_lines(&captured, cols, rows));
                    render_duration = Some(render_started.elapsed());
                    pane_content = Some(captured);
                    last_capture = Instant::now();
                }

                if cursor_due {
                    let cursor_started = Instant::now();
                    cursor = Some(TmuxManager::cursor_position(session, window).ok());
                    cursor_duration = Some(cursor_started.elapsed());
                    next_cursor_refresh = Instant::now();
                }

                tx.send(ViewSnapshot {
                    session: session.to_string(),
                    window: window.to_string(),
                    pane_content,
                    rendered_lines,
                    cursor,
                    capture_duration,
                    render_duration,
                    cursor_duration,
                    pipe_read_duration,
                });
                continue;
            }

            // No work to do — wait for wake or timeout.
            let guard = condvar.0.lock().unwrap();
            let _ = condvar.1.wait_timeout(guard, Duration::from_millis(10));
        }

        // Cleanup.
        let _ = TmuxManager::stop_pipe_pane(session, window);
        let _ = fs::remove_file(&fifo_path);
        Ok(())
    }

    pub fn ensure_view_snapshot_worker(&mut self) {
        let Some((session, window, cols, rows)) = self.current_view_snapshot_target() else {
            self.stop_view_snapshot_worker();
            return;
        };

        if self.view_snapshot_target.as_ref()
            == Some(&(session.clone(), window.clone(), cols, rows))
        {
            self.sync_view_snapshot_content_flag();
            return;
        }

        self.stop_view_snapshot_worker();
        self.view_display_frozen_until = None;
        self.drain_view_snapshots();
        self.pane_lines.clear();

        let stop = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicU8::new(VIEW_SNAPSHOT_REFRESH_NORMAL));
        let include_content = Arc::new(AtomicBool::new(true));
        let condvar = Arc::new((StdMutex::new(()), StdCondvar::new()));
        let tx = self.view_snapshot_tx.clone();
        let worker_session = session.clone();
        let worker_window = window.clone();
        let worker_cols = cols;
        let worker_rows = rows;
        let worker_stop = stop.clone();
        let worker_refresh = refresh.clone();
        let worker_include_content = include_content.clone();
        let worker_condvar = condvar.clone();
        let worker_auto_refresh = self.config.view_auto_refresh;

        std::thread::spawn(move || {
            if TmuxManager::uses_control_pty_input() {
                let control_result = Self::run_control_mode_view_worker(
                    &worker_session,
                    &worker_window,
                    worker_cols,
                    worker_rows,
                    &worker_stop,
                    &worker_refresh,
                    &worker_include_content,
                    worker_auto_refresh,
                    &worker_condvar,
                    &tx,
                );

                if let Err(e) = &control_result {
                    crate::debug::log_to_file(
                        crate::debug::LogLevel::Warn,
                        "perf",
                        &format!(
                            "control-mode view failed ({e}), falling back to pipe-pane snapshots"
                        ),
                    );
                }

                if control_result.is_ok() || worker_stop.load(Ordering::Relaxed) {
                    return;
                }
            }

            // Try pipe-pane approach first, fall back to polling capture-pane.
            let pipe_result = Self::run_pipe_pane_worker(
                &worker_session,
                &worker_window,
                worker_cols,
                worker_rows,
                &worker_stop,
                &worker_refresh,
                &worker_condvar,
                &tx,
            );

            if let Err(e) = &pipe_result {
                crate::debug::log_to_file(
                    crate::debug::LogLevel::Warn,
                    "perf",
                    &format!("pipe-pane failed ({e}), falling back to capture-pane polling"),
                );
            }

            // Fallback: original polling approach if pipe-pane failed
            // or if it exited early (e.g. FIFO issue).
            if pipe_result.is_err() && !worker_stop.load(Ordering::Relaxed) {
                Self::run_capture_pane_worker(
                    &worker_session,
                    &worker_window,
                    worker_cols,
                    worker_rows,
                    &worker_stop,
                    &worker_refresh,
                    &worker_condvar,
                    &tx,
                );
            }
        });

        self.view_snapshot_stop = Some(stop);
        self.view_snapshot_refresh = Some(refresh);
        self.view_snapshot_include_content = Some(include_content);
        self.view_snapshot_condvar = Some(condvar);
        self.view_snapshot_target = Some((session, window, cols, rows));
        self.sync_view_snapshot_content_flag();
    }

    /// Freeze the displayed pane on its current frame for `dur`. Incoming
    /// snapshots keep merging in the latest-wins mailbox but are not
    /// applied until the freeze lapses, at which point the freshest frame
    /// is revealed. Used to hide the re-anchor bounce.
    pub fn freeze_view_display(&mut self, dur: Duration) {
        self.view_display_frozen_until = Some(Instant::now() + dur);
    }

    fn update_startup_mask_for_pane_content(&mut self) {
        let AppMode::Viewing(view) = &mut self.mode else {
            return;
        };
        if view.startup_mask_started_at.is_none() {
            return;
        }

        let launch_echo_visible = self.pane_content.contains("AMF_TMUX_BIN=")
            || self.pane_content.contains("AMF_TMUX_SOCKET=")
            || self.pane_content.contains("AMF_TMUX_WINDOW=")
            || self.pane_content.contains("AMF_FEATURE_SESSION_ID=")
            || self.pane_content.contains("AMF_SESSION_ID=")
            || self.pane_content.contains("AMF_SESSION=");
        if !view.startup_mask_active()
            || (!self.pane_content.trim().is_empty() && !launch_echo_visible)
        {
            view.startup_mask_started_at = None;
        }
    }

    pub fn drain_view_snapshots(&mut self) -> (bool, bool) {
        // Hold the last good frame while a re-anchor bounce is in flight
        // so the shrink/restore and Claude Code's repaint stay off-screen.
        if let Some(until) = self.view_display_frozen_until {
            if Instant::now() < until {
                return (false, false);
            }
            self.view_display_frozen_until = None;
        }

        let current_target = self.current_view_snapshot_target();
        let mut pane_changed = false;
        let mut cursor_changed = false;

        // Latest-wins mailbox: at most one (merged) snapshot is pending.
        let pending = self.view_snapshot_tx.slot.lock().unwrap().take();
        if let Some(snapshot) = pending {
            if current_target.as_ref()
                != Some(&(
                    snapshot.session.clone(),
                    snapshot.window.clone(),
                    self.pane_content_cols,
                    self.pane_content_rows,
                ))
            {
                return (false, false);
            }

            if let Some(duration) = snapshot.capture_duration {
                self.perf
                    .record_duration("view.capture_pane_ansi", duration);
            }
            if let Some(duration) = snapshot.render_duration {
                self.perf
                    .record_duration("view.render_snapshot_lines", duration);
            }
            if let Some(duration) = snapshot.cursor_duration {
                self.perf.record_duration("view.cursor_position", duration);
            }
            if let Some(duration) = snapshot.pipe_read_duration {
                self.perf.record_duration("view.pipe_read", duration);
            }

            if let Some(pane_content) = snapshot.pane_content
                && self.pane_content != pane_content
            {
                self.pane_content = pane_content;
                self.update_startup_mask_for_pane_content();
                pane_changed = true;
            }

            if let Some(rendered_lines) = snapshot.rendered_lines
                && self.pane_lines != rendered_lines
            {
                self.pane_lines = rendered_lines;
                pane_changed = true;
            }

            if let Some(cursor) = snapshot.cursor
                && self.tmux_cursor != cursor
            {
                self.tmux_cursor = cursor;
                cursor_changed = true;
            }
        }

        (pane_changed, cursor_changed)
    }

    pub fn new(db_path: PathBuf) -> Result<Self> {
        setup::ensure_notify_scripts();
        let db = crate::db::AmfDb::open_or_seed(&db_path, &crate::project::global_db_path())?;
        let store = db.load_store()?;
        let (sidebar_load_tx, sidebar_load_rx) = std::sync::mpsc::channel();
        // These caches are populated by the background sidebar-load tasks
        // scheduled in startup task 7 (schedule_sidebar_loads_for_all_features).
        // Building them synchronously here required reading every Claude JSONL
        // and PLAN.md file before the first frame could draw, which caused the
        // "Loading AMF..." stall proportional to feature count.
        let latest_prompt_cache = HashMap::new();
        let config = load_config();
        let zai_enabled = config.zai.is_some();
        let zai_monthly = config.zai.as_ref().and_then(|z| z.get_monthly_limit());
        let zai_weekly = config.zai.as_ref().and_then(|z| z.get_weekly_limit());
        let zai_five_hour = config.zai.as_ref().and_then(|z| z.get_five_hour_limit());
        let global_ext = load_global_extension_config();
        let active_extension = store
            .projects
            .first()
            .map(|p| merge_project_extension_config(&global_ext, &p.repo))
            .unwrap_or(global_ext);
        let sidebar_plan_cache = HashMap::new();
        let (codex_sidebar_metadata_tx, codex_sidebar_metadata_rx) = std::sync::mpsc::channel();
        let view_snapshot_slot = Arc::new(StdMutex::new(None));
        let (wakeup_rx, wakeup_tx) = unsafe {
            let mut fds = [0i32; 2];
            libc::pipe(fds.as_mut_ptr());
            libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
            // Non-blocking write end too: a wakeup byte lost to a full
            // pipe is harmless (the pipe being full already wakes poll),
            // but a blocking write would stall the sending thread.
            libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK);
            (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
        };
        let view_snapshot_tx = SnapshotSender {
            slot: view_snapshot_slot,
            wakeup_fd: Arc::new(wakeup_tx),
        };
        let (harness_check_tx, harness_check_rx) = channel();
        let mut theme = crate::theme::Theme::load(&config.theme);
        theme.set_transparent(config.transparent_background);
        let store_path = db.path.clone();
        let mut app = Self {
            store,
            store_path,
            db: Some(db),
            config,
            active_extension,
            theme,
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            message: None,
            toasts: Vec::new(),
            should_quit: false,
            pane_content: String::new(),
            pane_lines: Vec::new(),
            pane_content_cols: 0,
            pane_content_rows: 0,
            viewport_cols: 0,
            viewport_rows: 0,
            viewport_total_rows: 0,
            tmux_cursor: None,
            leader_active: false,
            leader_activated_at: None,
            last_view_activity_at: None,
            view_input_batch: None,
            pending_inputs: Vec::new(),
            compose_direct_targets: std::collections::HashSet::new(),
            compose_drafts: HashMap::new(),
            compose_clipboard_paste: None,
            next_compose_clipboard_paste_id: 1,
            latest_prompt_cache,
            sidebar_model_cache: HashMap::new(),
            sidebar_plan_cache,
            codex_session_title_cache: HashMap::new(),
            codex_session_prompt_cache: HashMap::new(),
            codex_session_model_cache: HashMap::new(),
            codex_live_threads: HashMap::new(),
            codex_sidebar_metadata_tx,
            codex_sidebar_metadata_rx,
            codex_sidebar_metadata_inflight: std::collections::HashSet::new(),
            opencode_sidebar_cache: HashMap::new(),
            sidebar_load_tx,
            sidebar_load_rx,
            sidebar_load_executor: None,
            sidebar_load_signatures: HashMap::new(),
            pending_sidebar_loads: std::collections::HashSet::new(),
            usage: UsageManager::new(zai_enabled, zai_monthly, zai_weekly, zai_five_hour),
            token_tracker: SessionTokenTracker::default(),
            session_status_bg: None,
            pr_review_bg: None,
            pr_review_return: None,
            review_memory_bootstrap_bg: None,
            ai_review_bg: None,
            scroll_offset: 0,
            session_filter: SessionFilter::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            thinking_features: std::collections::HashSet::new(),
            opencode_thinking_pane_cache: HashMap::new(),
            ipc_thinking_sessions: std::collections::HashSet::new(),
            ipc_tool_sessions: std::collections::HashSet::new(),
            summary_state: SummaryState::new(),
            summary_rx: None,
            tmux: Box::new(TmuxManager),
            worktree: Box::new(WorktreeManager),
            debug_log: DebugLog::default(),
            pending_debug_log_entries: VecDeque::new(),
            last_debug_log_flush_at: Instant::now(),
            perf: PerfCollector::new(),
            background_deletions: HashMap::new(),
            background_hooks: HashMap::new(),
            ipc: None,
            fs_watcher: None,
            tmux_observer: None,
            ipc_chatty_log_count: 0,
            ipc_chatty_log_window_start: Instant::now(),
            last_file_notification_count: 0,
            last_file_notification_fingerprint: None,
            // Checked asynchronously — defaults to false until confirmed.
            vscode_available: false,
            view_snapshot_tx,
            view_snapshot_wakeup_rx: Some(wakeup_rx),
            view_snapshot_stop: None,
            view_snapshot_refresh: None,
            view_snapshot_include_content: None,
            view_snapshot_condvar: None,
            view_snapshot_target: None,
            view_display_frozen_until: None,
            harness_check_tx,
            harness_check_rx,
        };

        match crate::fswatch::FsWatcher::start(app.view_wakeup_tx()) {
            Ok(watcher) => app.fs_watcher = Some(watcher),
            Err(e) => {
                crate::debug::log_to_file(
                    crate::debug::LogLevel::Warn,
                    "fswatch",
                    &format!("watcher unavailable, falling back to scan timers: {e}"),
                );
            }
        }
        app.refresh_fs_watch_paths();
        // A version-mismatched -C client hangs without delivering any
        // events; leaving the observer unset keeps the faster 5s
        // status-sync polling instead of trusting a dead event stream.
        if TmuxManager::control_clients_compatible() {
            app.tmux_observer = Some(crate::tmux_observer::TmuxObserver::start(
                app.view_wakeup_tx(),
            ));
        } else {
            crate::debug::log_to_file(
                crate::debug::LogLevel::Warn,
                "tmux",
                "tmux observer disabled (client/server version mismatch); using status polling",
            );
        }

        // Seed in-memory caches from DB so first use after restart is fast.
        if let Some(ref db) = app.db {
            let _ = db.evict_stale_token_cache();
            let _ = db.evict_stale_pr_review_cache();
            let _ = db.evict_stale_pr_comment_triage();
            if let Ok(entries) = db.load_token_cache() {
                app.token_tracker.seed_from_db_cache(entries);
            }
            if let Ok(entries) = db.load_recent_log(app.debug_log.max_entries()) {
                app.debug_log.inject_entries(entries);
            }
        }

        for message in crate::highlight::validate_startup_parsers() {
            match message.level {
                crate::highlight::StartupValidationLevel::Info => {
                    app.log_info("syntax", message.message);
                }
                crate::highlight::StartupValidationLevel::Warn => {
                    app.log_warn("syntax", message.message);
                }
                crate::highlight::StartupValidationLevel::Error => {
                    app.log_error("syntax", message.message);
                }
            }
        }
        crate::highlight::reload_runtime_state();

        Ok(app)
    }

    pub fn log_startup(&mut self) {
        self.log_info("amf", "AMF started".to_string());
        self.log_debug("amf", format!("DB path: {}", self.store_path.display()));
        self.log_debug(
            "amf",
            format!("Projects loaded: {}", self.store.projects.len()),
        );
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
        let (sidebar_load_tx, sidebar_load_rx) = std::sync::mpsc::channel();
        let latest_prompt_cache = Self::build_latest_prompt_cache(&store);
        let sidebar_plan_cache = Self::build_sidebar_plan_cache(&store);
        let (codex_sidebar_metadata_tx, codex_sidebar_metadata_rx) = std::sync::mpsc::channel();
        let view_snapshot_slot = Arc::new(StdMutex::new(None));
        let (wakeup_rx, wakeup_tx) = unsafe {
            let mut fds = [0i32; 2];
            libc::pipe(fds.as_mut_ptr());
            libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
            // Non-blocking write end too: a wakeup byte lost to a full
            // pipe is harmless (the pipe being full already wakes poll),
            // but a blocking write would stall the sending thread.
            libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK);
            (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
        };
        let view_snapshot_tx = SnapshotSender {
            slot: view_snapshot_slot,
            wakeup_fd: Arc::new(wakeup_tx),
        };
        let (harness_check_tx, harness_check_rx) = channel();
        Self {
            store,
            store_path: PathBuf::new(),
            db: None,
            config: AppConfig::default(),
            active_extension: ExtensionConfig::default(),
            theme: crate::theme::Theme::default(),
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            message: None,
            toasts: Vec::new(),
            should_quit: false,
            pane_content: String::new(),
            pane_lines: Vec::new(),
            pane_content_cols: 0,
            pane_content_rows: 0,
            viewport_cols: 0,
            viewport_rows: 0,
            viewport_total_rows: 0,
            tmux_cursor: None,
            leader_active: false,
            leader_activated_at: None,
            last_view_activity_at: None,
            view_input_batch: None,
            pending_inputs: Vec::new(),
            compose_direct_targets: std::collections::HashSet::new(),
            compose_drafts: HashMap::new(),
            compose_clipboard_paste: None,
            next_compose_clipboard_paste_id: 1,
            latest_prompt_cache,
            sidebar_model_cache: HashMap::new(),
            sidebar_plan_cache,
            codex_session_title_cache: HashMap::new(),
            codex_session_prompt_cache: HashMap::new(),
            codex_session_model_cache: HashMap::new(),
            codex_live_threads: HashMap::new(),
            codex_sidebar_metadata_tx,
            codex_sidebar_metadata_rx,
            codex_sidebar_metadata_inflight: std::collections::HashSet::new(),
            opencode_sidebar_cache: HashMap::new(),
            sidebar_load_tx,
            sidebar_load_rx,
            sidebar_load_executor: None,
            sidebar_load_signatures: HashMap::new(),
            pending_sidebar_loads: std::collections::HashSet::new(),
            usage: UsageManager::new(false, None, None, None),
            token_tracker: SessionTokenTracker::default(),
            session_status_bg: None,
            pr_review_bg: None,
            pr_review_return: None,
            review_memory_bootstrap_bg: None,
            ai_review_bg: None,
            scroll_offset: 0,
            session_filter: SessionFilter::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            thinking_features: std::collections::HashSet::new(),
            opencode_thinking_pane_cache: HashMap::new(),
            ipc_thinking_sessions: std::collections::HashSet::new(),
            ipc_tool_sessions: std::collections::HashSet::new(),
            summary_state: SummaryState::new(),
            summary_rx: None,
            tmux,
            worktree,
            debug_log: DebugLog::default(),
            pending_debug_log_entries: VecDeque::new(),
            last_debug_log_flush_at: Instant::now(),
            perf: PerfCollector::new(),
            background_deletions: HashMap::new(),
            background_hooks: HashMap::new(),
            ipc: None,
            fs_watcher: None,
            tmux_observer: None,
            ipc_chatty_log_count: 0,
            ipc_chatty_log_window_start: Instant::now(),
            last_file_notification_count: 0,
            last_file_notification_fingerprint: None,
            vscode_available: false,
            view_snapshot_tx,
            view_snapshot_wakeup_rx: Some(wakeup_rx),
            view_snapshot_stop: None,
            view_snapshot_refresh: None,
            view_snapshot_include_content: None,
            view_snapshot_condvar: None,
            view_snapshot_target: None,
            view_display_frozen_until: None,
            harness_check_tx,
            harness_check_rx,
        }
    }

    pub(crate) fn has_active_sidebar(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::Viewing(view) if view.sidebar_session_kind().is_some()
        )
    }

    pub(crate) fn refresh_sidebar_for_current_view(&mut self) {
        let Some((project_name, feature_name, window, session_kind)) = (match &self.mode {
            AppMode::Viewing(view) => view.sidebar_session_kind().map(|session_kind| {
                (
                    view.project_name.clone(),
                    view.feature_name.clone(),
                    view.window.clone(),
                    session_kind,
                )
            }),
            _ => None,
        }) else {
            return;
        };

        let Some((pi, fi)) = self
            .store
            .projects
            .iter()
            .enumerate()
            .find(|(_, project)| project.name == project_name)
            .and_then(|(pi, project)| {
                project
                    .features
                    .iter()
                    .enumerate()
                    .find(|(_, feature)| feature.name == feature_name)
                    .map(|(fi, _)| (pi, fi))
            })
        else {
            return;
        };

        self.refresh_latest_prompt_for_feature(pi, fi);
        self.refresh_sidebar_plan_for_feature(pi, fi);
        self.request_codex_sidebar_metadata_for_view(
            &project_name,
            &feature_name,
            &window,
            &session_kind,
        );
        self.schedule_sidebar_load_for_feature(pi, fi);
    }

    #[allow(dead_code)] // exercised only by unit tests
    fn build_latest_prompt_cache(store: &ProjectStore) -> HashMap<String, String> {
        let mut cache = HashMap::new();

        for project in &store.projects {
            for feature in &project.features {
                if let Some(prompt) = latest_prompt_text_for_feature(feature)
                    .map(|prompt| prompt.trim().to_string())
                    .filter(|prompt| !prompt.is_empty())
                {
                    cache.insert(feature.tmux_session.clone(), prompt);
                }
            }
        }

        cache
    }

    #[allow(dead_code)] // exercised only by unit tests
    fn build_sidebar_plan_cache(store: &ProjectStore) -> HashMap<String, String> {
        let mut cache = HashMap::new();

        for project in &store.projects {
            for feature in &project.features {
                if let Some(plan) =
                    crate::markdown::read_plan_preview(&feature.workdir, Some(&project.repo))
                {
                    cache.insert(feature.tmux_session.clone(), plan);
                }
            }
        }

        cache
    }

    pub(crate) fn schedule_sidebar_load_for_feature(&mut self, pi: usize, fi: usize) {
        let Some(feature) = self
            .store
            .projects
            .get(pi)
            .and_then(|project| project.features.get(fi))
        else {
            return;
        };

        let request = SidebarLoadRequest::from_feature(feature);
        if !self
            .pending_sidebar_loads
            .insert(request.tmux_session.clone())
        {
            return;
        }

        let previous_signature = self
            .sidebar_load_signatures
            .get(&request.tmux_session)
            .copied();
        let tx = self.sidebar_load_tx.clone();
        self.sidebar_load_executor
            .get_or_insert_with(SidebarLoadExecutor::new)
            .submit(Box::new(move || {
                let _ = tx.send(request.load(previous_signature));
            }));
    }

    pub(crate) fn schedule_sidebar_loads_for_all_features(&mut self) {
        let mut targets = Vec::new();
        for (pi, project) in self.store.projects.iter().enumerate() {
            for (fi, _feature) in project.features.iter().enumerate() {
                targets.push((pi, fi));
            }
        }
        for (pi, fi) in targets {
            self.schedule_sidebar_load_for_feature(pi, fi);
        }
    }

    pub(crate) fn schedule_sidebar_loads_for_polling_fallback(&mut self) {
        if self.ipc.is_none() {
            self.schedule_sidebar_loads_for_all_features();
        }
    }

    pub(crate) fn poll_sidebar_load_results(&mut self) -> bool {
        let mut changed = false;
        while let Ok(result) = self.sidebar_load_rx.try_recv() {
            self.pending_sidebar_loads.remove(&result.tmux_session);
            self.sidebar_load_signatures
                .insert(result.tmux_session.clone(), result.signature);

            if !result.changed {
                continue;
            }

            changed = true;

            if let Some(prompt) = result.latest_prompt {
                let prompt = prompt.trim().to_string();
                if prompt.is_empty() {
                    self.latest_prompt_cache.remove(&result.tmux_session);
                } else {
                    self.latest_prompt_cache
                        .insert(result.tmux_session.clone(), prompt);
                }
            } else {
                self.latest_prompt_cache.remove(&result.tmux_session);
            }

            if let Some(model_text) = result.model_text {
                let model_text = model_text.trim().to_string();
                if model_text.is_empty() {
                    self.sidebar_model_cache.remove(&result.tmux_session);
                } else {
                    self.sidebar_model_cache
                        .insert(result.tmux_session.clone(), model_text);
                }
            } else {
                self.sidebar_model_cache.remove(&result.tmux_session);
            }

            if let Some(data) = result.opencode_sidebar {
                self.opencode_sidebar_cache
                    .insert(result.tmux_session, data);
            } else {
                self.opencode_sidebar_cache.remove(&result.tmux_session);
            }
        }
        changed
    }

    pub(crate) fn clear_sidebar_state_for_session(&mut self, tmux_session: &str) {
        self.pending_sidebar_loads.remove(tmux_session);
        self.sidebar_load_signatures.remove(tmux_session);
        self.latest_prompt_cache.remove(tmux_session);
        self.sidebar_model_cache.remove(tmux_session);
        self.sidebar_plan_cache.remove(tmux_session);
        self.codex_live_threads.remove(tmux_session);
        self.opencode_sidebar_cache.remove(tmux_session);
        self.prune_codex_sidebar_caches();
    }

    /// Drop codex sidebar metadata cache entries that no longer belong to
    /// any existing feature. These caches are keyed by `workdir::session_id`
    /// (not the tmux session), so they cannot be removed alongside the
    /// other per-session state above; without this GC they accumulate as
    /// features are deleted and codex sessions are resumed under new ids.
    fn prune_codex_sidebar_caches(&mut self) {
        let valid_prefixes: Vec<String> = self
            .store
            .projects
            .iter()
            .flat_map(|project| project.features.iter())
            .map(|feature| format!("{}::", feature.workdir.display()))
            .collect();

        let keep = |key: &String| {
            valid_prefixes
                .iter()
                .any(|prefix| key.starts_with(prefix.as_str()))
        };

        self.codex_session_title_cache.retain(|key, _| keep(key));
        self.codex_session_prompt_cache.retain(|key, _| keep(key));
        self.codex_session_model_cache.retain(|key, _| keep(key));
        self.codex_sidebar_metadata_inflight.retain(|key| keep(key));
    }

    pub(crate) fn refresh_latest_prompt_for_feature(&mut self, pi: usize, fi: usize) {
        let Some(feature) = self
            .store
            .projects
            .get(pi)
            .and_then(|project| project.features.get(fi))
        else {
            return;
        };

        if let Some(prompt) = latest_prompt_text_for_feature(feature)
            .map(|prompt| prompt.trim().to_string())
            .filter(|prompt| !prompt.is_empty())
        {
            self.latest_prompt_cache
                .insert(feature.tmux_session.clone(), prompt);
        } else {
            self.latest_prompt_cache.remove(&feature.tmux_session);
        }
    }

    pub(crate) fn refresh_sidebar_plan_for_feature(&mut self, pi: usize, fi: usize) {
        let Some((project, feature)) = self
            .store
            .projects
            .get(pi)
            .and_then(|project| project.features.get(fi).map(|feature| (project, feature)))
        else {
            return;
        };

        if let Some(plan) =
            crate::markdown::read_plan_preview(&feature.workdir, Some(&project.repo))
        {
            self.sidebar_plan_cache
                .insert(feature.tmux_session.clone(), plan);
        } else {
            self.sidebar_plan_cache.remove(&feature.tmux_session);
        }
    }

    pub fn latest_prompt_for_session(&self, tmux_session: &str) -> Option<&str> {
        self.latest_prompt_cache
            .get(tmux_session)
            .map(String::as_str)
    }

    #[allow(dead_code)] // exercised only by unit tests
    pub fn sidebar_plan_for_session(&self, tmux_session: &str) -> Option<&str> {
        self.sidebar_plan_cache
            .get(tmux_session)
            .map(String::as_str)
    }

    fn codex_sidebar_cache_key(workdir: &Path, session_id: &str) -> String {
        format!("{}::{session_id}", workdir.display())
    }

    pub(crate) fn request_codex_sidebar_metadata_for_session(
        &mut self,
        workdir: &Path,
        session_id: &str,
    ) {
        let cache_key = Self::codex_sidebar_cache_key(workdir, session_id);
        if self.codex_sidebar_metadata_inflight.contains(&cache_key)
            || (self.codex_session_title_cache.contains_key(&cache_key)
                && self.codex_session_prompt_cache.contains_key(&cache_key))
                && self.codex_session_model_cache.contains_key(&cache_key)
        {
            return;
        }

        self.codex_sidebar_metadata_inflight
            .insert(cache_key.clone());
        let tx = self.codex_sidebar_metadata_tx.clone();
        let workdir = workdir.to_path_buf();
        let session_id = session_id.to_string();
        std::thread::spawn(move || {
            let metadata = crate::app::codex_sidebar_metadata_for_session_id(&workdir, &session_id)
                .ok()
                .flatten();
            let result = CodexSidebarMetadataResult {
                cache_key,
                title: metadata
                    .as_ref()
                    .and_then(|metadata| metadata.title.clone()),
                prompt: metadata
                    .as_ref()
                    .and_then(|metadata| metadata.latest_prompt.clone()),
                model_text: metadata.and_then(|metadata| {
                    model_line(
                        metadata.model.as_deref(),
                        metadata.model_provider.as_deref(),
                    )
                }),
            };
            let _ = tx.send(result);
        });
    }

    pub(crate) fn request_codex_sidebar_metadata_for_view(
        &mut self,
        project_name: &str,
        feature_name: &str,
        window: &str,
        session_kind: &SessionKind,
    ) {
        if *session_kind != SessionKind::Codex {
            return;
        }

        let context = self
            .store
            .projects
            .iter()
            .find(|project| project.name == project_name)
            .and_then(|project| {
                project
                    .features
                    .iter()
                    .find(|feature| feature.name == feature_name)
            })
            .and_then(|feature| {
                feature
                    .sessions
                    .iter()
                    .find(|session| session.tmux_window == window)
                    .and_then(|session| {
                        session
                            .token_usage_source
                            .as_ref()
                            .filter(|source| {
                                source.provider == crate::token_tracking::TokenUsageProvider::Codex
                            })
                            .map(|source| (feature.workdir.clone(), source.id.clone()))
                    })
            });

        let Some((workdir, session_id)) = context else {
            return;
        };

        self.request_codex_sidebar_metadata_for_session(&workdir, &session_id);
    }

    #[allow(dead_code)] // exercised only by unit tests
    pub fn cached_codex_session_title(&self, workdir: &Path, session_id: &str) -> Option<&str> {
        let cache_key = Self::codex_sidebar_cache_key(workdir, session_id);
        self.codex_session_title_cache
            .get(&cache_key)
            .and_then(|title| title.as_deref())
    }

    pub fn cached_codex_session_prompt(&self, workdir: &Path, session_id: &str) -> Option<&str> {
        let cache_key = Self::codex_sidebar_cache_key(workdir, session_id);
        self.codex_session_prompt_cache
            .get(&cache_key)
            .and_then(|prompt| prompt.as_deref())
    }

    pub fn cached_codex_session_model(&self, workdir: &Path, session_id: &str) -> Option<&str> {
        let cache_key = Self::codex_sidebar_cache_key(workdir, session_id);
        self.codex_session_model_cache
            .get(&cache_key)
            .and_then(|model| model.as_deref())
    }

    pub fn codex_live_thread(&self, tmux_session: &str) -> Option<&CodexLiveThreadState> {
        self.codex_live_threads.get(tmux_session)
    }

    pub fn apply_codex_live_event(&mut self, tmux_session: &str, raw: &serde_json::Value) -> bool {
        let state = self
            .codex_live_threads
            .entry(tmux_session.to_string())
            .or_default();
        state.apply_event(raw)
    }

    /// Size session windows will have when shown in the embedded view:
    /// full terminal width by full height minus the 1-row view header.
    pub(crate) fn view_pane_viewport(&self) -> Option<(u16, u16)> {
        match (self.viewport_cols, self.viewport_total_rows) {
            (0, _) | (_, 0) => None,
            (cols, total_rows) => Some((cols, total_rows.saturating_sub(1))),
        }
    }

    pub(crate) fn resize_session_windows_for_viewport(
        tmux: &dyn TmuxOps,
        viewport: Option<(u16, u16)>,
        tmux_session: &str,
        windows: &[String],
    ) -> Result<()> {
        let Some((cols, rows)) = viewport else {
            return Ok(());
        };

        for window in windows {
            tmux.resize_pane(tmux_session, window, cols, rows)?;
        }

        Ok(())
    }

    /// Re-merge extension config for the currently selected
    /// project/feature. Call this whenever the selection changes.
    pub fn reload_extension_config(&mut self) {
        let global_ext = load_global_extension_config();
        self.active_extension = match &self.selection {
            Selection::Project(pi) => {
                if let Some(project) = self.store.projects.get(*pi) {
                    merge_project_extension_config(&global_ext, &project.repo)
                } else {
                    global_ext
                }
            }
            Selection::Feature(pi, fi) | Selection::Session(pi, fi, _) => {
                if let Some(project) = self.store.projects.get(*pi) {
                    if project.features.get(*fi).is_some() {
                        // Extension config is project-scoped and lives under
                        // `{repo}/.amf/config.json`, so worktree selections
                        // should still reload from the project repo.
                        merge_project_extension_config(&global_ext, &project.repo)
                    } else {
                        merge_project_extension_config(&global_ext, &project.repo)
                    }
                } else {
                    global_ext
                }
            }
        };
    }

    pub(crate) fn extension_for_repo(&self, repo: &Path) -> ExtensionConfig {
        let global_ext = load_global_extension_config();
        merge_project_extension_config(&global_ext, repo)
    }

    pub(crate) fn allowed_agents_for_repo(&self, repo: &Path) -> Vec<AgentKind> {
        let ext_allowed = self.extension_for_repo(repo).allowed_agents();
        if self.store.available_harnesses.is_empty() {
            ext_allowed
        } else {
            ext_allowed
                .into_iter()
                .filter(|a| self.store.available_harnesses.contains(a))
                .collect()
        }
    }

    pub(crate) fn repo_for_project_path(&self, path: &Path) -> PathBuf {
        self.worktree
            .repo_root(path)
            .unwrap_or_else(|_| path.to_path_buf())
    }

    pub(crate) fn allowed_agents_for_project_path(&self, path: &Path) -> Vec<AgentKind> {
        let repo = self.repo_for_project_path(path);
        self.allowed_agents_for_repo(&repo)
    }

    pub(crate) fn allows_agent_for_repo(&self, repo: &Path, agent: &AgentKind) -> bool {
        let allowed = self.allowed_agents_for_repo(repo);
        allowed.contains(agent)
    }

    pub(crate) fn normalize_agent_for_repo(
        &self,
        repo: &Path,
        preferred: &AgentKind,
    ) -> (AgentKind, usize) {
        let allowed = self.allowed_agents_for_repo(repo);
        let selected = allowed
            .iter()
            .find(|agent| *agent == preferred)
            .cloned()
            .unwrap_or_else(|| allowed[0].clone());
        let index = AgentKind::index_in(&allowed, &selected);
        (selected, index)
    }

    pub(crate) fn normalize_agent_for_project_path(
        &self,
        path: &Path,
        preferred: &AgentKind,
    ) -> (AgentKind, usize) {
        let repo = self.repo_for_project_path(path);
        self.normalize_agent_for_repo(&repo, preferred)
    }

    pub(crate) fn poll_harness_checks(&mut self) {
        while let Ok(result) = self.harness_check_rx.try_recv() {
            if let AppMode::HarnessSetup(state) = &mut self.mode
                && let Some(harness) = state.harnesses.iter_mut().find(|h| h.kind == result.kind)
            {
                match result.result {
                    Ok(()) => {
                        harness.status = HarnessCheckStatus::Installed;
                        harness.enabled = true;
                    }
                    Err(_) => {
                        let hint = Self::harness_install_hint(&result.kind);
                        harness.status = HarnessCheckStatus::NotFound(hint.to_string());
                    }
                }
            }
        }
    }

    pub(crate) fn open_harness_setup(&mut self, is_startup: bool) {
        let state =
            crate::app::state::HarnessSetupState::new(is_startup, &self.store.available_harnesses);
        self.mode = AppMode::HarnessSetup(state);
        self.message = None;
    }

    pub(crate) fn check_harness_available(kind: &AgentKind) -> Result<()> {
        match kind {
            AgentKind::Claude => crate::claude::ClaudeLauncher::check_available(),
            AgentKind::Codex => crate::codex::CodexLauncher::check_available(),
            AgentKind::Opencode => {
                let output = std::process::Command::new("opencode")
                    .arg("--version")
                    .output()
                    .context("opencode CLI not found - is Opencode installed?")?;
                if !output.status.success() {
                    anyhow::bail!("opencode CLI returned an error");
                }
                Ok(())
            }
            AgentKind::Pi => crate::pi::PiLauncher::check_available(),
        }
    }

    pub(crate) fn harness_install_hint(kind: &AgentKind) -> &'static str {
        match kind {
            AgentKind::Claude => "Install Claude Code: https://claude.ai/code",
            AgentKind::Codex => "Install Codex: npm install -g @openai/codex",
            AgentKind::Opencode => {
                "Install Opencode: go install github.com/opencode-ai/opencode@latest"
            }
            AgentKind::Pi => "Install Pi: check your provider's documentation",
        }
    }

    pub(crate) fn refresh_create_project_agent_selection(&mut self) {
        let (path, preferred) = match &self.mode {
            AppMode::CreatingProject(state) => (PathBuf::from(&state.path), state.agent.clone()),
            _ => return,
        };
        let (agent, agent_index) = self.normalize_agent_for_project_path(&path, &preferred);
        if let AppMode::CreatingProject(state) = &mut self.mode {
            state.agent = agent;
            state.agent_index = agent_index;
        }
    }

    pub(crate) fn default_project_preferred_agent(&self) -> AgentKind {
        self.config
            .projects
            .default_preferred_agent
            .clone()
            .unwrap_or_default()
    }

    pub(crate) fn use_custom_diff_review_viewer(&self) -> bool {
        matches!(self.config.diff_review_viewer, DiffReviewViewer::Amf)
    }

    pub(crate) fn ensure_agent_mode_supported(
        &self,
        agent: &AgentKind,
        mode: &VibeMode,
    ) -> Result<()> {
        if matches!(agent, AgentKind::Codex) && matches!(mode, VibeMode::Vibeless) {
            anyhow::bail!(
                "Codex does not support Vibeless diff review. Use Claude/Opencode, or switch to Vibe or SuperVibe."
            );
        }
        Ok(())
    }

    pub fn save(&self) -> Result<()> {
        if let Some(db) = &self.db {
            return db.save_store(&self.store);
        }
        // Fallback for tests: write JSON to store_path if set.
        if !self.store_path.as_os_str().is_empty() {
            return self.store.save(&self.store_path);
        }
        Ok(())
    }

    pub fn start_theme_picker(&mut self) {
        let themes = crate::theme::Theme::list();
        let selected = themes
            .iter()
            .position(|t| *t == self.config.theme)
            .unwrap_or(0);
        let original_theme = self.config.theme;
        self.mode = AppMode::ThemePicker(ThemePickerState {
            selected,
            themes,
            original_theme,
        });
    }

    pub fn preview_theme(&mut self, theme_name: crate::theme::ThemeName) {
        let mut theme = crate::theme::Theme::load(&theme_name);
        theme.set_transparent(self.config.transparent_background);
        self.theme = theme;
    }

    pub fn apply_theme(&mut self, theme_name: crate::theme::ThemeName) {
        self.config.theme = theme_name;
        let mut theme = crate::theme::Theme::load(&self.config.theme);
        theme.set_transparent(self.config.transparent_background);
        self.theme = theme;
        self.mode = AppMode::Normal;
        self.save_config();
    }

    pub fn toggle_transparent_background(&mut self) {
        self.config.transparent_background = !self.config.transparent_background;
        self.theme
            .set_transparent(self.config.transparent_background);
        self.save_config();
    }

    pub fn log_debug(&mut self, context: &str, message: String) {
        let entry = self.debug_log.debug(context, message);
        if self.db.is_some() {
            self.pending_debug_log_entries.push_back(entry);
        }
    }

    pub fn log_info(&mut self, context: &str, message: String) {
        let entry = self.debug_log.info(context, message);
        if self.db.is_some() {
            self.pending_debug_log_entries.push_back(entry);
        }
    }

    pub fn log_warn(&mut self, context: &str, message: String) {
        let entry = self.debug_log.warn(context, message);
        if self.db.is_some() {
            self.pending_debug_log_entries.push_back(entry);
        }
    }

    pub fn log_error(&mut self, context: &str, message: String) {
        let entry = self.debug_log.error(context, message);
        if self.db.is_some() {
            self.pending_debug_log_entries.push_back(entry);
        }
    }

    pub fn flush_pending_debug_log_entries(&mut self) {
        let Some(db) = &self.db else {
            self.pending_debug_log_entries.clear();
            return;
        };

        if self.pending_debug_log_entries.is_empty() {
            return;
        }

        let mut remaining = VecDeque::new();
        while let Some(entry) = self.pending_debug_log_entries.pop_front() {
            if db.append_log_entry(&entry).is_err() {
                remaining.push_back(entry);
                remaining.extend(self.pending_debug_log_entries.drain(..));
                break;
            }
        }

        self.pending_debug_log_entries = remaining;
        self.last_debug_log_flush_at = Instant::now();
    }

    pub fn should_flush_pending_debug_log_entries(&self) -> bool {
        !self.pending_debug_log_entries.is_empty()
            && (self.last_debug_log_flush_at.elapsed() >= Duration::from_secs(1)
                || self.pending_debug_log_entries.len() >= 32)
    }

    pub fn report_logged_error(&mut self, context: &str, detail: impl Into<String>) {
        let detail = detail.into();
        self.log_error(context, detail.clone());
        self.set_debug_log_error_message(detail);
    }

    pub fn set_debug_log_error_message(&mut self, message: impl Into<String>) {
        self.message = Some(format!(
            "Error: {} Check debug log for details.",
            message.into()
        ));
    }

    pub fn flush_token_cache_to_db(&mut self) {
        if !self.token_tracker.take_dirty() {
            return;
        }
        if let Some(db) = &self.db {
            let entries = self.token_tracker.all_cache_entries();
            if let Err(e) = db.save_token_cache(&entries) {
                // Non-fatal: next sync will retry.
                let _ = e;
            }
        }
    }

    pub fn save_config(&self) {
        if let Err(e) = self.try_save_config() {
            // Fire-and-forget callers (theme/background) can't surface errors; log only.
            eprintln!("save_config failed: {e}");
        }
    }

    pub fn try_save_config(&self) -> anyhow::Result<()> {
        if self.store_path.as_os_str().is_empty() {
            return Ok(());
        }
        let config_path = crate::project::amf_config_dir().join("config.json");
        let dir = config_path.parent().unwrap();
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(&self.config)?;
        std::fs::write(&config_path, json)?;
        Ok(())
    }
}

fn latest_prompt_text_for_feature(feature: &Feature) -> Option<String> {
    let preferred_session_kind = match feature.agent {
        AgentKind::Claude => Some(SessionKind::Claude),
        AgentKind::Opencode => Some(SessionKind::Opencode),
        AgentKind::Codex => Some(SessionKind::Codex),
        AgentKind::Pi => Some(SessionKind::Pi),
    };

    let preferred_opencode_session_id = if feature.agent == AgentKind::Opencode {
        feature
            .sessions
            .iter()
            .find(|session| session.kind == SessionKind::Opencode)
            .and_then(|session| session.token_usage_source.as_ref())
            .filter(|source| source.provider == crate::token_tracking::TokenUsageProvider::Opencode)
            .map(|source| source.id.as_str())
    } else {
        None
    };

    crate::app::util::read_latest_prompt_for_session(
        &feature.workdir,
        preferred_session_kind.as_ref(),
        preferred_opencode_session_id,
    )
}

struct SidebarLoadRequest {
    tmux_session: String,
    workdir: PathBuf,
    preferred_session_kind: Option<SessionKind>,
    preferred_session_id: Option<String>,
}

impl SidebarLoadRequest {
    fn from_feature(feature: &Feature) -> Self {
        let preferred_session_kind = match feature.agent {
            AgentKind::Claude => Some(SessionKind::Claude),
            AgentKind::Opencode => Some(SessionKind::Opencode),
            AgentKind::Codex => Some(SessionKind::Codex),
            AgentKind::Pi => Some(SessionKind::Pi),
        };
        let preferred_session_id = match feature.agent {
            AgentKind::Claude => feature
                .sessions
                .iter()
                .find(|session| session.kind == SessionKind::Claude)
                .and_then(|session| session.claude_session_id.clone()),
            AgentKind::Opencode => feature
                .sessions
                .iter()
                .find(|session| session.kind == SessionKind::Opencode)
                .and_then(|session| session.token_usage_source.as_ref())
                .filter(|source| {
                    source.provider == crate::token_tracking::TokenUsageProvider::Opencode
                })
                .map(|source| source.id.clone()),
            AgentKind::Codex => feature
                .sessions
                .iter()
                .find(|session| session.kind == SessionKind::Codex)
                .and_then(|session| session.token_usage_source.as_ref())
                .filter(|source| {
                    source.provider == crate::token_tracking::TokenUsageProvider::Codex
                })
                .map(|source| source.id.clone()),
            AgentKind::Pi => None,
        };

        Self {
            tmux_session: feature.tmux_session.clone(),
            workdir: feature.workdir.clone(),
            preferred_session_kind,
            preferred_session_id,
        }
    }

    fn signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.workdir.hash(&mut hasher);
        self.preferred_session_id.hash(&mut hasher);
        session_kind_signature(self.preferred_session_kind.as_ref()).hash(&mut hasher);
        sidebar_prompt_input_signature(&self.workdir).hash(&mut hasher);

        if self.preferred_session_kind == Some(SessionKind::Opencode) {
            opencode_storage::sidebar_input_signature(
                &self.workdir,
                self.preferred_session_id.as_deref(),
            )
            .hash(&mut hasher);
        }
        if self.preferred_session_kind == Some(SessionKind::Claude) {
            claude_sessions::sidebar_input_signature(
                &self.workdir,
                self.preferred_session_id.as_deref(),
            )
            .hash(&mut hasher);
        }
        if self.preferred_session_kind == Some(SessionKind::Codex)
            && let Some(home) = dirs::home_dir()
        {
            hash_path_metadata(&mut hasher, home.join(".codex").join("config.toml"));
            if let Some(session_id) = self.preferred_session_id.as_deref()
                && let Some(session) = crate::codex::indexed_session(&self.workdir, session_id)
            {
                hash_path_metadata(&mut hasher, session.rollout_path);
            }
        }

        hasher.finish()
    }

    fn load(self, previous_signature: Option<u64>) -> SidebarLoadResult {
        let signature = self.signature();
        let changed = previous_signature != Some(signature);

        if !changed {
            return SidebarLoadResult {
                tmux_session: self.tmux_session,
                signature,
                changed,
                latest_prompt: None,
                model_text: None,
                opencode_sidebar: None,
            };
        }

        let latest_prompt = crate::app::util::read_latest_prompt_for_session(
            &self.workdir,
            self.preferred_session_kind.as_ref(),
            self.preferred_session_id.as_deref(),
        );
        let opencode_sidebar = if self.preferred_session_kind == Some(SessionKind::Opencode) {
            opencode_storage::read_sidebar_data(&self.workdir, self.preferred_session_id.as_deref())
        } else {
            None
        };
        let model_text = match self.preferred_session_kind {
            Some(SessionKind::Claude) => self.preferred_session_id.as_deref().and_then(|id| {
                claude_sessions::sidebar_metadata_for_session_id(&self.workdir, id)
                    .ok()
                    .flatten()
                    .and_then(|metadata| model_line(metadata.model.as_deref(), None))
            }),
            Some(SessionKind::Opencode) => opencode_sidebar.as_ref().and_then(|sidebar| {
                model_line(sidebar.model.as_deref(), sidebar.provider.as_deref())
            }),
            Some(SessionKind::Codex) => {
                let configured_model = crate::codex_config::configured_model();
                model_line(configured_model.as_deref(), None)
            }
            _ => None,
        };

        SidebarLoadResult {
            tmux_session: self.tmux_session,
            signature,
            changed,
            latest_prompt,
            model_text,
            opencode_sidebar,
        }
    }
}

fn model_line(model: Option<&str>, provider: Option<&str>) -> Option<String> {
    let model = model.map(str::trim).filter(|model| !model.is_empty())?;
    let provider = provider
        .map(str::trim)
        .filter(|provider| !provider.is_empty() && !provider.eq_ignore_ascii_case(model));
    Some(match provider {
        Some(provider) => format!("Model: {provider}/{model}"),
        None => format!("Model: {model}"),
    })
}

fn session_kind_signature(session_kind: Option<&SessionKind>) -> u8 {
    match session_kind {
        Some(SessionKind::Claude) => 1,
        Some(SessionKind::Opencode) => 2,
        Some(SessionKind::Codex) => 3,
        Some(SessionKind::Pi) => 4,
        Some(SessionKind::Terminal) => 5,
        Some(SessionKind::Nvim) => 6,
        Some(SessionKind::Vscode) => 7,
        Some(SessionKind::Custom) => 8,
        Some(SessionKind::Todos) => 9,
        None => 0,
    }
}

fn sidebar_prompt_input_signature(workdir: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_path_metadata(&mut hasher, crate::app::util::latest_prompt_path(workdir));
    hash_path_metadata(
        &mut hasher,
        workdir.join(".codex").join("latest-prompt.txt"),
    );
    hasher.finish()
}

fn hash_path_metadata(hasher: &mut impl Hasher, path: PathBuf) {
    path.hash(hasher);
    match std::fs::metadata(&path) {
        Ok(metadata) => {
            true.hash(hasher);
            metadata.len().hash(hasher);
            metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_nanos())
                .hash(hasher);
        }
        Err(_) => false.hash(hasher),
    }
}

#[cfg(test)]
mod sidebar_load_executor_tests {
    use super::SidebarLoadExecutor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex, mpsc};
    use std::time::Duration;

    #[test]
    fn executor_bounds_concurrent_sidebar_loads() {
        let executor = SidebarLoadExecutor::with_worker_count(2);
        let active = Arc::new(AtomicUsize::new(0));
        let max_active = Arc::new(AtomicUsize::new(0));
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let (started_tx, started_rx) = mpsc::channel();

        for _ in 0..6 {
            let active = Arc::clone(&active);
            let max_active = Arc::clone(&max_active);
            let gate = Arc::clone(&gate);
            let started_tx = started_tx.clone();
            executor.submit(Box::new(move || {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                max_active.fetch_max(current, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                let (lock, ready) = &*gate;
                let mut open = lock.lock().unwrap();
                while !*open {
                    open = ready.wait(open).unwrap();
                }
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        drop(started_tx);

        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let third_started_while_blocked = started_rx.try_recv().is_ok();

        let (lock, ready) = &*gate;
        *lock.lock().unwrap() = true;
        ready.notify_all();
        while started_rx.recv_timeout(Duration::from_secs(1)).is_ok() {}

        assert!(!third_started_while_blocked);
        assert_eq!(max_active.load(Ordering::SeqCst), 2);
    }
}
