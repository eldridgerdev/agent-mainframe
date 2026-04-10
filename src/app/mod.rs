mod automation;
mod claude_session_picker;
mod claude_sessions;
mod codex_live;
mod codex_session_picker;
mod codex_sessions;
pub mod commands;
mod diff;
mod feature_ops;
mod harpoon;
mod hooks;
mod navigation;
mod notifications;
mod opencode;
pub(crate) mod opencode_storage;
mod project_ops;
mod rename;
mod review;
mod search;
mod session_config;
mod session_ops;
mod session_titles;
pub mod setup;
mod state;
mod steering;
mod switcher;
mod sync;
mod syntax;
pub mod util;
mod view;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Condvar as StdCondvar, Mutex as StdMutex};

use anyhow::Result;
use ratatui::text::Line;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crate::debug::DebugLog;
use crate::extension::{
    ExtensionConfig, FeaturePreset, load_global_extension_config, merge_project_extension_config,
};
use crate::perf::PerfCollector;
use crate::project::{
    AgentKind, Feature, FeatureSession, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode,
};
use crate::tmux::TmuxManager;
use crate::token_tracking::{SessionTokenTracker, TokenPricingConfig};
use crate::traits::{TmuxOps, WorktreeOps};
use crate::ui::render_ansi_lines;
use crate::usage::UsageManager;
use crate::worktree::WorktreeManager;

pub use self::setup::load_config;
pub use codex_live::CodexLiveThreadState;
pub use codex_sessions::sidebar_metadata_for_session_id as codex_sidebar_metadata_for_session_id;
pub use state::*;
pub use steering::{PromptAnalysis, analyze_prompt};

pub const VIEW_PANE_REFRESH_INTERVAL: Duration = Duration::from_millis(75);
pub const VIEW_CURSOR_REFRESH_INTERVAL: Duration = Duration::from_millis(125);
pub const VIEW_STARTUP_WARM_DURATION: Duration = Duration::from_millis(2500);
pub const VIEW_STARTUP_PANE_REFRESH_INTERVAL: Duration = Duration::from_millis(125);
pub const VIEW_STARTUP_CURSOR_REFRESH_INTERVAL: Duration = Duration::from_millis(350);
pub const VIEW_BURST_DURATION: Duration = Duration::from_millis(175);
pub const VIEW_BURST_PANE_REFRESH_INTERVAL: Duration = Duration::from_millis(30);
pub const VIEW_BURST_CURSOR_REFRESH_INTERVAL: Duration = Duration::from_millis(40);
pub const VIEW_BACKGROUND_SYNC_DEFER_INTERVAL: Duration = Duration::from_millis(1500);

const VIEW_SNAPSHOT_REFRESH_NONE: u8 = 0;
const VIEW_SNAPSHOT_REFRESH_NORMAL: u8 = 1;
const VIEW_SNAPSHOT_REFRESH_BURST: u8 = 2;

#[derive(Debug, Clone)]
pub struct CodexSidebarMetadataResult {
    pub cache_key: String,
    pub title: Option<String>,
    pub prompt: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub nerd_font: bool,
    pub leader_timeout_seconds: u64,
    pub diff_review_viewer: DiffReviewViewer,
    pub diff_viewer_layout: DiffViewerLayout,
    pub zai: Option<ZaiPlanConfig>,
    pub opencode_theme: Option<String>,
    pub projects: ProjectsConfig,
    pub extension: ExtensionConfig,
    pub theme: crate::theme::ThemeName,
    pub transparent_background: bool,
    #[serde(default)]
    pub token_pricing: TokenPricingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum DiffReviewViewer {
    #[default]
    #[serde(rename = "amf", alias = "custom")]
    Amf,
    #[serde(rename = "nvim", alias = "legacy")]
    Nvim,
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
            nerd_font: true,
            leader_timeout_seconds: 5,
            diff_review_viewer: DiffReviewViewer::default(),
            diff_viewer_layout: DiffViewerLayout::Unified,
            zai: None,
            opencode_theme: Some("catppuccin-frappe".to_string()),
            projects: ProjectsConfig::default(),
            extension: ExtensionConfig::default(),
            theme: crate::theme::ThemeName::default(),
            transparent_background: false,
            token_pricing: TokenPricingConfig::default(),
        }
    }
}

pub struct App {
    pub store: ProjectStore,
    pub store_path: PathBuf,
    pub config: AppConfig,
    pub active_extension: ExtensionConfig,
    pub theme: crate::theme::Theme,
    pub selection: Selection,
    pub mode: AppMode,
    pub message: Option<String>,
    pub should_quit: bool,
    pub should_switch: Option<String>,
    pub pane_content: String,
    pub pane_lines: Vec<Line<'static>>,
    pub pane_content_cols: u16,
    pub pane_content_rows: u16,
    pub viewport_cols: u16,
    pub viewport_rows: u16,
    pub tmux_cursor: Option<(u16, u16)>,
    pub leader_active: bool,
    pub leader_activated_at: Option<Instant>,
    pub last_view_activity_at: Option<Instant>,
    pub view_input_batch: Option<ViewInputBatch>,
    pub pending_inputs: Vec<PendingInput>,
    pub latest_prompt_cache: HashMap<String, String>,
    pub sidebar_plan_cache: HashMap<String, String>,
    pub codex_session_title_cache: HashMap<String, Option<String>>,
    pub codex_session_prompt_cache: HashMap<String, Option<String>>,
    pub codex_live_threads: HashMap<String, CodexLiveThreadState>,
    pub codex_sidebar_metadata_tx: std::sync::mpsc::Sender<CodexSidebarMetadataResult>,
    pub codex_sidebar_metadata_rx: std::sync::mpsc::Receiver<CodexSidebarMetadataResult>,
    pub codex_sidebar_metadata_inflight: std::collections::HashSet<String>,
    pub opencode_sidebar_cache: HashMap<String, opencode_storage::OpencodeSidebarData>,
    sidebar_load_tx: Sender<SidebarLoadResult>,
    sidebar_load_rx: Receiver<SidebarLoadResult>,
    sidebar_load_signatures: HashMap<String, u64>,
    pending_sidebar_loads: std::collections::HashSet<String>,
    pub usage: UsageManager,
    pub token_tracker: SessionTokenTracker,
    pub scroll_offset: usize,
    pub session_filter: SessionFilter,
    pub throbber_state: throbber_widgets_tui::ThrobberState,
    pub thinking_features: std::collections::HashSet<String>,
    pub ipc_thinking_sessions: std::collections::HashSet<String>,
    pub ipc_tool_sessions: std::collections::HashSet<String>,
    pub summary_state: SummaryState,
    pub summary_rx: Option<std::sync::mpsc::Receiver<(String, Result<String, anyhow::Error>)>>,
    pub tmux: Box<dyn TmuxOps>,
    pub worktree: Box<dyn WorktreeOps>,
    pub debug_log: DebugLog,
    pub perf: PerfCollector,
    pub background_deletions: HashMap<String, BackgroundDeletion>,
    pub background_hooks: HashMap<String, BackgroundHook>,
    pub ipc: Option<crate::ipc::IpcGuard>,
    pub ipc_fallback_logged: bool,
    pub last_file_notification_count: usize,
    pub last_file_notification_fingerprint: Option<u64>,
    pub vscode_available: bool,
    view_snapshot_tx: Sender<ViewSnapshot>,
    view_snapshot_rx: Receiver<ViewSnapshot>,
    view_snapshot_stop: Option<Arc<AtomicBool>>,
    view_snapshot_refresh: Option<Arc<AtomicU8>>,
    view_snapshot_condvar: Option<Arc<(StdMutex<()>, StdCondvar)>>,
    view_snapshot_target: Option<(String, String, u16, u16)>,
}

struct SidebarLoadResult {
    tmux_session: String,
    signature: u64,
    latest_prompt: Option<String>,
    opencode_sidebar: Option<opencode_storage::OpencodeSidebarData>,
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
        match &self.mode {
            AppMode::Normal => self.has_dashboard_animation(),
            AppMode::RunningHook(state) => state.child.is_some(),
            AppMode::DeletingFeatureInProgress(state) => state.child.is_some(),
            AppMode::SyntaxLanguagePicker(state) => state.operation.is_some(),
            AppMode::DiffReviewPrompt(state) => state.explanation_child.is_some(),
            _ => false,
        }
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
        self.should_switch.hash(&mut hasher);
        self.leader_active.hash(&mut hasher);
        self.message.hash(&mut hasher);
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
            view.sidebar_visible.hash(&mut hasher);
            view.todos_expanded.hash(&mut hasher);
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
            self.request_view_snapshot_burst();
        }
        result.map(|_| true)
    }

    fn current_view_snapshot_target(&self) -> Option<(String, String, u16, u16)> {
        match &self.mode {
            AppMode::Viewing(view) if self.pane_content_cols > 0 && self.pane_content_rows > 0 => {
                Some((
                    view.session.clone(),
                    view.window.clone(),
                    self.pane_content_cols,
                    self.pane_content_rows,
                ))
            }
            _ => None,
        }
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
        self.view_snapshot_condvar = None;
        self.view_snapshot_target = None;
    }

    pub fn request_view_snapshot_refresh(&self) {
        self.request_view_snapshot_refresh_kind(VIEW_SNAPSHOT_REFRESH_NORMAL);
    }

    pub fn request_view_snapshot_burst(&self) {
        self.request_view_snapshot_refresh_kind(VIEW_SNAPSHOT_REFRESH_BURST);
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
    fn run_capture_pane_worker(
        session: &str,
        window: &str,
        cols: u16,
        rows: u16,
        stop: &AtomicBool,
        refresh: &AtomicU8,
        condvar: &(StdMutex<()>, StdCondvar),
        tx: &Sender<ViewSnapshot>,
    ) {
        let mut next_pane_refresh = Instant::now() - VIEW_PANE_REFRESH_INTERVAL;
        let mut next_cursor_refresh = Instant::now() - VIEW_CURSOR_REFRESH_INTERVAL;
        let mut burst_until = Instant::now();
        let worker_started_at = Instant::now();

        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            let refresh_kind = refresh.swap(VIEW_SNAPSHOT_REFRESH_NONE, Ordering::Relaxed);
            let refresh_requested = refresh_kind != VIEW_SNAPSHOT_REFRESH_NONE;
            if refresh_kind == VIEW_SNAPSHOT_REFRESH_BURST {
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
            let pane_due = refresh_requested
                || now.duration_since(next_pane_refresh) >= pane_interval;
            let cursor_due = refresh_requested
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
                        TmuxManager::capture_pane_ansi(session, window)
                            .unwrap_or_default();
                    capture_duration = Some(started_at.elapsed());
                    let render_started_at = Instant::now();
                    rendered_lines = Some(render_ansi_lines(&captured, cols, rows));
                    render_duration = Some(render_started_at.elapsed());
                    pane_content = Some(captured);
                    next_pane_refresh = Instant::now();
                }

                if cursor_due {
                    let cursor_started_at = Instant::now();
                    cursor = Some(
                        TmuxManager::cursor_position(session, window).ok(),
                    );
                    cursor_duration = Some(cursor_started_at.elapsed());
                    next_cursor_refresh = Instant::now();
                }

                let _ = tx.send(ViewSnapshot {
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
        tx: &Sender<ViewSnapshot>,
    ) -> anyhow::Result<()> {
        use std::fs;
        use std::io::Read as _;
        use std::os::unix::fs::OpenOptionsExt;

        // Slug the session name for a safe FIFO path.
        let slug: String = session
            .chars()
            .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' })
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
            if refresh_requested {
                pane_has_new_output = true;
            }
            if refresh_kind == VIEW_SNAPSHOT_REFRESH_BURST {
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
            let pane_due = pane_has_new_output
                && now.duration_since(last_capture) >= pane_interval;

            let cursor_interval = if burst_active {
                VIEW_BURST_CURSOR_REFRESH_INTERVAL
            } else {
                VIEW_CURSOR_REFRESH_INTERVAL
            };
            let cursor_due = refresh_requested
                || now.duration_since(next_cursor_refresh) >= cursor_interval;

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
                    let captured = TmuxManager::capture_pane_ansi(session, window)
                        .unwrap_or_default();
                    capture_duration = Some(started_at.elapsed());
                    let render_started = Instant::now();
                    rendered_lines = Some(render_ansi_lines(&captured, cols, rows));
                    render_duration = Some(render_started.elapsed());
                    pane_content = Some(captured);
                    last_capture = Instant::now();
                }

                if cursor_due {
                    let cursor_started = Instant::now();
                    cursor = Some(
                        TmuxManager::cursor_position(session, window).ok(),
                    );
                    cursor_duration = Some(cursor_started.elapsed());
                    next_cursor_refresh = Instant::now();
                }

                let _ = tx.send(ViewSnapshot {
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

        if self.view_snapshot_target.as_ref() == Some(&(session.clone(), window.clone(), cols, rows))
        {
            return;
        }

        self.stop_view_snapshot_worker();
        self.drain_view_snapshots();
        self.pane_lines.clear();

        let stop = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicU8::new(VIEW_SNAPSHOT_REFRESH_NORMAL));
        let condvar = Arc::new((StdMutex::new(()), StdCondvar::new()));
        let tx = self.view_snapshot_tx.clone();
        let worker_session = session.clone();
        let worker_window = window.clone();
        let worker_cols = cols;
        let worker_rows = rows;
        let worker_stop = stop.clone();
        let worker_refresh = refresh.clone();
        let worker_condvar = condvar.clone();

        std::thread::spawn(move || {
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
        self.view_snapshot_condvar = Some(condvar);
        self.view_snapshot_target = Some((session, window, cols, rows));
    }

    pub fn drain_view_snapshots(&mut self) -> (bool, bool) {
        let current_target = self.current_view_snapshot_target();
        let mut pane_changed = false;
        let mut cursor_changed = false;

        while let Ok(snapshot) = self.view_snapshot_rx.try_recv() {
            if current_target.as_ref()
                != Some(&(
                    snapshot.session.clone(),
                    snapshot.window.clone(),
                    self.pane_content_cols,
                    self.pane_content_rows,
                ))
            {
                continue;
            }

            if let Some(duration) = snapshot.capture_duration {
                self.perf.record_duration("view.capture_pane_ansi", duration);
            }
            if let Some(duration) = snapshot.render_duration {
                self.perf.record_duration("view.render_snapshot_lines", duration);
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
                pane_changed = true;
            }

            if let Some(rendered_lines) = snapshot.rendered_lines
                && (pane_changed || self.pane_lines.is_empty())
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

    pub fn new(store_path: PathBuf) -> Result<Self> {
        setup::ensure_notify_scripts();
        crate::project::prepare_store_path(&store_path, &crate::project::global_store_path());
        let store = ProjectStore::load(&store_path)?;
        let (sidebar_load_tx, sidebar_load_rx) = std::sync::mpsc::channel();
        let latest_prompt_cache = Self::build_latest_prompt_cache(&store);
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
        let sidebar_plan_cache = Self::build_sidebar_plan_cache(&store);
        let (codex_sidebar_metadata_tx, codex_sidebar_metadata_rx) = std::sync::mpsc::channel();
        let (view_snapshot_tx, view_snapshot_rx) = channel();
        let mut theme = crate::theme::Theme::load(&config.theme);
        theme.set_transparent(config.transparent_background);
        Ok(Self {
            store,
            store_path,
            config,
            active_extension,
            theme,
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            message: None,
            should_quit: false,
            should_switch: None,
            pane_content: String::new(),
            pane_lines: Vec::new(),
            pane_content_cols: 0,
            pane_content_rows: 0,
            viewport_cols: 0,
            viewport_rows: 0,
            tmux_cursor: None,
            leader_active: false,
            leader_activated_at: None,
            last_view_activity_at: None,
            view_input_batch: None,
            pending_inputs: Vec::new(),
            latest_prompt_cache,
            sidebar_plan_cache,
            codex_session_title_cache: HashMap::new(),
            codex_session_prompt_cache: HashMap::new(),
            codex_live_threads: HashMap::new(),
            codex_sidebar_metadata_tx,
            codex_sidebar_metadata_rx,
            codex_sidebar_metadata_inflight: std::collections::HashSet::new(),
            opencode_sidebar_cache: HashMap::new(),
            sidebar_load_tx,
            sidebar_load_rx,
            sidebar_load_signatures: HashMap::new(),
            pending_sidebar_loads: std::collections::HashSet::new(),
            usage: UsageManager::new(zai_enabled, zai_monthly, zai_weekly, zai_five_hour),
            token_tracker: SessionTokenTracker::default(),
            scroll_offset: 0,
            session_filter: SessionFilter::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            thinking_features: std::collections::HashSet::new(),
            ipc_thinking_sessions: std::collections::HashSet::new(),
            ipc_tool_sessions: std::collections::HashSet::new(),
            summary_state: SummaryState::new(),
            summary_rx: None,
            tmux: Box::new(TmuxManager),
            worktree: Box::new(WorktreeManager),
            debug_log: DebugLog::default(),
            perf: PerfCollector::new(),
            background_deletions: HashMap::new(),
            background_hooks: HashMap::new(),
            ipc: None,
            ipc_fallback_logged: false,
            last_file_notification_count: 0,
            last_file_notification_fingerprint: None,
            vscode_available: std::process::Command::new("code")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok(),
            view_snapshot_tx,
            view_snapshot_rx,
            view_snapshot_stop: None,
            view_snapshot_refresh: None,
            view_snapshot_condvar: None,
            view_snapshot_target: None,
        })
    }

    pub fn log_startup(&mut self) {
        self.debug_log.info("amf", "AMF started".to_string());
        self.debug_log
            .debug("amf", format!("Store path: {}", self.store_path.display()));
        self.debug_log.debug(
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
        let (view_snapshot_tx, view_snapshot_rx) = channel();
        Self {
            store,
            store_path: PathBuf::new(),
            config: AppConfig::default(),
            active_extension: ExtensionConfig::default(),
            theme: crate::theme::Theme::default(),
            selection: Selection::Project(0),
            mode: AppMode::Normal,
            message: None,
            should_quit: false,
            should_switch: None,
            pane_content: String::new(),
            pane_lines: Vec::new(),
            pane_content_cols: 0,
            pane_content_rows: 0,
            viewport_cols: 0,
            viewport_rows: 0,
            tmux_cursor: None,
            leader_active: false,
            leader_activated_at: None,
            last_view_activity_at: None,
            view_input_batch: None,
            pending_inputs: Vec::new(),
            latest_prompt_cache,
            sidebar_plan_cache,
            codex_session_title_cache: HashMap::new(),
            codex_session_prompt_cache: HashMap::new(),
            codex_live_threads: HashMap::new(),
            codex_sidebar_metadata_tx,
            codex_sidebar_metadata_rx,
            codex_sidebar_metadata_inflight: std::collections::HashSet::new(),
            opencode_sidebar_cache: HashMap::new(),
            sidebar_load_tx,
            sidebar_load_rx,
            sidebar_load_signatures: HashMap::new(),
            pending_sidebar_loads: std::collections::HashSet::new(),
            usage: UsageManager::new(false, None, None, None),
            token_tracker: SessionTokenTracker::default(),
            scroll_offset: 0,
            session_filter: SessionFilter::default(),
            throbber_state: throbber_widgets_tui::ThrobberState::default(),
            thinking_features: std::collections::HashSet::new(),
            ipc_thinking_sessions: std::collections::HashSet::new(),
            ipc_tool_sessions: std::collections::HashSet::new(),
            summary_state: SummaryState::new(),
            summary_rx: None,
            tmux,
            worktree,
            debug_log: DebugLog::default(),
            perf: PerfCollector::new(),
            background_deletions: HashMap::new(),
            background_hooks: HashMap::new(),
            ipc: None,
            ipc_fallback_logged: false,
            last_file_notification_count: 0,
            last_file_notification_fingerprint: None,
            vscode_available: false,
            view_snapshot_tx,
            view_snapshot_rx,
            view_snapshot_stop: None,
            view_snapshot_refresh: None,
            view_snapshot_condvar: None,
            view_snapshot_target: None,
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
        let signature = request.signature();

        if self
            .sidebar_load_signatures
            .get(&request.tmux_session)
            .is_some_and(|cached| *cached == signature)
        {
            return;
        }

        if !self
            .pending_sidebar_loads
            .insert(request.tmux_session.clone())
        {
            return;
        }

        let tx = self.sidebar_load_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(request.load(signature));
        });
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

    pub(crate) fn poll_sidebar_load_results(&mut self) {
        while let Ok(result) = self.sidebar_load_rx.try_recv() {
            self.pending_sidebar_loads.remove(&result.tmux_session);
            self.sidebar_load_signatures
                .insert(result.tmux_session.clone(), result.signature);

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

            if let Some(data) = result.opencode_sidebar {
                self.opencode_sidebar_cache
                    .insert(result.tmux_session, data);
            } else {
                self.opencode_sidebar_cache.remove(&result.tmux_session);
            }
        }
    }

    pub(crate) fn clear_sidebar_state_for_session(&mut self, tmux_session: &str) {
        self.pending_sidebar_loads.remove(tmux_session);
        self.sidebar_load_signatures.remove(tmux_session);
        self.latest_prompt_cache.remove(tmux_session);
        self.sidebar_plan_cache.remove(tmux_session);
        self.codex_live_threads.remove(tmux_session);
        self.opencode_sidebar_cache.remove(tmux_session);
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
                prompt: metadata.and_then(|metadata| metadata.latest_prompt),
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

    pub(crate) fn viewport_size(&self) -> Option<(u16, u16)> {
        match (self.viewport_cols, self.viewport_rows) {
            (0, _) | (_, 0) => None,
            dims => Some(dims),
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
        self.extension_for_repo(repo).allowed_agents()
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
        self.extension_for_repo(repo).allows_agent(agent)
    }

    pub(crate) fn allowed_feature_presets_for_repo(&self, repo: &Path) -> Vec<FeaturePreset> {
        self.extension_for_repo(repo).allowed_feature_presets()
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
        self.store.save(&self.store_path)
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
        self.debug_log.debug(context, message);
    }

    pub fn log_info(&mut self, context: &str, message: String) {
        self.debug_log.info(context, message);
    }

    pub fn log_warn(&mut self, context: &str, message: String) {
        self.debug_log.warn(context, message);
    }

    pub fn log_error(&mut self, context: &str, message: String) {
        self.debug_log.error(context, message);
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

    pub fn save_config(&self) {
        if self.store_path.as_os_str().is_empty() {
            return;
        }
        let config_path = crate::project::amf_config_dir().join("config.json");
        let dir = config_path.parent().unwrap();
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&self.config).unwrap_or_default(),
        );
    }
}

fn latest_prompt_text_for_feature(feature: &Feature) -> Option<String> {
    let preferred_session_kind = match feature.agent {
        AgentKind::Claude => Some(SessionKind::Claude),
        AgentKind::Opencode => Some(SessionKind::Opencode),
        AgentKind::Codex => Some(SessionKind::Codex),
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

        hasher.finish()
    }

    fn load(self, signature: u64) -> SidebarLoadResult {
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

        SidebarLoadResult {
            tmux_session: self.tmux_session,
            signature,
            latest_prompt,
            opencode_sidebar,
        }
    }
}

fn session_kind_signature(session_kind: Option<&SessionKind>) -> u8 {
    match session_kind {
        Some(SessionKind::Claude) => 1,
        Some(SessionKind::Opencode) => 2,
        Some(SessionKind::Codex) => 3,
        Some(SessionKind::Terminal) => 4,
        Some(SessionKind::Nvim) => 5,
        Some(SessionKind::Vscode) => 6,
        Some(SessionKind::Custom) => 7,
        None => 0,
    }
}

fn sidebar_prompt_input_signature(workdir: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_path_metadata(&mut hasher, crate::app::util::latest_prompt_path(workdir));
    hash_path_metadata(&mut hasher, workdir.join(".codex").join("latest-prompt.txt"));
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
