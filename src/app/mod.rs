pub(crate) mod agent_sidebar;
pub(crate) mod ai_review;
pub(crate) mod attention;
mod automation;
mod bookmarks;
mod claude_session_picker;
mod claude_sessions;
pub(crate) mod clone_project;
mod clone_project_tui;
mod codex_live;
mod codex_session_picker;
mod codex_sessions;
pub mod commands;
mod compose;
mod compose_state;
mod config_wizard;
mod construction;
mod context_hints;
mod context_settings;
mod diff;
pub(crate) mod dormant;
pub(crate) mod edit_explanation;
pub(crate) mod editor_ops;
pub(crate) mod feature_ops;
pub(crate) mod feature_setup;
pub(crate) mod fix_cost;
pub(crate) mod github_workflow;
mod handoff;
mod harness_change;
mod hooks;
pub(crate) mod issue_fixer;
pub(crate) mod learning;
pub(crate) mod model_analysis;
mod navigation;
mod notifications;
mod opencode;
pub(crate) mod opencode_storage;
mod pane_runtime;
pub(crate) mod plan;
mod plan_interview;
mod plan_interview_attach;
mod plan_interview_state;
pub(crate) mod pr_review;
pub(crate) mod pr_workspace;
pub(crate) mod precall;
mod project_ops;
mod prompt_library;
mod prompt_library_files;
mod prompt_library_state;
mod prompt_override_state;
pub(crate) mod prompt_overrides;
pub(crate) mod remote_actions;
pub(crate) mod remote_attention;
pub mod remote_control;
pub(crate) mod remote_push;
pub(crate) mod remote_reach;
pub(crate) mod remote_server;
pub(crate) mod remote_tailscale;
pub(crate) mod remote_todos;
mod rename;
pub(crate) mod resource_gate;
pub(crate) mod review;
pub(crate) mod review_destination;
pub(crate) mod review_memory;
pub(crate) mod review_questions;
mod sandbox;
pub(crate) mod screenshots;
mod search;
mod session_config;
pub(crate) mod session_ops;
mod session_titles;
pub mod setup;
mod sidebar_runtime;
mod skill_picker;
mod state;
mod steering;
pub(crate) mod supervised_edits;
mod switcher;
mod sync;
mod syntax;
pub(crate) mod toast;
mod todo_state;
mod todos;
pub(crate) mod triage_feature;
pub mod util;
mod view;

#[cfg(test)]
mod tests;

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use ratatui::text::Line;
use serde::{Deserialize, Serialize};
use std::os::unix::io::{AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use sidebar_runtime::SidebarLoadExecutor;

use crate::context_collectors::SessionContextCollector;
use crate::context_tracking::SessionContextState;
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
pub(crate) use self::sync::SummaryGenerator;
pub use automation::FeatureCreateOptions;
pub(crate) use automation::WorktreeHookChoiceError;
pub use codex_live::CodexLiveThreadState;
pub use codex_sessions::sidebar_metadata_for_session_id as codex_sidebar_metadata_for_session_id;
pub(crate) use config_wizard::agent_toggles_to_allowed;
pub(crate) use diff::context_level_label;
pub use feature_setup::{FeatureSetupRow, FeatureSetupState};
pub(crate) use issue_fixer::{ISSUE_PAGE_SIZE, issue_branch_name, issue_fix_prompt};
pub use issue_fixer::{IssueBrowserState, IssueBrowserStatus};
pub(crate) use resource_gate::{StartIntent, Started};
pub(crate) use session_ops::session_kind_for_agent;
pub use state::*;
pub use steering::{PromptAnalysis, PromptConstraint, analyze_prompt};
pub use toast::Toast;
pub(crate) use todos::{NextTodo, ReferencedTodoDeleted};

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

/// How long the dashboard PR sweep stands down after GitHub reports an
/// exhausted GraphQL budget.
///
/// The budget refills on a rolling hourly window, so the exact reset is not
/// knowable from a failed call. Fifteen minutes retries a few times an hour at
/// a cost of one sweep each, rather than either giving up for the hour or
/// hammering a limit that is still empty.
pub const GH_GRAPHQL_BACKOFF: Duration = Duration::from_secs(15 * 60);

/// How often dashboard PR badges refresh.
///
/// This used to ride the 30-second feature-status cadence, which is wrong for
/// this data: the sweep runs `gh pr view` once per *feature* (measured at 2
/// GraphQL points each) plus a thread query per open PR, against a 5,000-point
/// hourly budget. At ~34 features that is ~71 points a sweep, and at 120
/// sweeps an hour roughly 8,500 points — the budget is gone in about 35
/// minutes of AMF simply being open, which then takes PR Triage down with it.
///
/// Volume is the whole problem; no single call here is expensive. Five minutes
/// cuts it tenfold and is still far inside a PR badge's useful freshness.
pub const ACTIVE_PR_SYNC_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// `App::save`'s message on a detected cross-process conflict (`AMF_PLAN.md`
/// Task 5). A shared constant, not a literal repeated at each call site, so
/// `gui_contract::GuiError`'s `From<anyhow::Error>` impl can classify this
/// specific failure as `GuiErrorKind::Conflict` by exact match without the
/// two texts silently drifting apart.
pub(crate) const SAVE_CONFLICT_MESSAGE: &str = "Workspace changed elsewhere (another AMF window or process) before this save landed. Your \
     change was not saved; the view has been refreshed with the current state -- please retry.";

/// How many times `App::save_reapplying` re-applies a change after a
/// conflict before giving up. Each conflict means another process committed
/// in the moment between this one's reload and its save; two in a row is
/// already unusual, so this only bounds a pathological writer.
const SAVE_REAPPLY_ATTEMPTS: usize = 3;

/// Result of `App::save_reapplying`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReapplyOutcome {
    /// The change is on disk.
    Saved,
    /// The row the change applies to was removed by another writer; the
    /// store is that writer's, without the change.
    TargetGone,
    /// Other writers kept landing first; the store is the latest on disk,
    /// without the change.
    Conflict,
}

/// Cached dashboard metadata for an open pull request associated with a
/// feature's branch. The branch and head SHA travel with the badge so a
/// background result can never be applied after the feature changes branches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivePrStatus {
    pub branch: String,
    pub head_sha: String,
    pub number: u32,
    pub is_draft: bool,
    /// `None` means the PR resolved but its thread metadata could not be read.
    pub unresolved_threads: Option<usize>,
}

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

use crate::tmux::{parse_tmux_output_notification, sanitize_tmux_control_line};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalCommand {
    OpenDebugLog,
    RefreshNotifications,
    CheckPendingDiffReview,
    PlanInterview,
    ChangeVibeMode(VibeMode),
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Path to the **cross-project** review-findings memory doc, read by the
    /// AI reviewer in addition to each repo's own doc and writable from the
    /// `M` / bootstrap flows' "global" scope. Relative values resolve against
    /// the AMF config dir; absolute ones are used as-is. Defaults to
    /// [`review_memory::DEFAULT_GLOBAL_REVIEW_MEMORY_FILE`] there (i.e.
    /// `~/.config/amf/review-memory.md`) when unset. Global-only by nature —
    /// unlike [`Self::review_memory_path`] there is no per-project override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_review_memory_path: Option<String>,
    /// Name of an existing Claude Code skill/command (without the leading `/`,
    /// e.g. `"review"`) to lead the AI PR-review prompt (`A` in the PR-review
    /// pane, Epic E of `pr-comment-review-plan.md`) with, so its review
    /// methodology runs before AMF's own structured-findings instructions.
    /// `None` (default) skips this — AMF ships no bundled skill for reviewing a
    /// PR diff, so the feature must work without assuming one is installed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_review_skill: Option<String>,
    /// Default explicit model name/id passed as `--model` to the headless CLI
    /// for every review action listed on [`ReviewAction`], independent of
    /// whatever model the feature's own interactive session runs. Format
    /// depends on the harness running the review (a bare name/alias for
    /// Claude/Codex/Pi, `provider/model` for Opencode). `None` (default)
    /// passes no explicit model, so the harness's own default model applies.
    /// Overridden
    /// per-action by `review_models`; see [`AppConfig::review_model_for`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_model: Option<String>,
    /// Per-action overrides of `review_model`, keyed by
    /// [`ReviewAction::config_key`] (e.g. `"changeset_overview"`). An action
    /// missing here (or an unrecognized key) falls back to `review_model`.
    /// Lets a cheap single-file pass (the walkthrough) run a lighter model
    /// than a whole-changeset pass without forcing every action onto one
    /// setting.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub review_models: std::collections::BTreeMap<String, String>,
    /// Default reasoning level for every review action, paired with
    /// `review_model`. `None` (default) passes nothing, so the harness's own
    /// level applies. A level the chosen harness cannot express is dropped
    /// at dispatch (see `headless::reasoning_args`), never passed through.
    /// Overridden per-action by `review_reasonings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_reasoning: Option<crate::headless::ReasoningLevel>,
    /// Per-action overrides of `review_reasoning`, keyed like `review_models`.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub review_reasonings: std::collections::BTreeMap<String, crate::headless::ReasoningLevel>,
    /// Soft cap on how many agent-harness sessions may run at once across
    /// **all** projects (the store is machine-global, so the limit is too).
    /// Only agent harnesses count — terminals, editors, and TODOs sessions
    /// do not. Starting past the cap warns and asks for an explicit confirm;
    /// it is never a hard refusal. `0` disables the check.
    #[serde(default = "default_max_concurrent_agents")]
    pub max_concurrent_agents: usize,
    /// Warn before starting an agent when the OS reports less than this much
    /// available memory (MiB). Like the concurrency cap this is a soft gate:
    /// warn plus explicit confirm. `0` disables the check, as does a platform
    /// with no usable memory signal.
    #[serde(default = "default_low_memory_warn_mb")]
    pub low_memory_warn_mb: u64,
    /// Kill the editor AMF launched for a feature (and its children, e.g.
    /// language servers) when the feature is stopped. Editors AMF cannot
    /// prove it owns are left alone regardless of this setting.
    #[serde(default = "default_true")]
    pub kill_editor_on_stop: bool,
    /// A feature counts as dormant only when its agent has been idle for at
    /// least this many minutes **and** [`Self::dormant_last_accessed_hours`]
    /// has also elapsed. `0` in either key disables dormant detection.
    #[serde(default = "default_dormant_idle_minutes")]
    pub dormant_idle_minutes: u64,
    /// Second half of the dormancy test: the feature was last accessed at
    /// least this many hours ago. Both halves must hold. `0` in either key
    /// disables dormant detection.
    #[serde(default = "default_dormant_last_accessed_hours")]
    pub dormant_last_accessed_hours: u64,
    /// How long a needs-attention state (question / completed-awaiting-review)
    /// stays on the dashboard before ageing out into plain idle. A session
    /// nobody has answered for this long is no longer news, and leaving it in
    /// the needs-attention list forever would drown the sessions that are.
    /// `0` disables ageing, matching [`Self::max_concurrent_agents`] and
    /// [`Self::low_memory_warn_mb`].
    #[serde(default = "default_waiting_stale_minutes")]
    pub waiting_stale_minutes: u64,
    /// Overrides the context-window size AMF assumes when a harness's own
    /// telemetry doesn't report one (Claude falls back to a hardcoded
    /// 900,000; Codex, OpenCode, and Pi otherwise have no fallback at all —
    /// see `CLAUDE_DEFAULT_CONTEXT_LIMIT` in `context_collectors.rs`). `None`
    /// keeps each harness's existing default behavior.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_override: Option<u64>,
    /// Percentage of the context window at which the severity indicator
    /// switches from normal to `ContextBand::Warning`.
    #[serde(default = "default_context_warning_percent")]
    pub context_warning_percent: u8,
    /// Percentage of the context window at which the severity indicator
    /// switches to `ContextBand::Critical`. Must stay greater than
    /// `context_warning_percent` for the bands to remain meaningful.
    #[serde(default = "default_context_critical_percent")]
    pub context_critical_percent: u8,
    /// Global fallback for [`crate::extension::ExtensionConfig::review_prompt_budget_tokens`]:
    /// the estimated prompt-token ceiling above which `W` AI PR review and
    /// final-review co-review split an oversized diff into slices. `None` uses
    /// the built-in per-harness default (Claude/Codex 128k, OpenCode/Pi 96k);
    /// `0` disables pre-send splitting (adaptive halving only). A project's
    /// `amf.json` `review_prompt_budget_tokens` overrides this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_prompt_budget_tokens: Option<usize>,
    /// Where the Remote Control server listens (`host:port`). Loopback by
    /// default: reaching it from a phone goes through a tunnel the user runs
    /// (`tailscale serve`, cloudflared, …), and a fixed port is what lets
    /// that tunnel survive toggling the server. `0.0.0.0:<port>` exposes it
    /// on the LAN instead (plain HTTP, so no install or push there).
    #[serde(default = "default_remote_bind")]
    pub remote_bind: String,
    /// The URL a phone uses to reach the Remote Control server, e.g.
    /// `https://my-pc.tailnet.ts.net`. The pairing QR encodes it so a scan
    /// opens the right page; unset, the QR falls back to the bind address,
    /// which only works on this machine or over `adb reverse`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_public_url: Option<String>,
    /// The `tailscale` CLI AMF asks for this machine's tailnet address when
    /// `remote_public_url` is unset (`crate::tailscale`). Unset means
    /// `tailscale` on `PATH`, then the macOS app's bundled CLI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_tailscale_cli: Option<String>,
    /// `tailscaled`'s socket, for a daemon not at its platform default —
    /// one the user starts themselves with its own `--socket`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_tailscale_socket: Option<String>,
    /// MCP servers (e.g. an issue tracker) the plan interview's Claude
    /// passes may consult, read-only. Global scope only, deliberately: an MCP
    /// config names programs to run, so a repository's `amf.json` must not be
    /// able to supply one. See [`crate::headless::HeadlessMcpConfig`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_interview_mcp: Option<crate::headless::HeadlessMcpConfig>,
}

pub(crate) fn default_remote_bind() -> String {
    "127.0.0.1:47800".to_string()
}

/// The distinct headless review call sites that each read `review_model`
/// (optionally overridden per-action via `review_models`). Bootstrapping and
/// compacting the review-memory doc share the `review_memory` key: both
/// operate on the same doc with the same cost/quality tradeoff, so splitting
/// them would just be two knobs for one decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewAction {
    /// Explicit Expert review of a synthesized implementation plan.
    PlanPreflight,
    /// On-demand walkthrough for a noteless file (`w` in Final Review).
    Walkthrough,
    /// AI co-reviewer first pass over the current file (`A` in Final Review).
    CoReview,
    /// Whole-changeset overview + risk markers (`O` in Final Review).
    ChangesetOverview,
    /// Config-wizard / hook diff-review explanation.
    DiffExplain,
    /// AI PR review pane (`A` in PR Triage / the AI Review pane).
    PrReview,
    /// Review-memory lookback bootstrap and compaction (PR picker).
    ReviewMemory,
}

impl ReviewAction {
    pub fn config_key(self) -> &'static str {
        match self {
            ReviewAction::PlanPreflight => "plan_preflight",
            ReviewAction::Walkthrough => "walkthrough",
            ReviewAction::CoReview => "co_review",
            ReviewAction::ChangesetOverview => "changeset_overview",
            ReviewAction::DiffExplain => "diff_explain",
            ReviewAction::PrReview => "pr_review",
            ReviewAction::ReviewMemory => "review_memory",
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_agent_restart_limit() -> usize {
    1
}

// Resource-guard defaults. Grounded in measured idle RSS per harness on a
// typical dev box (the README's Configuration section carries the numbers): a Claude
// harness settles around 450 MiB, Codex/Opencode a little under that, and each
// editor-launched language server (rust-analyzer on this codebase) dwarfs them
// at 1-2 GiB. Four concurrent harnesses is roughly 1.8 GiB of agent, which
// leaves room for the editors and language servers they pull in on a 16 GiB
// machine without the limit tripping during ordinary two-or-three-feature work.
fn default_max_concurrent_agents() -> usize {
    4
}

// One harness plus the tool subprocesses it spawns needs on the order of
// 700 MiB - 1 GiB; warning below 1.5 GiB available leaves enough headroom to
// notice before the OOM killer does.
fn default_low_memory_warn_mb() -> u64 {
    1536
}

fn default_dormant_idle_minutes() -> u64 {
    60
}

fn default_dormant_last_accessed_hours() -> u64 {
    4
}

// Half an hour is long enough to survive a coffee break or a meeting — the
// common reasons a question goes unanswered — and short enough that a
// forgotten session drops out of the needs-attention list well before the
// 60-minute dormancy check surfaces it in `z`.
fn default_waiting_stale_minutes() -> u64 {
    30
}

fn default_context_warning_percent() -> u8 {
    crate::context_tracking::DEFAULT_CONTEXT_WARNING_PERCENT
}

fn default_context_critical_percent() -> u8 {
    crate::context_tracking::DEFAULT_CONTEXT_CRITICAL_PERCENT
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
    /// Directory new projects (e.g. clones) are created under. `None` means
    /// the default, `~/code`; see [`AppConfig::projects_root`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects_root: Option<String>,
    /// Sandbox settings for projects created with "run in container".
    #[serde(skip_serializing_if = "container_settings_is_default")]
    pub container: crate::container::ContainerSettings,
}

fn container_settings_is_default(c: &crate::container::ContainerSettings) -> bool {
    *c == crate::container::ContainerSettings::default()
}

/// Default projects root when none is configured.
pub(crate) const DEFAULT_PROJECTS_ROOT: &str = "~/code";

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
            global_review_memory_path: None,
            ai_review_skill: None,
            review_model: None,
            review_models: std::collections::BTreeMap::new(),
            review_reasoning: None,
            review_reasonings: std::collections::BTreeMap::new(),
            max_concurrent_agents: default_max_concurrent_agents(),
            low_memory_warn_mb: default_low_memory_warn_mb(),
            kill_editor_on_stop: true,
            dormant_idle_minutes: default_dormant_idle_minutes(),
            dormant_last_accessed_hours: default_dormant_last_accessed_hours(),
            waiting_stale_minutes: default_waiting_stale_minutes(),
            context_window_override: None,
            context_warning_percent: default_context_warning_percent(),
            context_critical_percent: default_context_critical_percent(),
            review_prompt_budget_tokens: None,
            remote_bind: default_remote_bind(),
            remote_public_url: None,
            remote_tailscale_cli: None,
            remote_tailscale_socket: None,
            plan_interview_mcp: None,
        }
    }
}

impl AppConfig {
    /// The effective projects root: the configured value (blank counts as
    /// unset) or `~/code`, with a leading `~` / `~/` expanded. The directory
    /// is not created.
    pub fn projects_root(&self) -> std::path::PathBuf {
        let raw = self
            .projects
            .projects_root
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(DEFAULT_PROJECTS_ROOT);
        expand_tilde(raw, dirs::home_dir())
    }

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

    /// The `--model` override for a given review action: `review_models`'s
    /// entry for [`ReviewAction::config_key`] if set, else the shared
    /// `review_model` default, else `None` (harness default).
    pub fn review_model_for(&self, action: ReviewAction) -> Option<String> {
        let action_model = self.review_models.get(action.config_key()).cloned();
        if action == ReviewAction::PlanPreflight {
            // An Expert plan review must never silently inherit an ordinary
            // review model. The user either names its model at dispatch time
            // or configures this action explicitly.
            action_model
        } else {
            action_model.or_else(|| self.review_model.clone())
        }
    }

    /// The reasoning level for a review action, resolved exactly like
    /// [`Self::review_model_for`]: the per-action entry, else the shared
    /// default (except Expert plan review, which never inherits one).
    pub fn review_reasoning_for(
        &self,
        action: ReviewAction,
    ) -> Option<crate::headless::ReasoningLevel> {
        let action_level = self.review_reasonings.get(action.config_key()).copied();
        if action == ReviewAction::PlanPreflight {
            action_level
        } else {
            action_level.or(self.review_reasoning)
        }
    }

    /// The concurrency cap, or `None` when the check is disabled (`0`).
    pub fn agent_concurrency_limit(&self) -> Option<usize> {
        (self.max_concurrent_agents > 0).then_some(self.max_concurrent_agents)
    }

    /// The available-memory floor in MiB, or `None` when the check is
    /// disabled (`0`). A platform with no memory signal also skips the gate.
    pub fn low_memory_threshold_mb(&self) -> Option<u64> {
        (self.low_memory_warn_mb > 0).then_some(self.low_memory_warn_mb)
    }

    /// How long a needs-attention state survives before ageing out, or `None`
    /// when ageing is disabled (`0`).
    pub fn waiting_stale_threshold(&self) -> Option<Duration> {
        (self.waiting_stale_minutes > 0)
            .then(|| Duration::from_secs(self.waiting_stale_minutes * 60))
    }

    /// The two dormancy thresholds as `(idle, last_accessed)` durations, or
    /// `None` when either key is `0` — dormancy is an AND of both halves, so
    /// switching one off switches the whole check off rather than degrading
    /// it into "everything is dormant".
    pub fn dormant_thresholds(&self) -> Option<(Duration, Duration)> {
        if self.dormant_idle_minutes == 0 || self.dormant_last_accessed_hours == 0 {
            return None;
        }
        Some((
            Duration::from_secs(self.dormant_idle_minutes * 60),
            Duration::from_secs(self.dormant_last_accessed_hours * 3600),
        ))
    }
}

pub struct App {
    pub store: ProjectStore,
    pub store_path: PathBuf,
    pub db: Option<crate::db::AmfDb>,
    /// The `store_meta` version `store` was loaded at, when `db` is `Some`.
    /// `save` uses this for a version-checked (cross-process-safe) write and
    /// updates it after every save/reload — see `AMF_PLAN.md` Task 5 and
    /// `db::store::save_checked`. `None` when there is no DB (tests, or the
    /// JSON-file fallback), where there is nothing else to race against.
    pub(crate) store_version: Option<u64>,
    /// Set by `precall_confirm` when the user clears a pre-call notice; the
    /// re-dispatched `start_*` method consumes it to skip the gate and spawn.
    pub precall_cleared: Option<precall::PrecallAction>,
    /// Last path probed for git by the create-project dialog, so typing in
    /// the Name field does not re-run `git rev-parse` for an unchanged path.
    pub(crate) create_project_git_probe: Option<(PathBuf, Option<bool>)>,
    /// The one in-flight "new project from git URL" operation, created on
    /// first use. See `clone_project.rs`.
    pub(crate) clone_project: Option<clone_project::CloneProjectJob>,
    /// Set while an interface that saves review progress itself (and must
    /// report whether the save succeeded) drives the shared review actions, so
    /// their best-effort `persist_review_progress` does not write a second time.
    pub(crate) defer_review_progress_persist: bool,
    /// Stashed pre-call notice while the user is in the override manager it
    /// opened (`e`); restored when the manager closes.
    pub precall_return: Option<Box<precall::PendingPrecall>>,
    pub config: AppConfig,
    /// `config` as this process last read it from or wrote it to
    /// `config.json`. A save writes only what differs from this, so it cannot
    /// undo a setting another writer changed in the meantime.
    pub(crate) saved_config: AppConfig,
    pub active_extension: ExtensionConfig,
    pub theme: crate::theme::Theme,
    pub selection: Selection,
    pub mode: AppMode,
    pub(crate) diagram_worker: crate::mermaid::DiagramWorker,
    /// A plan interview temporarily parked while the user inspects the
    /// dashboard or one of its sessions. Unlike a persisted draft this keeps
    /// the live editor cursor and uncommitted answer text intact.
    pub paused_plan_interview: Option<PlanInterviewState>,
    /// Brief composed from a TODO, waiting for the create-feature wizard to
    /// hand over to the interview it seeds.
    ///
    /// It cannot ride on the wizard state: the wizard may detour through an
    /// `on_worktree_created` hook that rebuilds its launch from scratch. Taken
    /// by `start_plan_interview`, and only ever read when that launch carries a
    /// `todo_origin`, so an unrelated feature created afterwards cannot pick up
    /// a stale brief.
    pub pending_todo_plan_brief: Option<String>,
    /// Composer seed for a "start an agent in a new feature" spawn from a TODO,
    /// waiting for the create-feature wizard to finish. Kept here rather than on
    /// the wizard state for the same reason [`Self::pending_todo_plan_brief`]
    /// is: the wizard can detour through an `on_worktree_created` hook that
    /// rebuilds its launch from scratch. Taken unconditionally by
    /// `finish_feature_launch_with_resource_approval`, and only read when that
    /// launch carries a `todo_origin` and is not a plan run.
    pub pending_todo_spawn_prompt: Option<String>,
    /// Process-lifetime visibility for the project TODO scope. This is shared
    /// by every TODO view but intentionally resets whenever AMF starts.
    pub todo_project_visible: bool,
    /// Process-lifetime visibility for the global TODO scope. This is shared
    /// by every TODO view but intentionally resets whenever AMF starts.
    pub todo_global_visible: bool,
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
    /// Line offset into the Ctrl+Space menu when it is taller than the pane.
    pub leader_scroll: usize,
    /// Largest useful `leader_scroll` as of the last draw (draw is `&App`).
    pub leader_max_scroll: std::cell::Cell<usize>,
    pub leader_activated_at: Option<Instant>,
    pub last_view_activity_at: Option<Instant>,
    pub view_input_batch: Option<ViewInputBatch>,
    pub pending_inputs: Vec<PendingInput>,
    pub(crate) pending_automation_prompts: Vec<automation::PendingAutomationPrompt>,
    /// Feature IDs stopped from the dashboard (`x`) during this run. Their
    /// missing tmux session is expected, so opening a saved agent pane
    /// restarts and resumes directly instead of asking (see
    /// `open_stopped_session_dialog`). In-memory only: after an AMF restart a
    /// missing session is indistinguishable from a crash, which is exactly the
    /// case the recovery dialog is for.
    pub user_stopped_features: std::collections::HashSet<String>,
    /// Tmux "session:window" targets where compose interception is
    /// disabled and keys pass straight through to Claude Code.
    pub compose_direct_targets: std::collections::HashSet<String>,
    /// Unsent compose drafts keyed by tmux "session:window" target.
    pub compose_drafts: HashMap<String, ComposeDraft>,
    compose_clipboard_paste: Option<ComposeClipboardPaste>,
    compose_submit: Option<ComposeSubmit>,
    next_compose_clipboard_paste_id: u64,
    pub latest_prompt_cache: HashMap<String, String>,
    pub sidebar_model_cache: HashMap<String, String>,
    pub sidebar_plan_cache: HashMap<String, String>,
    /// The sidebar's "Current: <path>" line for each feature's effective
    /// plan (see `app::plan::resolve_effective_plan`); no entry means no
    /// plan resolved (or none has loaded yet). Populated by the
    /// background sidebar-load pipeline, never resolved on the render
    /// thread — the resolution touches the filesystem (an `is_file` check
    /// and, for a manual selection, a `canonicalize`), and `draw()` runs
    /// up to ~20x/sec while a pane is in view.
    pub sidebar_effective_plan_cache: HashMap<String, String>,
    pub codex_session_title_cache: HashMap<String, Option<String>>,
    pub codex_session_prompt_cache: HashMap<String, Option<String>>,
    pub codex_session_model_cache: HashMap<String, Option<String>>,
    pub codex_live_threads: HashMap<String, CodexLiveThreadState>,
    pub codex_sidebar_metadata_tx: std::sync::mpsc::Sender<CodexSidebarMetadataResult>,
    pub codex_sidebar_metadata_rx: std::sync::mpsc::Receiver<CodexSidebarMetadataResult>,
    pub codex_sidebar_metadata_inflight: std::collections::HashSet<String>,
    pub opencode_sidebar_cache: HashMap<String, opencode_storage::OpencodeSidebarData>,
    /// Rendered per-session active-TODO sidebar text, rebuilt on status sync and
    /// after any local mutation that affects it (completion, deletion,
    /// spawn) rather than on every frame. Resolving it runs two SQLite
    /// queries per TODO-referenced session, and
    /// `build_agent_sidebar_data` is rebuilt up to ~20x/sec in Viewing mode.
    pub active_todos_sidebar_cache: HashMap<String, String>,
    pub(crate) learning_runs: learning::runtime::LearningRuns,
    sidebar_runtime: sidebar_runtime::SidebarRuntime,
    pub usage: UsageManager,
    pub token_tracker: SessionTokenTracker,
    /// Transient context-window telemetry keyed by AMF session ID.
    pub context_states: HashMap<String, SessionContextState>,
    /// Transient context-hint dismissal state keyed by AMF session ID.
    pub(crate) context_hint_states: context_hints::ContextHintStates,
    pub context_collector: SessionContextCollector,
    pub(crate) session_status_bg: Option<Receiver<sync::SessionStatusBgResult>>,
    /// Background refresh and last-known values for the dashboard's open-PR
    /// badges. Rendering only reads `active_prs`; all `gh` calls happen on the
    /// worker behind `active_pr_bg`.
    pub(crate) active_pr_bg: Option<Receiver<sync::ActivePrSweep>>,
    /// When GitHub's GraphQL point budget was seen exhausted. The dashboard PR
    /// sweep stops until `GH_GRAPHQL_BACKOFF` has passed, because every call in
    /// the depleted window fails identically and retrying only keeps the budget
    /// at zero once it starts refilling.
    pub(crate) gh_graphql_limited_at: Option<Instant>,
    pub(crate) active_prs: HashMap<String, ActivePrStatus>,
    /// Last-known terminal (merged/closed) state per feature, keyed by feature
    /// id like `active_prs`. Unlike `active_prs`, this is a durable fact —
    /// merged and closed never revert — so it is seeded from
    /// `pr_terminal_state` at startup (`load_terminal_prs_from_db`) and every
    /// positive lookup is persisted back, rather than being purely
    /// session-lived like the open-PR cache.
    pub(crate) terminal_prs: HashMap<String, crate::github::TerminalPr>,
    /// Feature ids the terminal sweep has already confirmed have no
    /// merged/closed PR, keyed like `active_prs`/`terminal_prs`. Session-only
    /// (never persisted): unlike `terminal_prs`, a `NoPr` result is not a
    /// durable fact — the branch could still get a PR later — so this exists
    /// purely to stop `sync_active_prs_background` from re-querying the same
    /// settled "never had one" answer on every sweep. Cleared the moment a
    /// branch is observed open again (`apply_active_pr_updates`), so a PR
    /// that opens and later closes is re-checked rather than staying stuck on
    /// a stale negative answer.
    pub(crate) confirmed_no_terminal_pr: HashSet<String>,
    pub(crate) pr_review_work: pr_review::runtime::PrReviewWork,
    /// Background GitHub issue-page request and its cancellation boundary.
    pub(crate) issue_work: issue_fixer::IssueWork,
    pub(crate) issue_comment_work: issue_fixer::IssueCommentWork,
    /// Receiver for the background "all prompts" scan (leader-key latest-prompt
    /// menu). Reading and parsing every Claude/Codex/opencode transcript file
    /// for a session can be slow, so it runs off the UI thread; see
    /// `app::view::open_latest_prompt_from_view`.
    pub(crate) latest_prompt_menu_bg: Option<Receiver<view::LatestPromptScanResult>>,
    /// Receiver for the background AI-adaptive plan-interview round (a
    /// headless harness call). Carries the round number alongside the
    /// result so a late-arriving response can be matched or discarded. See
    /// `app::plan_interview::poll_plan_interview_ai_bg`.
    pub plan_interview_ai_bg: Option<Receiver<(usize, Result<String>)>>,
    /// The `plan_interview_mcp` validation error last reported, so an invalid
    /// config warns once rather than on every pass and every precall replay.
    /// Cleared when the config validates, so breaking it again warns again.
    pub(crate) plan_interview_mcp_warned: Option<String>,
    /// Receiver for the final plan-synthesis headless call. Kept separate
    /// from adaptive rounds so late results can only be applied to the
    /// matching loading phase.
    pub plan_interview_synthesis_bg: Option<Receiver<Result<String>>>,
    /// Receiver for the optional Expert review of a draft plan. Separate again
    /// so a late review can never be mistaken for a synthesis result and
    /// overwrite the plan it was only meant to comment on.
    pub plan_interview_critique_bg: Option<Receiver<Result<String>>>,
    /// Receiver for a user-directed plan revision. This pass is separate from
    /// synthesis and critique because it may inspect the feature workdir with
    /// read-only tools and is only valid for its dedicated loading phase.
    pub plan_interview_directed_feedback_bg: Option<Receiver<Result<String>>>,
    /// Receiver for the optional context-isolated investigation workflow. Its
    /// worker runs one fresh read-only context per focus, then a separate
    /// no-tools merge context, and returns the proposed revised plan alongside
    /// the focuses whose investigator failed.
    pub plan_interview_investigation_bg:
        Option<Receiver<Result<crate::plan_interview::PlanInvestigationOutcome>>>,
    /// A PR Triage pane stashed by `pr_review_toggle_to_session` (`P`) while the
    /// user watches the linked fix session; `leader+P` pops it back without a
    /// re-fetch. See [`PrReviewReturn`].
    pub pr_review_return: Option<PrReviewReturn>,
    /// AI review findings handed to PR Triage for fixing that haven't been
    /// installed yet (PR Triage may still be loading). See
    /// [`App::apply_pending_local_findings`].
    pub(crate) pr_review_pending_local: Option<pr_review::PendingLocalFindings>,
    /// Receiver for the background review-memory lookback bootstrap (fetch +
    /// distill pass). See `app::pr_review::run_review_memory_bootstrap`.
    pub review_memory_bootstrap_bg: Option<Receiver<pr_review::BootstrapProgress>>,
    /// Receiver for the background review-memory compact pass ("prevent
    /// review-memory rot"). See `app::pr_review::run_review_memory_compact`.
    pub review_memory_compact_bg: Option<Receiver<pr_review::CompactProgress>>,
    /// The origin picker + resolved doc path a background compact pass
    /// ([`Self::review_memory_compact_bg`]) was started against, kept alive
    /// independent of `self.mode` — in particular across
    /// `cancel_review_memory_compact` (`esc`), which restores `self.mode` to
    /// `PrPicker` well before the background pass finishes. Without this, a
    /// `Done` arriving after a cancel would find `self.mode` is no longer
    /// `ReviewMemoryCompactRunning` and has nowhere to land the proposed
    /// rewrite — mirrors the pending AI-review run origin. `Some` exactly while
    /// `review_memory_compact_bg` is `Some`; both are cleared together once
    /// `Done` is processed.
    pub review_memory_compact_pending: Option<CompactRunState>,
    /// Receiver for the background "summarize with AI" run started from the
    /// memory-add dialog (`s`). See `app::pr_review::pr_review_start_memory_ai_summary`.
    pub memory_ai_summary_bg: Option<Receiver<pr_review::MemoryAiSummaryDone>>,
    pub(crate) ai_review_run: pr_review::runtime::AiReviewRun,
    pub(crate) review_question_work: review_questions::Work,
    pub(crate) model_analysis_work: model_analysis::Work,
    /// The mode to restore when the AI Review pane closes (`esc`/`q`),
    /// stashed by `open_ai_review_from_triage` so returning from a review
    /// started inside PR Triage lands back in that same pane rather than the
    /// dashboard. `None` for the other three entry points (dashboard, an
    /// agent session, the PR picker), which close straight to `Normal` —
    /// matching how `close_pr_review` already behaves for those.
    pub ai_review_return_to: Option<Box<AppMode>>,
    /// Experimental PR workspace. The active engine remains in `mode`; the
    /// other views are parked here so existing handlers keep their ownership.
    pub(crate) pr_workspace: Option<pr_workspace::PrWorkspace>,
    /// Memoized result of [`Self::ai_review_finding_fix_costs`], valid only
    /// while its key still matches the open AI Review pane. Recomputed lazily
    /// (a triage DB load + a full cached-review JSON parse + a sibling query
    /// per correlated finding) when the key changes, instead of every render
    /// tick; cleared on pane (re)open and when an `A` run lands.
    pub(crate) ai_review_fix_cost_cache:
        Option<(ai_review::AiReviewFixCostKey, Vec<Option<String>>)>,
    /// Background PR Triage refresh kicked off only after GitHub confirms an
    /// AI Review post. Separate from the PR fetch slot so it can update a stashed
    /// pane without changing the current AI Review mode.
    pub ai_review_triage_refresh_bg: Option<Receiver<Result<pr_review::PrReview>>>,
    /// PR/workdir identity paired with `ai_review_triage_refresh_bg`.
    pub ai_review_triage_refresh_pending: Option<AiReviewTriageRefresh>,
    /// Background `codex debug models` catalog probe
    /// (`codex_config::spawn_cli_catalog_probe`), kicked off when a Codex
    /// model picker opens with no preset rows (cache and availability table
    /// both empty — a fresh install). Polled and, once it resolves, merged
    /// into whichever model picker is still open; never awaited inline, since
    /// that would block the event loop on the `codex` CLI.
    pub codex_models_cli_bg: Option<Receiver<Option<Vec<String>>>>,
    /// Memoized `GhCli::current_user` result for the session, so opening or
    /// refreshing the PR picker doesn't repeat the `gh api user` call every
    /// time. `None` = not yet resolved; `Some(None)` = resolution was
    /// attempted and failed (e.g. `gh` unauthenticated).
    pub gh_current_user: Option<Option<String>>,
    pub scroll_offset: usize,
    pub session_filter: SessionFilter,
    pub throbber_state: throbber_widgets_tui::ThrobberState,
    pub thinking_features: std::collections::HashSet<String>,
    /// Why each stopped session is stopped, keyed by `feature.tmux_session`
    /// (the same granularity thinking status is tracked at). Deliberately
    /// **not** persisted: it is rebuilt from hook events and starts empty
    /// after a restart, so no `amf.db` migration is involved. Composed with
    /// the persisted `ProjectStatus` at render time.
    pub attention: HashMap<String, attention::AttentionRecord>,
    /// Rate-limits the opencode `capture-pane` thinking fallback: the
    /// 500ms thinking tick must not spawn a subprocess per feature.
    pub(crate) opencode_thinking_pane_cache: HashMap<String, (Instant, Option<bool>)>,
    pub ipc_thinking_sessions: std::collections::HashSet<String>,
    /// Features whose dispatched review-fix prompt is being watched, keyed by
    /// `feature.tmux_session`. Populated when a finished review pastes the
    /// feedback prompt into an agent session; drained by `sync_thinking_status`
    /// once that session is observed thinking-then-idle, at which point a
    /// "fixes ready — re-review?" notification is raised.
    pub awaiting_review_fixes: HashMap<String, AwaitingReviewFix>,
    /// A file the reviewer asked to open in `$VISUAL`/`$EDITOR`. Set by the
    /// review viewer and drained by the main loop, which suspends the TUI,
    /// runs the editor, and restores the screen.
    pub pending_editor: Option<PendingEditorOpen>,
    pub ipc_tool_sessions: std::collections::HashSet<String>,
    /// Feature-session IDs currently reported as thinking by hook/plugin IPC.
    /// Unlike `ipc_thinking_sessions`, this distinguishes agent windows that
    /// share one feature-level tmux session.
    pub ipc_thinking_feature_sessions: std::collections::HashSet<String>,
    /// Feature-session IDs currently running a tool, kept separate from
    /// thinking so either signal can independently keep a session working.
    pub ipc_tool_feature_sessions: std::collections::HashSet<String>,
    pub summary_state: SummaryState,
    pub summary_rx: Option<std::sync::mpsc::Receiver<(String, Result<String, anyhow::Error>)>>,
    pub tmux: Box<dyn TmuxOps>,
    /// Which features run in a container; shared with the tmux decorator.
    pub sandbox: std::sync::Arc<crate::container::tmux::SandboxRegistry>,
    pub worktree: std::sync::Arc<dyn WorktreeOps>,
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
    pub(crate) evidence_work: screenshots::EvidenceWork,
    /// Persistent tmux control-mode client observing session lifecycle;
    /// None in tests (status polling falls back to its interval).
    pub tmux_observer: Option<crate::tmux_observer::TmuxObserver>,
    pub(crate) ipc_chatty_log_count: u64,
    pub(crate) ipc_chatty_log_window_start: Instant,
    pub last_file_notification_count: usize,
    pub last_file_notification_fingerprint: Option<u64>,
    pub vscode_available: bool,
    /// VS Code launches whose window has not been attributed yet. Ownership is
    /// resolved on a background thread seconds after the launch, so a feature
    /// stopped in the meantime has nothing to kill; these entries let the stop
    /// hand the job to the resolver instead of silently skipping it. See
    /// [`editor_ops::PendingEditorLaunch`].
    pub(crate) pending_editor_launches: Vec<editor_ops::PendingEditorLaunch>,
    pane_runtime: pane_runtime::PaneRuntime,
    pub(crate) harness_check_tx: Sender<HarnessCheckResult>,
    harness_check_rx: Receiver<HarnessCheckResult>,
    /// The Remote Control companion-app server: `Some` exactly while it is
    /// running, on a dedicated thread with its own tokio runtime. Toggled
    /// on/off by the user only — never started automatically — per the
    /// on-demand server-lifecycle decision in
    /// `docs/backlog/remote-control-companion-app-plan.md`. Drained every
    /// main-loop tick by `poll_remote_server_bg` like the other `poll_*_bg`
    /// background jobs.
    pub remote_server: Option<crate::remote_server::RemoteServerHandle>,
    /// The server's actual bound address, set on `Started` and cleared on
    /// `Stopped` (`poll_remote_server_bg`). `None` while the server is
    /// starting up or not running — `start_pairing` needs this to build the
    /// pairing QR when no `remote_public_url` is configured.
    pub remote_server_addr: Option<SocketAddr>,
    /// When `Ctrl+Space Q` was pressed while the server was off or still
    /// starting: the pairing dialog opens once it is listening and the
    /// dashboard or a session view is on screen (`poll_remote_server_bg`).
    /// Forgotten if the start fails or the request outlives
    /// `remote_server::PAIRING_REQUEST_WINDOW`.
    pub pairing_requested: Option<std::time::Instant>,
    /// What the local Tailscale looks like, for the pairing QR's address
    /// and its setup steps — see `app/remote_tailscale.rs`.
    pub remote_tailscale: remote_tailscale::RemoteTailscaleState,
    /// The background `/health` check of the pairing address — see
    /// `app/remote_reach.rs`.
    pub remote_reach: remote_reach::RemoteReachState,
    /// Web Push to paired phones — see `app/remote_push.rs`.
    pub remote_push: remote_push::RemotePushState,
    /// The authorized-device table the server checks bearer tokens
    /// against — see `app/remote_server.rs`.
    pub remote_devices: remote_server::AuthorizedDevicesCache,
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

struct ComposeSubmit {
    target: String,
    rx: Receiver<Result<(), String>>,
}

struct SidebarLoadResult {
    tmux_session: String,
    signature: u64,
    changed: bool,
    latest_prompt: Option<String>,
    model_text: Option<String>,
    opencode_sidebar: Option<opencode_storage::OpencodeSidebarData>,
    plan_text: Option<String>,
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
        if self.pr_workspace.as_ref().is_some_and(|workspace| {
            workspace.owns_mode(&self.mode) && workspace.ai_running(&self.ai_review_run)
        }) {
            return true;
        }
        if self
            .review_questions()
            .is_some_and(|q| q.open && q.request.is_some())
        {
            return true;
        }
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
            // is in flight (see `ui::dialogs::draw_ai_review`).
            AppMode::AiReview(_) => self.ai_review_run.is_pending(),
            // Full-screen loading/running views: `redraw_signature()` only
            // hashes the mode's discriminant, not its stage, so without this
            // the throbber only advances on the rare frame something else
            // (a progress message, `Done`) forces a redraw — for the long
            // stretch in between (the blocking `gh`/`claude` call) it just
            // sits frozen, reading as "nothing is happening" even though the
            // background thread is working.
            AppMode::PrReviewLoading(_)
            | AppMode::PrInvestigationLoading(_)
            | AppMode::ReviewMemoryBootstrapRunning(_)
            | AppMode::ReviewMemoryCompactRunning(_)
            | AppMode::AiReviewRunning(_) => true,
            // Animates the loading frame's throbber and elapsed-time display
            // while plan-interview AI work runs in the background.
            AppMode::ModelAnalysis(state) => {
                matches!(state.status, model_analysis::Status::Loading)
            }
            AppMode::PlanInterview(state) => matches!(
                state.phase,
                PlanInterviewPhase::AiLoading
                    | PlanInterviewPhase::SynthesisLoading
                    | PlanInterviewPhase::CritiqueLoading
                    | PlanInterviewPhase::DirectedFeedbackLoading
                    | PlanInterviewPhase::InvestigationLoading
            ),
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
        self.leader_scroll.hash(&mut hasher);
        self.message.hash(&mut hasher);
        self.toasts.len().hash(&mut hasher);
        self.pending_inputs.len().hash(&mut hasher);
        self.tmux_cursor.hash(&mut hasher);
        self.todo_project_visible.hash(&mut hasher);
        self.todo_global_visible.hash(&mut hasher);
        if let Some(workspace) = &self.pr_workspace {
            workspace.tab.hash(&mut hasher);
            workspace.focus.hash(&mut hasher);
            workspace.expanded.hash(&mut hasher);
            workspace.help.hash(&mut hasher);
            workspace.help_scroll.hash(&mut hasher);
        }

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
            view.pending_vibe_mode
                .as_ref()
                .map(std::mem::discriminant)
                .hash(&mut hasher);
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
            state.submit_in_progress.hash(&mut hasher);
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

    /// Returns the read end of the wakeup pipe so the main loop can use
    /// `libc::poll()` to wake immediately when a snapshot is ready.
    pub fn view_wakeup_rx_fd(&self) -> Option<RawFd> {
        self.pane_runtime
            .view_snapshot_wakeup_rx
            .as_ref()
            .map(|fd| fd.as_raw_fd())
    }

    /// Write end of the wakeup pipe. Other event sources (e.g. the IPC
    /// listener) write a byte to it so the main loop's `poll()` returns
    /// immediately instead of waiting out its timeout.
    pub fn view_wakeup_tx(&self) -> Arc<OwnedFd> {
        self.pane_runtime.view_snapshot_tx.wakeup_fd()
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
        if let Some(until) = self.pane_runtime.view_display_frozen_until {
            if Instant::now() < until {
                return (false, false);
            }
            self.pane_runtime.view_display_frozen_until = None;
        }

        let current_target = self.current_view_snapshot_target();
        let mut pane_changed = false;
        let mut cursor_changed = false;

        // Latest-wins mailbox: at most one (merged) snapshot is pending.
        let pending = self.pane_runtime.view_snapshot_tx.take_pending();
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

    pub fn log_startup(&mut self) {
        self.log_info("amf", "AMF started".to_string());
        self.log_debug("amf", format!("DB path: {}", self.store_path.display()));
        self.log_debug(
            "amf",
            format!("Projects loaded: {}", self.store.projects.len()),
        );
    }

    #[allow(dead_code)] // exercised only by unit tests
    pub(super) fn build_latest_prompt_cache(store: &ProjectStore) -> HashMap<String, String> {
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

        let dedicated_opencode_session = match &self.mode {
            AppMode::PrReview(state) if state.workdir == feature.workdir => {
                pr_review::pr_triage_session_index_named(
                    feature,
                    state.fix_target,
                    &state.dedicated_session_label,
                )
                .map(|si| &feature.sessions[si])
                .filter(|session| session.kind == SessionKind::Opencode)
            }
            _ => None,
        };
        let request = dedicated_opencode_session.map_or_else(
            || SidebarLoadRequest::from_feature(feature),
            |session| SidebarLoadRequest::from_opencode_session(feature, session),
        );
        if !self
            .sidebar_runtime
            .pending_sidebar_loads
            .insert(request.tmux_session.clone())
        {
            return;
        }

        let previous_signature = self
            .sidebar_runtime
            .sidebar_load_signatures
            .get(&request.tmux_session)
            .copied();
        let tx = self.sidebar_runtime.sidebar_load_tx.clone();
        self.sidebar_runtime
            .sidebar_load_executor
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
        while let Ok(result) = self.sidebar_runtime.sidebar_load_rx.try_recv() {
            self.sidebar_runtime
                .pending_sidebar_loads
                .remove(&result.tmux_session);
            self.sidebar_runtime
                .sidebar_load_signatures
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

            if let Some(plan_text) = result.plan_text {
                self.sidebar_effective_plan_cache
                    .insert(result.tmux_session.clone(), plan_text);
            } else {
                self.sidebar_effective_plan_cache
                    .remove(&result.tmux_session);
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
        self.sidebar_runtime
            .pending_sidebar_loads
            .remove(tmux_session);
        self.sidebar_runtime
            .sidebar_load_signatures
            .remove(tmux_session);
        self.latest_prompt_cache.remove(tmux_session);
        self.sidebar_model_cache.remove(tmux_session);
        self.sidebar_plan_cache.remove(tmux_session);
        self.sidebar_effective_plan_cache.remove(tmux_session);
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
        if crate::custom_agent::reconcile(&mut self.store, &global_ext)
            && let Err(error) = self.save()
        {
            self.log_warn(
                "session",
                format!("Could not save custom agent recognition: {error}"),
            );
        }
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

    /// The effective template for a headless prompt on the feature at
    /// `repo` / `workdir`, plus where it came from. Consults, nearest-first:
    /// the feature-scope `prompt_overrides` row (keyed by `workdir`), the
    /// repo's `amf.json` `prompt_overrides`, the global-scope row, then the
    /// built-in default. Read fresh each call so a hand-edit to `amf.json` or
    /// an override saved from the manager takes effect on the next run.
    ///
    /// Returned without interpolation so a caller that renders on a worker
    /// thread can resolve here (on the UI thread, where `self` lives) and
    /// [`crate::prompts::render_template`] there.
    pub(crate) fn resolve_headless_template(
        &self,
        id: crate::prompts::PromptId,
        harness: &AgentKind,
        repo: &Path,
        workdir: &Path,
    ) -> (String, crate::prompts::PromptSource) {
        let db_view = self
            .db
            .as_ref()
            .and_then(|db| db.load_prompt_overrides().ok());
        let project = crate::prompts::project::load_from_repo(repo);
        let layers = crate::prompts::PromptLayers {
            feature_workdir: workdir.to_str(),
            db: db_view.as_ref(),
            project: Some(&project),
        };
        let (text, source) = crate::prompts::resolve_template_layered(id, harness, &layers);
        if source.is_override() {
            crate::debug::log_to_file(
                crate::debug::LogLevel::Debug,
                "prompts",
                &format!("{} resolved from {}", id.as_str(), source.label()),
            );
        }
        (text.into_owned(), source)
    }

    /// [`Self::resolve_headless_template`] followed by interpolation of `ctx`.
    /// The common path: a call site that builds its context and dispatches on
    /// the UI thread.
    pub(crate) fn resolve_headless_prompt(
        &self,
        id: crate::prompts::PromptId,
        harness: &AgentKind,
        repo: &Path,
        workdir: &Path,
        ctx: &crate::prompts::PromptContext,
    ) -> String {
        let (text, _source) = self.resolve_headless_template(id, harness, repo, workdir);
        crate::prompts::render_template(&text, ctx)
    }

    /// The batched-review size budget in estimated prompt tokens for a run of
    /// `harness` against `repo`: the project's `amf.json`
    /// `review_prompt_budget_tokens`, else the global
    /// `AppConfig::review_prompt_budget_tokens`, else the built-in per-harness
    /// default. A configured `0` is honored (disables pre-send splitting).
    pub(crate) fn review_prompt_budget(&self, repo: &Path, harness: &AgentKind) -> usize {
        self.extension_for_repo(repo)
            .review_prompt_budget_tokens
            .or(self.config.review_prompt_budget_tokens)
            .unwrap_or_else(|| crate::headless::default_prompt_budget_tokens(harness))
    }

    /// The validated `plan_interview_mcp` for a pass run by `harness`, or
    /// `None` when it is unset, the harness is not Claude (the only one whose
    /// isolation mode can load an explicit MCP config), or the config is
    /// invalid — which is logged and surfaced, never fatal: the pass runs
    /// exactly as it would without MCP.
    pub(crate) fn plan_interview_mcp(
        &mut self,
        harness: &AgentKind,
    ) -> Option<crate::headless::HeadlessMcp> {
        let config = self.config.plan_interview_mcp.as_ref()?;
        if *harness != AgentKind::Claude {
            self.log_info(
                "plan_interview",
                format!(
                    "plan_interview_mcp ignored: {} passes cannot load MCP servers",
                    harness.display_name()
                ),
            );
            return None;
        }
        match config.validate() {
            Ok(mcp) => {
                self.plan_interview_mcp_warned = None;
                Some(mcp)
            }
            Err(reason) => {
                if self.plan_interview_mcp_warned.as_ref() != Some(&reason) {
                    self.log_warn("plan_interview", reason.clone());
                    self.message = Some(format!("MCP not loaded for plan interview: {reason}"));
                    self.plan_interview_mcp_warned = Some(reason);
                }
                None
            }
        }
    }

    pub(crate) fn allowed_agents_for_repo(&self, repo: &Path) -> Vec<AgentKind> {
        self.installed_agents(self.extension_for_repo(repo).allowed_agents())
    }

    /// The global choices alone, for a project that does not exist yet and so
    /// has no `amf.json` to override them.
    pub(crate) fn globally_allowed_agents(&self) -> Vec<AgentKind> {
        self.installed_agents(load_global_extension_config().allowed_agents())
    }

    fn installed_agents(&self, allowed: Vec<AgentKind>) -> Vec<AgentKind> {
        if self.store.available_harnesses.is_empty() {
            allowed
        } else {
            allowed
                .into_iter()
                .filter(|a| self.store.available_harnesses.contains(a))
                .collect()
        }
    }

    pub(crate) fn repo_for_project_path(&self, path: &Path) -> PathBuf {
        // A blank form field names no folder; don't ask git about one.
        if path.as_os_str().is_empty() {
            return PathBuf::new();
        }
        self.worktree
            .repo_root(path)
            .unwrap_or_else(|_| path.to_path_buf())
    }

    /// Resolve the configured review-memory path override for `repo`, in
    /// precedence order:
    ///
    /// 1. `{repo}/.amf/config.json` `review_memory_path` (project scope).
    /// 2. The `"extension"` block's `review_memory_path` in the global
    ///    `~/.config/amf/config.json` (set via the config wizard's global
    ///    scope).
    /// 3. The top-level `review_memory_path` key in that same global
    ///    `~/.config/amf/config.json` — an older setting
    ///    (`AppConfig::review_memory_path`) that predates the `"extension"`
    ///    block gaining this field. It has no wizard UI of its own, and a
    ///    value set here is silently shadowed by #2 if both are present in
    ///    the file.
    ///
    /// None of the above set means the caller should fall back to
    /// [`review_memory::DEFAULT_REVIEW_MEMORY_PATH`].
    pub(crate) fn configured_review_memory_path(&self, repo: &Path) -> Option<String> {
        self.extension_for_repo(repo)
            .review_memory_path
            .or_else(|| self.config.review_memory_path.clone())
    }

    /// Both review-memory doc paths for `repo`: its own doc (via
    /// [`Self::configured_review_memory_path`]) and the user's cross-project
    /// one. Every review-memory flow resolves through this, so a scope toggle
    /// is just a lookup rather than a second round of config plumbing, and
    /// the reviewer reads the same file the add/bootstrap/compact flows write.
    /// An ignored project doc in a linked worktree resolves to the main
    /// checkout's copy ([`review_memory::resolve_project_memory_path`]).
    pub(crate) fn review_memory_paths(&self, repo: &Path) -> review_memory::ReviewMemoryPaths {
        let (project, worktree_local) = review_memory::resolve_project_memory_path(
            repo,
            self.configured_review_memory_path(repo).as_deref(),
        );
        review_memory::ReviewMemoryPaths {
            project,
            global: review_memory::global_review_memory_path(
                self.config.global_review_memory_path.as_deref(),
            ),
            worktree_local,
        }
    }

    pub(crate) fn allowed_agents_for_project_path(&self, path: &Path) -> Vec<AgentKind> {
        let repo = self.repo_for_project_path(path);
        self.allowed_agents_for_repo(&repo)
    }

    pub(crate) fn allows_agent_for_repo(&self, repo: &Path, agent: &AgentKind) -> bool {
        let allowed = self.allowed_agents_for_repo(repo);
        allowed.contains(agent)
    }

    /// The preferred harness if allowed, else the first allowed one.
    ///
    /// With nothing allowed, returns `preferred` itself at index 0. That is
    /// only a placeholder: callers never index the installed list with the
    /// result, and every launch path re-checks `allows_agent_for_repo` (or
    /// returns early on an empty list) before using the harness.
    pub(crate) fn normalize_agent_for_repo(
        &self,
        repo: &Path,
        preferred: &AgentKind,
    ) -> (AgentKind, usize) {
        let allowed = self.allowed_agents_for_repo(repo);
        let selected =
            AgentKind::preferred_in(&allowed, preferred).unwrap_or_else(|| preferred.clone());
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
            AgentKind::Codex => crate::codex::CodexLauncher::check_available(),
            _ => crate::headless::HeadlessRunner::check_available(kind),
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
        let path_is_git = match &self.create_project_git_probe {
            Some((cached, result)) if *cached == path => *result,
            _ => {
                let result = self.probe_path_is_git(&path);
                self.create_project_git_probe = Some((path.clone(), result));
                result
            }
        };
        if let AppMode::CreatingProject(state) = &mut self.mode {
            state.agent = agent;
            state.agent_index = agent_index;
            state.path_is_git = path_is_git;
        }
    }

    /// `Some(is_git)` for an existing directory, `None` otherwise.
    pub(crate) fn probe_path_is_git(&self, path: &std::path::Path) -> Option<bool> {
        path.is_dir().then(|| self.worktree.repo_root(path).is_ok())
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
        if !mode.supports_agent(agent) {
            anyhow::bail!(
                "Codex does not support Vibeless diff review. Use Claude/Opencode, or switch to Vibe or SuperVibe."
            );
        }
        Ok(())
    }

    /// Persists `self.store`, reporting whether it actually saved or hit a
    /// concurrent-writer conflict, without turning a conflict into an `Err`
    /// -- see `save`, which wraps this for TUI call sites that just want to
    /// bail with a message, `save_reapplying` for call sites whose change can
    /// be re-applied onto the refreshed store, and `gui_contract::GuiHandle`,
    /// which wraps it to report `GuiErrorKind::Conflict` instead of a bare
    /// internal error.
    ///
    /// Cross-process-safe when backed by a real DB (`AMF_PLAN.md` Task 5):
    /// the save only applies if nothing else -- the GUI, another AMF
    /// process, or a stale reload -- has saved since this `App` last loaded
    /// or saved successfully. On a detected conflict the in-memory store is
    /// refreshed from disk (see `adopt_store_from_disk`) before returning, so
    /// the caller's next action sees current state instead of repeating the
    /// same conflict; the change that triggered this save is discarded, not
    /// merged, which is the honest outcome for a full-replace store with no
    /// per-field merge logic (see `db::store::save`'s doc comment).
    ///
    /// Any `(pi, fi, si)` the caller held from before the call may be stale
    /// after `Ok(false)`; re-resolve by stable id before indexing again.
    pub(crate) fn save_reporting_conflict(&mut self) -> Result<bool> {
        self.sync_sandbox_registry();
        if let Some(db) = &self.db {
            let Some(expected) = self.store_version else {
                // No known baseline -- either genuinely the first save this
                // process has made, or (several tests' pattern) a DB attached
                // to an `App` built via `new_for_test` after the fact, with
                // no `load_store_versioned` call to establish one. Either
                // way there is nothing yet to conflict *with*: save
                // unconditionally once and adopt the version that save
                // committed (read inside its own transaction, so it cannot
                // be a later writer's), so every save after this one is
                // protected.
                self.store_version = Some(db.save_store(&self.store)?);
                return Ok(true);
            };
            return match db.save_store_checked(&self.store, expected)? {
                crate::db::store::SaveOutcome::Saved { new_version } => {
                    self.store_version = Some(new_version);
                    Ok(true)
                }
                crate::db::store::SaveOutcome::Conflict { .. } => {
                    // One snapshot, not `load_store` plus the version the
                    // rejected save saw: a write landing between the two
                    // would pair this data with an older version and make
                    // the next save conflict for no reason.
                    let (store, version) = db.load_store_versioned()?;
                    self.adopt_store_from_disk(store, version);
                    Ok(false)
                }
            };
        }
        // Fallback for tests: write JSON to store_path if set. No other
        // writer exists in this path, so there is nothing to race against.
        if !self.store_path.as_os_str().is_empty() {
            self.store.save(&self.store_path)?;
        }
        Ok(true)
    }

    /// Save, and on a cross-process conflict re-apply this caller's change
    /// onto the refreshed store and try again, instead of discarding it.
    ///
    /// For changes that can be described by stable id -- "append this
    /// session to feature X", "tag session Y with TODO Z" -- and that follow
    /// a side effect (a tmux window, a worktree) which would otherwise be
    /// left running with no store record. `reapply` runs against the store
    /// just reloaded from disk; it returns `false`, without mutating, when
    /// its target no longer exists there, which ends the attempt with
    /// [`ReapplyOutcome::TargetGone`] so the caller can undo its side effect.
    ///
    /// After anything but `Saved` the store holds the other writer's state
    /// without this change. An `Err` (a failed DB write, not a conflict)
    /// leaves the unsaved change in memory, as `save` does.
    pub(crate) fn save_reapplying(
        &mut self,
        mut reapply: impl FnMut(&mut ProjectStore) -> bool,
    ) -> Result<ReapplyOutcome> {
        let mut attempts = 0;
        loop {
            if self.save_reporting_conflict()? {
                return Ok(ReapplyOutcome::Saved);
            }
            attempts += 1;
            if attempts >= SAVE_REAPPLY_ATTEMPTS {
                return Ok(ReapplyOutcome::Conflict);
            }
            if !reapply(&mut self.store) {
                return Ok(ReapplyOutcome::TargetGone);
            }
        }
    }

    /// Adopt a newer store written by another process (the GUI, another
    /// TUI, or `amf` automation) if one has landed since this `App` last
    /// loaded or saved. Called from the TUI's event loop so the next save
    /// starts from current state instead of conflicting on it.
    ///
    /// Only in modes whose state refers to the store by name or id -- the
    /// dashboard and the session view -- because other modes can hold
    /// `(pi, fi)` indices a reload would silently re-point. Anywhere else
    /// the check waits for the next call.
    pub fn refresh_store_if_changed_elsewhere(&mut self) -> Result<bool> {
        if !matches!(
            self.mode,
            AppMode::Normal | AppMode::Viewing(_) | AppMode::ForkingFeature(_)
        ) {
            return Ok(false);
        }
        let Some(db) = &self.db else {
            return Ok(false);
        };
        let Some(known) = self.store_version else {
            return Ok(false);
        };
        if db.current_store_version()? == known {
            return Ok(false);
        }
        let (store, version) = db.load_store_versioned()?;
        self.adopt_store_from_disk(store, version);
        Ok(true)
    }

    /// Replace the in-memory store with one just loaded from disk, keeping
    /// the dashboard selection on the same project/feature/session by id --
    /// an index into the old store can name a different row, or none, in
    /// the new one. A selection whose row is gone falls back to its nearest
    /// surviving ancestor, then to the first project.
    pub(crate) fn adopt_store_from_disk(&mut self, store: ProjectStore, version: u64) {
        let ids = self.selection_ids();
        self.store = store;
        crate::custom_agent::reconcile(&mut self.store, &load_global_extension_config());
        self.store_version = Some(version);
        self.selection = self.selection_for_ids(ids);
    }

    fn selection_ids(&self) -> (Option<String>, Option<String>, Option<String>) {
        let (pi, fi, si) = match self.selection {
            Selection::Project(pi) => (pi, None, None),
            Selection::Feature(pi, fi) => (pi, Some(fi), None),
            Selection::Session(pi, fi, si) => (pi, Some(fi), Some(si)),
        };
        let project = self.store.projects.get(pi);
        let feature = fi.and_then(|fi| project.and_then(|p| p.features.get(fi)));
        let session = si.and_then(|si| feature.and_then(|f| f.sessions.get(si)));
        (
            project.map(|p| p.id.clone()),
            feature.map(|f| f.id.clone()),
            session.map(|s| s.id.clone()),
        )
    }

    fn selection_for_ids(
        &self,
        (project_id, feature_id, session_id): (Option<String>, Option<String>, Option<String>),
    ) -> Selection {
        if let Some((pi, fi, si)) = session_id
            .as_deref()
            .and_then(|id| self.session_indices_by_id(id))
        {
            return Selection::Session(pi, fi, si);
        }
        if let Some((pi, fi)) = feature_id
            .as_deref()
            .and_then(|id| self.feature_indices_by_id(id))
        {
            return Selection::Feature(pi, fi);
        }
        let pi = project_id
            .as_deref()
            .and_then(|id| self.store.projects.iter().position(|p| p.id == id))
            .unwrap_or(0);
        Selection::Project(pi)
    }

    pub fn save(&mut self) -> Result<()> {
        if self.save_reporting_conflict()? {
            Ok(())
        } else {
            anyhow::bail!(SAVE_CONFLICT_MESSAGE)
        }
    }

    pub fn start_theme_picker(&mut self) {
        let entries = ThemePickerEntry::build();
        let original_theme = self.config.theme;

        // If the active theme lives inside a group, land directly on its
        // second screen with that theme pre-selected instead of forcing the
        // user to drill back in.
        let mut selected = 0;
        let mut group = None;
        for (i, entry) in entries.iter().enumerate() {
            match entry {
                ThemePickerEntry::Theme(name) if *name == original_theme => {
                    selected = i;
                    break;
                }
                ThemePickerEntry::Group { label, themes } if themes.contains(&original_theme) => {
                    selected = i;
                    let group_selected = themes
                        .iter()
                        .position(|t| *t == original_theme)
                        .unwrap_or(0);
                    group = Some(ThemePickerGroupState {
                        label,
                        themes: themes.clone(),
                        selected: group_selected,
                    });
                    break;
                }
                _ => {}
            }
        }

        self.mode = AppMode::ThemePicker(ThemePickerState {
            selected,
            entries,
            original_theme,
            previewed: original_theme,
            group,
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

    /// Shared persisted history plus this process's not-yet-flushed entries.
    /// Reading does not flush, clear or otherwise mutate either log.
    pub(crate) fn recent_debug_log_entries(&self) -> anyhow::Result<Vec<LogEntry>> {
        let limit = self.debug_log.max_entries();
        let mut entries = if let Some(db) = &self.db {
            let mut entries = db.load_recent_log(limit)?;
            entries.extend(self.pending_debug_log_entries.iter().cloned());
            entries
        } else {
            self.debug_log.entries().iter().cloned().collect()
        };
        // Pending entries can interleave with another process's DB writes.
        entries.sort_by_key(|entry| entry.timestamp);
        if entries.len() > limit {
            entries.drain(..entries.len() - limit);
        }
        Ok(entries)
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
        self.message = Some(Self::logged_error_message(&message.into()));
    }

    /// The one place the status-line "Error: " prefix and the debug-log
    /// pointer are added. A prefix already on `detail` is dropped first so
    /// callers that pre-prefix cannot produce "Error: Error: ...", and the
    /// pointer is omitted when the message already says what to do next.
    pub(crate) fn logged_error_message(detail: &str) -> String {
        let detail = strip_error_prefix(detail);
        let lower = detail.to_lowercase();
        let words: Vec<&str> = lower
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .collect();
        let has_word = |w: &str| words.contains(&w);
        let actionable = has_word("retry")
            || has_word("press")
            || lower.contains("is written to")
            || lower.contains("debug log");
        if actionable {
            format!("Error: {detail}")
        } else {
            let sep = if detail.ends_with(['.', '!', '?']) {
                ""
            } else {
                "."
            };
            format!("Error: {detail}{sep} Press D for the debug log.")
        }
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

    pub fn save_config(&mut self) {
        if let Err(e) = self.try_save_config() {
            // Fire-and-forget callers (theme/background) can't surface errors; log only.
            self.log_error("config", format!("save_config failed: {e:#}"));
        }
    }

    pub fn try_save_config(&mut self) -> anyhow::Result<()> {
        if self.store_path.as_os_str().is_empty() {
            return Ok(());
        }
        let config_path = crate::project::amf_config_dir().join("config.json");
        setup::save_config_at(&config_path, &self.saved_config, &self.config)?;
        self.saved_config = self.config.clone();
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
    selected_plan_path: Option<PathBuf>,
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
            selected_plan_path: feature.selected_plan_path.clone(),
        }
    }

    fn from_opencode_session(feature: &Feature, session: &FeatureSession) -> Self {
        let preferred_session_id = session
            .token_usage_source
            .as_ref()
            .filter(|source| source.provider == crate::token_tracking::TokenUsageProvider::Opencode)
            .map(|source| source.id.clone());
        Self {
            tmux_session: feature.tmux_session.clone(),
            workdir: feature.workdir.clone(),
            preferred_session_kind: Some(SessionKind::Opencode),
            preferred_session_id,
            selected_plan_path: feature.selected_plan_path.clone(),
        }
    }

    fn signature(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.workdir.hash(&mut hasher);
        self.preferred_session_id.hash(&mut hasher);
        session_kind_signature(self.preferred_session_kind.as_ref()).hash(&mut hasher);
        sidebar_prompt_input_signature(&self.workdir).hash(&mut hasher);
        sidebar_plan_input_signature(&self.workdir, self.selected_plan_path.as_deref())
            .hash(&mut hasher);

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
                plan_text: None,
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
        let plan_text = crate::app::plan::plan_sidebar_display_text(
            &self.workdir,
            self.selected_plan_path.as_deref(),
        );

        SidebarLoadResult {
            tmux_session: self.tmux_session,
            signature,
            changed,
            latest_prompt,
            model_text,
            opencode_sidebar,
            plan_text,
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

fn sidebar_plan_input_signature(workdir: &Path, selected_plan_path: Option<&Path>) -> u64 {
    let mut hasher = DefaultHasher::new();
    hash_path_metadata(
        &mut hasher,
        workdir.join(crate::app::plan::DEFAULT_PLAN_FILE),
    );
    selected_plan_path.hash(&mut hasher);
    if let Some(selected) = selected_plan_path {
        let candidate = if selected.is_absolute() {
            selected.to_path_buf()
        } else {
            workdir.join(selected)
        };
        hash_path_metadata(&mut hasher, candidate);
    }
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

/// Strip a leading `Error:` (any case, with its spacing) so a message is never
/// prefixed twice.
pub(crate) fn strip_error_prefix(message: &str) -> &str {
    let mut rest = message.trim_start();
    while let Some(tail) = rest
        .get(..6)
        .filter(|p| p.eq_ignore_ascii_case("error:"))
        .map(|_| &rest[6..])
    {
        rest = tail.trim_start();
    }
    rest
}

#[cfg(test)]
mod sidebar_load_executor_tests {
    use super::sidebar_runtime::SidebarLoadExecutor;
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

fn expand_tilde(raw: &str, home: Option<std::path::PathBuf>) -> std::path::PathBuf {
    let rest = if raw == "~" {
        Some("")
    } else {
        raw.strip_prefix("~/")
    };
    match (rest, home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => std::path::PathBuf::from(raw),
    }
}

#[cfg(test)]
mod projects_root_tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn default_is_code_under_home() {
        let config = AppConfig::default();
        assert_eq!(config.projects.projects_root, None);
        let expected = dirs::home_dir().map_or(PathBuf::from("~/code"), |h| h.join("code"));
        assert_eq!(config.projects_root(), expected);
    }

    #[test]
    fn tilde_expansion() {
        let home = Some(PathBuf::from("/home/u"));
        assert_eq!(
            expand_tilde("~/src", home.clone()),
            PathBuf::from("/home/u/src")
        );
        assert_eq!(expand_tilde("~", home.clone()), PathBuf::from("/home/u"));
        assert_eq!(
            expand_tilde("/abs/p", home.clone()),
            PathBuf::from("/abs/p")
        );
        assert_eq!(expand_tilde("~other/x", home), PathBuf::from("~other/x"));
        assert_eq!(expand_tilde("~/src", None), PathBuf::from("~/src"));
    }

    #[test]
    fn blank_value_falls_back_to_default() {
        let mut config = AppConfig::default();
        config.projects.projects_root = Some("  ".into());
        assert_eq!(config.projects_root(), AppConfig::default().projects_root());
    }

    #[test]
    fn custom_value_round_trips_and_unset_is_omitted() {
        let unset = serde_json::to_value(AppConfig::default()).unwrap();
        assert!(unset["projects"].get("projects_root").is_none());

        let mut config = AppConfig::default();
        config.projects.projects_root = Some("/work/repos".into());
        let json = serde_json::to_string(&config).unwrap();
        let back: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.projects.projects_root.as_deref(), Some("/work/repos"));
        assert_eq!(back.projects_root(), PathBuf::from("/work/repos"));
    }

    #[test]
    fn save_config_at_persists_only_when_set() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let before = AppConfig::default();
        let mut after = before.clone();
        after.projects.projects_root = Some("/work/repos".into());
        crate::app::setup::save_config_at(&path, &before, &after).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("/work/repos"));
        let loaded: AppConfig = serde_json::from_str(&text).unwrap();
        assert_eq!(
            loaded.projects.projects_root.as_deref(),
            Some("/work/repos")
        );
    }
}
