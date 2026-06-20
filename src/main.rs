#![allow(dead_code)]

mod app;
mod automation;
mod claude;
mod codex;
mod codex_config;
mod db;
mod debug;
mod diff;
mod editor;
mod extension;
mod fswatch;
mod handlers;
mod highlight;
mod http_client;
mod ipc;
mod markdown;
mod perf;
mod pi;
mod project;
mod prompt_library;
mod summary;
mod theme;
mod tmux;
mod tmux_observer;
mod token_tracking;
mod traits;
mod transcript;
mod ui;
mod upgrade;
mod usage;
mod worktree;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use crossterm::{
    cursor::SetCursorStyle,
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Paragraph};
use std::io;
use std::os::unix::io::RawFd;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use app::{App, load_config};
use tmux::TmuxManager;

#[derive(Parser, Debug)]
#[command(name = "amf")]
#[command(version, disable_version_flag = true)]
#[command(about = "Run many AI coding agents in parallel", long_about = None)]
struct Cli {
    #[arg(short = 'V', long = "version")]
    version: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Upgrade amf to the latest release
    Upgrade,
    /// Run machine-friendly automation actions against a running AMF instance
    Automation {
        #[command(subcommand)]
        command: AutomationCommands,
    },
    /// Send a notification to the running AMF instance via the
    /// IPC socket. Reads JSON from stdin. Used by hook scripts.
    #[command(hide = true)]
    Notify,
    /// Send a notification and wait for an IPC response JSON.
    /// Used by review hooks that require a decision.
    #[command(hide = true)]
    NotifyWait {
        /// Timeout in milliseconds while waiting for reply.
        #[arg(long, default_value_t = 120000)]
        timeout_ms: u64,
    },
    /// Write a custom session status into the AMF database.
    /// Used by hook scripts for custom sessions.
    #[command(name = "set-status", hide = true)]
    SetStatus {
        session_id: String,
        status_text: String,
    },
}

#[derive(Subcommand, Debug)]
enum AutomationCommands {
    /// Create a single AMF project from JSON input
    CreateProject {
        /// Read request JSON from a file. Omit or pass `-` to read stdin.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Override the JSON payload and perform validation only.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Timeout in milliseconds while waiting for AMF to reply.
        #[arg(long, default_value_t = 120000)]
        timeout_ms: u64,
    },
    /// Create a single feature/worktree inside an existing AMF project from JSON input
    CreateFeature {
        /// Read request JSON from a file. Omit or pass `-` to read stdin.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Override the JSON payload and perform validation only.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Timeout in milliseconds while waiting for AMF to reply.
        #[arg(long, default_value_t = 120000)]
        timeout_ms: u64,
    },
    /// Create one project with many parallel feature worktrees from JSON input
    CreateBatchFeatures {
        /// Read request JSON from a file. Omit or pass `-` to read stdin.
        #[arg(long)]
        file: Option<PathBuf>,
        /// Override the JSON payload and perform validation only.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Timeout in milliseconds while waiting for AMF to reply.
        #[arg(long, default_value_t = 120000)]
        timeout_ms: u64,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.version && cli.command.is_none() {
        println!("amf {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    if let Some(Commands::Upgrade) = cli.command {
        return upgrade::upgrade();
    }

    if let Some(Commands::Automation { command }) = cli.command {
        return run_automation_command(command);
    }

    if let Some(Commands::Notify) = cli.command {
        use std::io::Read;
        let mut payload = String::new();
        std::io::stdin().read_to_string(&mut payload)?;
        let payload = payload.trim();
        if payload.is_empty() {
            return Ok(());
        }
        let socket = ipc::socket_path();
        // Propagate error so hook scripts get a non-zero exit
        // code and can fall back to file-based delivery.
        ipc::send(&socket, payload)?;
        return Ok(());
    }

    if let Some(Commands::NotifyWait { timeout_ms }) = cli.command {
        use std::io::Read;
        let mut payload = String::new();
        std::io::stdin().read_to_string(&mut payload)?;
        let payload = payload.trim();
        if payload.is_empty() {
            return Ok(());
        }
        let socket = ipc::socket_path();
        let reply = ipc::send_wait(&socket, payload, Duration::from_millis(timeout_ms))?;
        println!(
            "{}",
            serde_json::to_string(&reply).unwrap_or_else(|_| "{}".to_string())
        );
        return Ok(());
    }

    if let Some(Commands::SetStatus {
        session_id,
        status_text,
    }) = cli.command
    {
        let db_path = project::db_path();
        let db = db::AmfDb::open_or_seed(&db_path, &project::global_db_path())?;
        let store = db.load_store()?;
        let feature_id = store
            .projects
            .iter()
            .flat_map(|project| &project.features)
            .find_map(|feature| {
                feature
                    .sessions
                    .iter()
                    .any(|session| session.id == session_id)
                    .then(|| feature.id.clone())
            })
            .context("session not found in store")?;
        db.upsert_session_status(&session_id, &feature_id, &status_text, None)?;
        // Best-effort IPC push so a running AMF instance updates immediately.
        let json = serde_json::json!({
            "type": "session-status",
            "session_id": session_id,
            "message": status_text,
        });
        let _ = ipc::send(&ipc::socket_path(), &json.to_string());
        return Ok(());
    }

    let config = load_config();
    TmuxManager::configure_control_mode(config.tmux_control_mode);

    if let Err(e) = TmuxManager::check_available() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }

    debug::install_panic_hook();
    debug::rotate_log_if_needed();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture,
        SetCursorStyle::SteadyBlock
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| {
        draw_loading(
            frame,
            "Loading AMF...",
            startup_loading_hint(Duration::ZERO),
        )
    })?;

    let result = panic::catch_unwind(AssertUnwindSafe(|| -> Result<()> {
        cleanup_global_hooks();
        app::App::cleanup_stale_thinking_files();

        let db_path = project::db_path();
        let mut app = App::new(db_path)?;
        app.log_startup();

        // Check for VS Code availability in the background so it doesn't
        // block startup.  The result is applied once ready.
        let vscode_check_rx = {
            let (tx, rx) = std::sync::mpsc::channel::<bool>();
            std::thread::spawn(move || {
                let available = std::process::Command::new("code")
                    .arg("--version")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok();
                let _ = tx.send(available);
            });
            rx
        };

        if !app.store.has_any_harnesses() {
            app.open_harness_setup(true);
        }

        // Start IPC socket server for push-based hook notifications.
        let socket = ipc::socket_path();
        match ipc::start_with_wakeup(&socket, Some(app.view_wakeup_tx())) {
            Ok(guard) => {
                app.log_info("ipc", format!("Socket listening at {}", socket.display()));
                app.ipc = Some(guard);
            }
            Err(e) => {
                app.log_warn(
                    "ipc",
                    format!(
                        "Could not start IPC socket, \
                         falling back to file polling: {e}"
                    ),
                );
            }
        }

        run_loop(&mut terminal, &mut app, vscode_check_rx)
    }));

    debug::flush_log_writer();
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        SetCursorStyle::DefaultUserShape,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    match result {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!(
            "AMF panicked; see {}",
            debug::global_log_path().display()
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn startup_loading_message(
    sync_statuses_pending: bool,
    session_status_pending: bool,
    usage_pending: bool,
    claude_hooks_pending: bool,
    opencode_plugins_pending: bool,
    sidebar_warm_pending: bool,
    session_status_bg_pending: bool,
) -> &'static str {
    if sync_statuses_pending {
        "Checking managed sessions..."
    } else if session_status_pending || session_status_bg_pending {
        "Reading session status..."
    } else if usage_pending {
        "Refreshing usage..."
    } else if claude_hooks_pending {
        "Refreshing Claude hooks..."
    } else if opencode_plugins_pending {
        "Refreshing opencode plugins..."
    } else if sidebar_warm_pending {
        "Preparing dashboard..."
    } else {
        "Loading AMF..."
    }
}

fn startup_loading_hint(elapsed: Duration) -> &'static str {
    const HINT_ROTATION_SECS: u64 = 4;
    const HINTS: &[&str] = &[
        "Tip: Ctrl+Space then R refreshes pane sizing if the view looks wrong.",
        "Tip: Press D on the dashboard to open the debug log.",
        "Tip: Ctrl+Space opens view-mode commands while inside a session.",
    ];

    let index = (elapsed.as_secs() / HINT_ROTATION_SECS) as usize % HINTS.len();
    HINTS[index]
}

fn startup_loading_pending(
    sync_statuses_pending: bool,
    session_status_pending: bool,
    claude_hooks_pending: bool,
    opencode_plugins_pending: bool,
) -> bool {
    sync_statuses_pending
        || session_status_pending
        || claude_hooks_pending
        || opencode_plugins_pending
}

fn startup_sidebar_can_warm(session_status_inflight: bool, usage_stats_inflight: bool) -> bool {
    !session_status_inflight && !usage_stats_inflight
}

fn draw_loading(frame: &mut Frame, message: &str, hint: &str) {
    let area = frame.area();
    frame.render_widget(Block::default(), area);

    let width = area.width.min(48);
    let height = 7u16.min(area.height);
    let rect = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    );

    let text = vec![
        Line::from(Span::styled(
            "AMF",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(Color::Gray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            hint.to_string(),
            Style::default().fg(Color::DarkGray),
        )),
    ];
    let paragraph = Paragraph::new(text).alignment(Alignment::Center);
    frame.render_widget(paragraph, rect);
}

fn read_json_input(file: Option<&PathBuf>) -> Result<String> {
    use std::io::Read;

    let mut payload = String::new();
    match file {
        Some(path) if path.as_os_str() != "-" => {
            payload = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
        }
        _ => {
            std::io::stdin()
                .read_to_string(&mut payload)
                .context("Failed to read automation JSON from stdin")?;
        }
    }

    let payload = payload.trim();
    if payload.is_empty() {
        anyhow::bail!("Automation request payload is empty");
    }

    Ok(payload.to_string())
}

fn run_automation_command(command: AutomationCommands) -> Result<()> {
    match command {
        AutomationCommands::CreateProject {
            file,
            dry_run,
            timeout_ms,
        } => {
            let payload = read_json_input(file.as_ref())?;
            let mut request: automation::CreateProjectRequest =
                serde_json::from_str(&payload).context("Invalid create_project JSON payload")?;
            if dry_run {
                request.dry_run = true;
            }

            let socket = ipc::socket_path();
            let outbound = serde_json::to_string(&request.ipc_payload())?;
            let reply = ipc::send_wait(&socket, &outbound, Duration::from_millis(timeout_ms))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&reply).unwrap_or_else(|_| "{}".to_string())
            );
            Ok(())
        }
        AutomationCommands::CreateFeature {
            file,
            dry_run,
            timeout_ms,
        } => {
            let payload = read_json_input(file.as_ref())?;
            let mut request: automation::CreateFeatureRequest =
                serde_json::from_str(&payload).context("Invalid create_feature JSON payload")?;
            if dry_run {
                request.dry_run = true;
            }

            let socket = ipc::socket_path();
            let outbound = serde_json::to_string(&request.ipc_payload())?;
            let reply = ipc::send_wait(&socket, &outbound, Duration::from_millis(timeout_ms))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&reply).unwrap_or_else(|_| "{}".to_string())
            );
            Ok(())
        }
        AutomationCommands::CreateBatchFeatures {
            file,
            dry_run,
            timeout_ms,
        } => {
            let payload = read_json_input(file.as_ref())?;
            let mut request: automation::CreateBatchFeaturesRequest =
                serde_json::from_str(&payload)
                    .context("Invalid create_batch_features JSON payload")?;
            if dry_run {
                request.dry_run = true;
            }

            let socket = ipc::socket_path();
            let outbound = serde_json::to_string(&request.ipc_payload())?;
            let reply = ipc::send_wait(&socket, &outbound, Duration::from_millis(timeout_ms))?;
            println!(
                "{}",
                serde_json::to_string_pretty(&reply).unwrap_or_else(|_| "{}".to_string())
            );
            Ok(())
        }
    }
}

/// Merges AMF thinking-detection hooks into ~/.claude/settings.json.
///
/// Uses Claude Code's PreToolUse / Stop hooks to touch / remove a
/// sentinel file at /tmp/amf-thinking/<AMF_SESSION> so the dashboard
/// can show a throbber without polling tmux pane content.
///
/// The function is idempotent: it only appends entries when they are
/// not already present, and silently skips on any I/O error.
/// Removes any AMF-managed hook entries that were previously
/// injected into the global ~/.claude/settings.json.
/// Hook management now happens in the per-worktree local
/// settings via `ensure_notification_hooks`.
fn cleanup_global_hooks() {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return,
    };
    let settings_path = std::path::PathBuf::from(&home)
        .join(".claude")
        .join("settings.json");
    let extra_cmds = [format!("{home}/.config/amf/notify.sh")];
    let extra: Vec<&str> = extra_cmds.iter().map(|s| s.as_str()).collect();
    cleanup_hooks_at(&settings_path, &extra);
}

/// Inner logic for `cleanup_global_hooks`, factored out for
/// testability.  `extra_cmds` are host-specific command
/// strings (e.g. absolute paths) to remove in addition to
/// the static AMF commands.
pub fn cleanup_hooks_at(settings_path: &std::path::Path, extra_cmds: &[&str]) {
    use serde_json::Value;

    let mut root: Value = match std::fs::read_to_string(settings_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(v) => v,
        None => return,
    };

    let Some(hooks_obj) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return;
    };

    // Static commands previously injected by AMF globally.
    let static_cmds: &[&str] = &[
        "[ -n \"$AMF_SESSION\" ] && mkdir -p /tmp/amf-thinking && touch \"/tmp/amf-thinking/$AMF_SESSION\" || true",
        "[ -n \"$AMF_SESSION\" ] && rm -f \"/tmp/amf-thinking/$AMF_SESSION\" || true",
        "/tmp/debug-hook.sh",
    ];

    let mut changed = false;
    for event_arr in hooks_obj.values_mut() {
        let Some(arr) = event_arr.as_array_mut() else {
            continue;
        };
        let before = arr.len();
        arr.retain(|entry| {
            !entry["hooks"].as_array().is_some_and(|hs| {
                hs.iter().any(|h| {
                    h["command"]
                        .as_str()
                        .is_some_and(|c| static_cmds.contains(&c) || extra_cmds.contains(&c))
                })
            })
        });
        if arr.len() != before {
            changed = true;
        }
    }

    // Drop empty event arrays.
    hooks_obj.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));

    if !changed {
        return;
    }

    if let Ok(serialized) = serde_json::to_string_pretty(&root) {
        let _ = std::fs::write(settings_path, serialized + "\n");
    }
}

/// Minimal deadline registry for the event loop: every periodic
/// subsystem contributes its next-due instant and the loop sleeps in
/// `poll()` until the earliest one (or an fd event: stdin, snapshot
/// wakeup, IPC wakeup). New periodic work must register here, making
/// its scheduling cost explicit — zero work while idle is the default.
#[derive(Default)]
struct LoopDeadlines {
    next: Option<Instant>,
}

impl LoopDeadlines {
    fn register(&mut self, due_at: Instant) {
        self.next = Some(match self.next {
            Some(current) => current.min(due_at),
            None => due_at,
        });
    }

    fn register_after(&mut self, since: Instant, interval: Duration) {
        self.register(since + interval);
    }

    /// Time to sleep until the earliest deadline. The floor guards
    /// against a busy spin when a due task is deferred (e.g. background
    /// sync during active view input); the cap bounds how stale
    /// anything not represented by a deadline or fd can get.
    fn sleep_duration(&self, now: Instant, floor: Duration, cap: Duration) -> Duration {
        self.next
            .map(|due_at| due_at.saturating_duration_since(now))
            .unwrap_or(cap)
            .clamp(floor, cap)
    }
}

fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    vscode_check_rx: std::sync::mpsc::Receiver<bool>,
) -> Result<()> {
    let mut last_sync = std::time::Instant::now();
    let mut last_thinking_sync = std::time::Instant::now();
    let mut last_file_log_flush = std::time::Instant::now();
    let mut last_log_rotation_check = std::time::Instant::now();
    let mut last_notification_prune = std::time::Instant::now();
    // Clear out notification files abandoned by previous runs before the
    // first scan, so a directory grown large over days does not slow the
    // initial reconciliation.
    app.prune_stale_notifications();
    let mut last_usage_debug: Option<(Option<i64>, Option<i64>, u64, u64)> = None;
    let mut last_claude_usage_debug: Option<String> = None;
    let mut last_resize: Option<(u16, u16, String, String)> = None;
    let mut force_redraw = true;
    let startup_grace_until = Instant::now() + app.config.input_request_wait_duration();
    let mut startup_task_spacing_until = Instant::now();
    let mut startup_sync_statuses_pending = true;
    let mut startup_session_status_pending = true;
    let mut startup_usage_pending = true;
    let mut startup_claude_hooks_pending = true;
    let mut startup_opencode_plugins_pending = true;
    let mut startup_sidebar_warm_pending = true;
    const ANIMATED_REDRAW_INTERVAL: Duration = Duration::from_millis(125);
    const VIEW_IDLE_REFRESH_QUIET_PERIOD: Duration = Duration::from_millis(150);
    // Thinking sync is event-driven via the filesystem watcher; this
    // is only the safety-net cadence when the watcher is running.
    const THINKING_SYNC_FALLBACK_INTERVAL: Duration = Duration::from_secs(2);
    // Status sync is event-driven via the tmux observer; this is only
    // the safety-net cadence when the observer is running.
    const STATUSES_SYNC_FALLBACK_INTERVAL: Duration = Duration::from_secs(30);
    let wakeup_rx_fd: Option<RawFd> = app.view_wakeup_rx_fd();
    let startup_loading_started_at = Instant::now();
    let mut last_view_refresh_request = Instant::now();
    // Backoff for pane-size drift corrections: if drift reappears right
    // after we fixed it, something else (another AMF instance, a user
    // attached elsewhere) is also resizing the pane, and fighting it
    // every few seconds garbles the agent's screen on each SIGWINCH.
    const DRIFT_CORRECTION_BACKOFF: Duration = Duration::from_secs(30);
    let mut last_drift_correction: Option<Instant> = None;
    // Terminal resizes arrive as a stream while a window is dragged or
    // re-tiled; forwarding each one to tmux lands a SIGWINCH mid-frame
    // in the agent pane, and every reflow can bake blended rows into
    // its scrollback. Wait for the viewport to hold still before
    // resizing the pane.
    const VIEWPORT_RESIZE_DEBOUNCE: Duration = Duration::from_millis(250);
    let mut last_viewport_dims: Option<(u16, u16)> = None;
    let mut viewport_stable_since = Instant::now();
    // Periodic re-anchor bounce for Claude panes. Two-phase so the main
    // thread never sleeps: shrink the pane one row, then restore it a
    // later iteration once `VIEW_REANCHOR_BOUNCE_DWELL` has passed. The
    // restore tuple is `(session, window, cols, full_rows, shrunk_at)`.
    let mut last_reanchor_bounce = Instant::now();
    let mut pending_reanchor_restore: Option<(String, String, u16, u16, Instant)> = None;
    let mut last_statuses_sync = std::time::Instant::now();
    let mut last_redraw_signature = app.redraw_signature();

    loop {
        // Carried over from the end of the previous iteration so the
        // signature is hashed only once per loop.
        let loop_state_signature = last_redraw_signature;
        let is_viewing = matches!(app.mode, app::AppMode::Viewing(_));
        // The compose box keeps the live pane visible behind it, so the
        // pane-refresh paths must stay active while composing too.
        let pane_live = is_viewing || matches!(app.mode, app::AppMode::Compose(_));
        let animating = app.has_visible_animation();

        let size = terminal.size()?;
        let visible_rows = size.height.saturating_sub(3);
        app.viewport_cols = size.width;
        app.viewport_rows = visible_rows;
        app.viewport_total_rows = size.height;
        if last_viewport_dims != Some((size.width, size.height)) {
            last_viewport_dims = Some((size.width, size.height));
            viewport_stable_since = Instant::now();
        }

        app.throbber_state.calc_next();
        if app.tick_toasts() {
            force_redraw = true;
        }

        if matches!(app.mode, app::AppMode::RunningHook(_))
            && let Err(e) = app.poll_running_hook()
        {
            app.show_error(e);
        }

        if matches!(app.mode, app::AppMode::DeletingFeatureInProgress(_))
            && let Err(e) = app.poll_deleting_feature()
        {
            app.show_error(e);
        }

        if matches!(app.mode, app::AppMode::DiffReviewPrompt(_))
            && let Err(e) = app.poll_diff_review_explanation()
        {
            app.show_error(e);
        }

        if matches!(app.mode, app::AppMode::SyntaxLanguagePicker(_))
            && let Err(e) = app.poll_syntax_language_picker()
        {
            app.show_error(e);
        }

        if matches!(app.mode, app::AppMode::HarnessSetup(_)) {
            app.poll_harness_checks();
        }

        if !app.background_deletions.is_empty()
            && let Err(e) = app.poll_background_deletions()
        {
            app.show_error(e);
        }

        if !app.background_hooks.is_empty()
            && let Err(e) = app.poll_background_hooks()
        {
            app.show_error(e);
        }

        if app.has_active_sidebar() {
            force_redraw |= app.poll_codex_sidebar_metadata();
        }

        if app.session_status_bg.is_some() {
            let started_at = Instant::now();
            if app.poll_session_status_bg() {
                app.perf
                    .record_duration("startup.sync_session_status_applied", started_at.elapsed());
                force_redraw = true;
            }
        }

        // Apply the one-shot VS Code availability check when it resolves.
        if let Ok(available) = vscode_check_rx.try_recv() {
            app.vscode_available = available;
        }

        if let Some(alert) = debug::take_user_alert() {
            app.push_toast_info(alert);
            force_redraw = true;
        }

        if app.leader_active && app.leader_timed_out() {
            app.deactivate_leader();
            force_redraw = true;
        }

        let startup_tasks_pending = startup_sync_statuses_pending
            || startup_session_status_pending
            || startup_usage_pending
            || startup_claude_hooks_pending
            || startup_opencode_plugins_pending
            || startup_sidebar_warm_pending;
        // Usage parsing and sidebar warming remain startup-owned so periodic
        // refreshes do not overlap them, but they run behind the interactive
        // dashboard rather than extending the blocking loading screen.
        let startup_loading = startup_loading_pending(
            startup_sync_statuses_pending,
            startup_session_status_pending,
            startup_claude_hooks_pending,
            startup_opencode_plugins_pending,
        );

        let poll_duration = if startup_loading {
            Duration::ZERO
        } else {
            let now = Instant::now();
            let mut deadlines = LoopDeadlines::default();
            deadlines.register_after(last_sync, Duration::from_secs(5));
            let thinking_fallback = if app.fs_watcher.is_some() {
                THINKING_SYNC_FALLBACK_INTERVAL
            } else {
                Duration::from_millis(500)
            };
            deadlines.register_after(last_thinking_sync, thinking_fallback);
            deadlines.register_after(last_file_log_flush, Duration::from_secs(1));
            if !is_viewing {
                let statuses_fallback = if app.tmux_observer.is_some() {
                    STATUSES_SYNC_FALLBACK_INTERVAL
                } else {
                    Duration::from_secs(5)
                };
                deadlines.register_after(last_statuses_sync, statuses_fallback);
            }
            if pane_live {
                deadlines
                    .register_after(last_view_refresh_request, app::VIEW_DRIFT_RESEED_INTERVAL);
            }
            if animating {
                deadlines.register_after(now, ANIMATED_REDRAW_INTERVAL);
            }
            if app.leader_active
                && let Some(activated_at) = app.leader_activated_at
            {
                deadlines.register(
                    activated_at + Duration::from_secs(app.config.leader_timeout_seconds.max(1)),
                );
            }
            // The view-mode resize check is not fd-driven (libc::poll cannot
            // observe SIGWINCH), so keep its worst-case latency at 250ms;
            // outside the libc::poll path crossterm's own poll wakes on
            // resize. Compose mode shares the libc::poll path.
            let cap = if pane_live {
                Duration::from_millis(250)
            } else {
                Duration::from_millis(500)
            };
            deadlines.sleep_duration(now, Duration::from_millis(50), cap)
        };
        let mut handled_user_events = false;

        // In view mode, multiplex stdin and the snapshot-wakeup pipe so we
        // unblock the moment a new frame arrives rather than waiting for the
        // full poll_duration.  Outside view mode (or when no pipe is
        // available) fall back to the standard crossterm path.
        let has_terminal_events = if pane_live && !startup_loading {
            if let Some(wfd) = wakeup_rx_fd {
                // crossterm may already hold buffered events internally, in
                // which case fd 0 shows no data — never sleep in libc::poll
                // while events are pending.
                if event::poll(Duration::ZERO)? {
                    true
                } else {
                    let mut fds = [
                        libc::pollfd {
                            fd: 0,
                            events: libc::POLLIN,
                            revents: 0,
                        },
                        libc::pollfd {
                            fd: wfd,
                            events: libc::POLLIN,
                            revents: 0,
                        },
                    ];
                    let timeout_ms = poll_duration.as_millis() as libc::c_int;
                    unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout_ms) };
                    if fds[1].revents & libc::POLLIN != 0 {
                        // Drain fully (read end is non-blocking) so bytes
                        // accumulated while not polling can't keep the
                        // pipe permanently readable.
                        let mut buf = [0u8; 1024];
                        while unsafe { libc::read(wfd, buf.as_mut_ptr() as *mut _, buf.len()) } > 0
                        {
                        }
                    }
                    event::poll(Duration::ZERO)?
                }
            } else {
                event::poll(poll_duration)?
            }
        } else {
            event::poll(poll_duration)?
        };

        if has_terminal_events {
            let mut events = vec![event::read()?];

            // Drain all pending events before drawing once — without
            // this, fast typing or key repeat in compose mode pays a
            // full frame draw per keystroke and falls behind.
            if pane_live || startup_loading {
                while event::poll(Duration::ZERO)? {
                    events.push(event::read()?);
                }
            }

            for ev in events {
                if startup_loading {
                    match ev {
                        Event::Key(key) => {
                            if key.kind == KeyEventKind::Release {
                                continue;
                            }
                            if matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                                || (matches!(key.code, KeyCode::Char('c'))
                                    && key.modifiers.contains(KeyModifiers::CONTROL))
                            {
                                app.should_quit = true;
                                force_redraw = true;
                            }
                        }
                        Event::Resize(_, _) => {
                            last_resize = None;
                            force_redraw = true;
                        }
                        _ => {}
                    }
                    continue;
                }

                handled_user_events = true;
                match ev {
                    Event::Key(key) => {
                        if key.kind == KeyEventKind::Release {
                            continue;
                        }
                        if pane_live {
                            app.note_view_activity();
                        }
                        app.perf.increment_counter("event.key");
                        app.perf.note_input_for_next_draw();
                        let started_at = Instant::now();
                        if let Err(e) = handlers::handle_key(app, key, visible_rows) {
                            app.show_error(e);
                        }
                        app.perf
                            .record_duration("main.handle_key", started_at.elapsed());
                    }
                    Event::Mouse(mouse) => {
                        if pane_live {
                            app.note_view_activity();
                        }
                        app.perf.increment_counter("event.mouse");
                        let started_at = Instant::now();
                        if let Err(e) = handlers::handle_mouse(app, mouse, visible_rows) {
                            app.show_error(e);
                        }
                        app.perf
                            .record_duration("main.handle_mouse", started_at.elapsed());
                    }
                    Event::Paste(text) => {
                        if pane_live {
                            app.note_view_activity();
                        }
                        app.perf.increment_counter("event.paste");
                        app.perf.note_input_for_next_draw();
                        let started_at = Instant::now();
                        if let Err(e) = handlers::handle_paste(app, &text) {
                            app.show_error(e);
                        }
                        app.perf
                            .record_duration("main.handle_paste", started_at.elapsed());
                    }
                    Event::Resize(_, _) => {
                        last_resize = None;
                        force_redraw = true;
                    }
                    _ => {}
                }
            }
        }

        if app.should_quit {
            app.flush_pending_debug_log_entries();
            return Ok(());
        }

        if app.has_pending_view_input() {
            if let Err(e) = app.flush_view_input_batch() {
                app.show_error(e);
            }
        }

        if pane_live
            && !startup_loading
            && !handled_user_events
            && last_view_refresh_request.elapsed() >= app::VIEW_DRIFT_RESEED_INTERVAL
            && app
                .last_view_activity_at
                .is_none_or(|last| last.elapsed() >= VIEW_IDLE_REFRESH_QUIET_PERIOD)
        {
            // The pane can be resized behind our back (another client
            // attaching to the session, a second AMF instance, etc.).
            // last_resize only tracks sizes we set ourselves, so
            // reconcile against the actual pane size here; otherwise
            // every snapshot is parsed at the wrong geometry and the
            // view stays garbled until restart.
            let drift_target = match &app.mode {
                app::AppMode::Viewing(view) => Some((
                    view.session.clone(),
                    view.window.clone(),
                    ui::viewing_main_width(view, size.width),
                )),
                _ => None,
            };
            if let Some((session, window, expected_cols)) = drift_target {
                let expected_rows = size.height.saturating_sub(1);
                if let Ok(actual) = TmuxManager::pane_size(&session, &window)
                    && actual != (expected_cols, expected_rows)
                    && !TmuxManager::has_attached_client(&session)
                {
                    let in_backoff = last_drift_correction
                        .is_some_and(|at| at.elapsed() < DRIFT_CORRECTION_BACKOFF);
                    if in_backoff {
                        app.log_debug(
                            "tmux",
                            format!(
                                "pane size drift for {session}:{window} (actual {}x{}, expected {expected_cols}x{expected_rows}) within correction backoff; leaving it alone",
                                actual.0, actual.1
                            ),
                        );
                    } else {
                        app.log_warn(
                            "tmux",
                            format!(
                                "pane size drift for {session}:{window} (actual {}x{}, expected {expected_cols}x{expected_rows}), resizing",
                                actual.0, actual.1
                            ),
                        );
                        let _ = TmuxManager::resize_pane(
                            &session,
                            &window,
                            expected_cols,
                            expected_rows,
                        );
                        last_drift_correction = Some(Instant::now());
                    }
                }
            }
            app.request_view_snapshot_refresh();
            last_view_refresh_request = Instant::now();
        }

        let defer_background_sync = app.should_defer_view_background_sync();

        // Feature-status reconciliation is event-driven via the tmux
        // observer (%sessions-changed); the interval is only a fallback.
        // Without an observer, keep the legacy 5s cadence.
        let statuses_fallback = if app.tmux_observer.is_some() {
            STATUSES_SYNC_FALLBACK_INTERVAL
        } else {
            Duration::from_secs(5)
        };
        if !handled_user_events
            && !is_viewing
            && !startup_tasks_pending
            && (app.take_sessions_dirty() || last_statuses_sync.elapsed() >= statuses_fallback)
        {
            let started_at = Instant::now();
            app.sync_statuses();
            app.perf
                .record_duration("sync.statuses", started_at.elapsed());
            last_statuses_sync = Instant::now();
            force_redraw = true;
        }

        if !handled_user_events
            && last_sync.elapsed() >= Duration::from_secs(5)
            && !defer_background_sync
            && !startup_tasks_pending
        {
            // Kick off a background refresh only when the previous one has
            // finished; results are applied each tick via poll_session_status_bg.
            if app.session_status_bg.is_none() {
                let session_status_started_at = Instant::now();
                app.sync_session_status_background();
                app.perf.record_duration(
                    "sync.session_status_bg_start",
                    session_status_started_at.elapsed(),
                );
            }
            app.refresh_fs_watch_paths();
            let usage_refresh_started_at = Instant::now();
            let usage_stats_hint = app.usage_stats_hint();
            app.usage.refresh(usage_stats_hint);
            app.perf
                .record_duration("usage.refresh", usage_refresh_started_at.elapsed());
            let usage = app.usage.get_data();
            let key = (
                usage.codex.five_hour_usage_pct.map(|v| v.round() as i64),
                usage.codex.weekly_usage_pct.map(|v| v.round() as i64),
                usage.codex.today_tokens,
                usage.codex.today_calls,
            );
            if last_usage_debug != Some(key) {
                app.log_debug(
                    "usage",
                    format!(
                        "codex 5h_pct={:?} 7d_pct={:?} today_tokens={} calls={} 5h_tokens={}",
                        usage.codex.five_hour_usage_pct,
                        usage.codex.weekly_usage_pct,
                        usage.codex.today_tokens,
                        usage.codex.today_calls,
                        usage.codex.five_hour_tokens
                    ),
                );
                last_usage_debug = Some(key);
            }
            let claude_summary = format!(
                "claude 5h_pct={:?} 7d_pct={:?} 5h_reset={:?} 7d_reset={:?} sub={:?} err={:?} today_msgs={} today_tokens={}",
                usage.claude.five_hour_pct,
                usage.claude.seven_day_pct,
                usage.claude.five_hour_resets,
                usage.claude.seven_day_resets,
                usage.claude.subscription_type,
                usage.claude.last_error,
                usage.claude.today_messages,
                usage.claude.today_tokens
            );
            if last_claude_usage_debug.as_ref() != Some(&claude_summary) {
                app.log_debug("usage", claude_summary.clone());
                if let Some(err) = &usage.claude.last_error {
                    app.log_warn("usage", format!("claude usage error: {err}"));
                }
                last_claude_usage_debug = Some(claude_summary);
            }
            last_sync = std::time::Instant::now();
            force_redraw = true;
        }

        let mut ipc_activity = false;
        if app.ipc.is_some() {
            // Drain all buffered socket messages each iteration.
            ipc_activity = app.drain_ipc_messages();
        }

        force_redraw |= app.handle_prompt_file_events();

        // Event-driven scan of fallback notification files: only runs
        // when the watcher saw a change inside a notifications dir
        // (e.g. a diff-review hook fell back to file delivery because
        // IPC was unavailable). No polling happens otherwise.
        if app.take_notifications_dirty() {
            let started_at = Instant::now();
            let notifications_changed = app.scan_notifications_forced();
            app.perf
                .record_duration("scan.notifications", started_at.elapsed());
            force_redraw |= notifications_changed;
        }

        // With the filesystem watcher running, thinking sync is driven
        // by marker-file events and IPC activity; the interval is only
        // a fallback. Without a watcher, keep the legacy 500ms cadence.
        let thinking_fallback = if app.fs_watcher.is_some() {
            THINKING_SYNC_FALLBACK_INTERVAL
        } else {
            Duration::from_millis(500)
        };
        if !handled_user_events
            && (app.take_thinking_dirty()
                || ipc_activity
                || last_thinking_sync.elapsed() >= thinking_fallback)
        {
            let started_at = Instant::now();
            let thinking_changed = app.sync_thinking_status();
            app.perf
                .record_duration("sync.thinking_status", started_at.elapsed());
            last_thinking_sync = std::time::Instant::now();
            force_redraw |= thinking_changed;
        }

        if app.summary_rx.is_some() {
            let summary_poll_started_at = Instant::now();
            if let Err(e) = app.poll_summary_result() {
                app.show_error(e);
            }
            app.perf
                .record_duration("summary.poll_result", summary_poll_started_at.elapsed());
        }

        if app.has_active_sidebar() {
            force_redraw |= app.poll_sidebar_load_results();
        }

        if app.should_flush_pending_debug_log_entries() {
            app.flush_pending_debug_log_entries();
        }

        if last_file_log_flush.elapsed() >= Duration::from_secs(1) {
            debug::flush_log_writer();
            last_file_log_flush = Instant::now();
            if last_log_rotation_check.elapsed() >= Duration::from_secs(3600) {
                debug::rotate_log_if_needed();
                last_log_rotation_check = Instant::now();
            }
            if last_notification_prune.elapsed() >= Duration::from_secs(3600) {
                app.prune_stale_notifications();
                last_notification_prune = Instant::now();
            }
        }
        let startup_grace_active = is_viewing && Instant::now() < startup_grace_until;
        let can_run_startup_task = startup_tasks_pending
            && !handled_user_events
            && !defer_background_sync
            && !startup_grace_active
            && Instant::now() >= startup_task_spacing_until;

        if can_run_startup_task {
            if startup_sync_statuses_pending {
                let started_at = Instant::now();
                app.sync_statuses();
                app.perf
                    .record_duration("startup.sync_statuses", started_at.elapsed());
                startup_sync_statuses_pending = false;
                force_redraw = true;
            } else if startup_session_status_pending {
                let started_at = Instant::now();
                app.sync_session_status_background();
                app.perf
                    .record_duration("startup.sync_session_status_bg_start", started_at.elapsed());
                startup_session_status_pending = false;
                // Results applied asynchronously via poll_session_status_bg().
            } else if startup_claude_hooks_pending {
                let started_at = Instant::now();
                let refreshed = app::setup::refresh_claude_hooks_for_store(&app.store, &app.config);
                app.perf
                    .record_duration("startup.refresh_claude_hooks", started_at.elapsed());
                app.log_info(
                    "setup",
                    format!("Refreshed Claude hooks for {refreshed} feature(s)"),
                );
                startup_claude_hooks_pending = false;
            } else if startup_opencode_plugins_pending {
                let started_at = Instant::now();
                let refreshed = app::setup::refresh_opencode_plugins_for_store(&app.store);
                app.perf
                    .record_duration("startup.refresh_opencode_plugins", started_at.elapsed());
                app.log_info(
                    "setup",
                    format!("Refreshed opencode plugins for {refreshed} feature(s)"),
                );
                startup_opencode_plugins_pending = false;
            } else if startup_usage_pending && app.session_status_bg.is_none() {
                let started_at = Instant::now();
                app.usage.refresh(None);
                app.perf
                    .record_duration("startup.usage_refresh", started_at.elapsed());
                startup_usage_pending = false;
                force_redraw = true;
            } else if startup_sidebar_warm_pending
                && startup_sidebar_can_warm(
                    app.session_status_bg.is_some(),
                    app.usage.stats_refresh_inflight(),
                )
            {
                let started_at = Instant::now();
                app.schedule_sidebar_loads_for_all_features();
                app.perf
                    .record_duration("startup.schedule_sidebar_warm", started_at.elapsed());
                startup_sidebar_warm_pending = false;
            }

            startup_task_spacing_until = if startup_loading {
                Instant::now()
            } else {
                Instant::now() + Duration::from_millis(250)
            };
        }

        if let app::AppMode::Viewing(ref view) = app.mode {
            // The view content area is the full terminal minus the
            // 1-row header (see ui/pane.rs); the pane, the vt100
            // parser, and the rendered area must all share these
            // dimensions or captured output reflows incorrectly.
            let content_rows = size.height.saturating_sub(1);
            let content_cols = ui::viewing_main_width(view, size.width);
            let current_resize = (
                content_cols,
                content_rows,
                view.session.clone(),
                view.window.clone(),
            );

            let mut dims_applied = last_resize.as_ref() == Some(&current_resize);
            if !dims_applied {
                // Resize immediately when switching to a different
                // session/window; debounce when only the dimensions
                // changed (mid-drag) so the agent pane gets a single
                // SIGWINCH once the viewport settles.
                let same_target = last_resize.as_ref().is_some_and(|(_, _, session, window)| {
                    session == &view.session && window == &view.window
                });
                if !same_target || viewport_stable_since.elapsed() >= VIEWPORT_RESIZE_DEBOUNCE {
                    let _ = TmuxManager::resize_pane(
                        &view.session,
                        &view.window,
                        content_cols,
                        content_rows,
                    );
                    last_resize = Some(current_resize);
                    app.request_view_snapshot_refresh();
                    force_redraw = true;
                    dims_applied = true;
                }
            }
            // Store the rendering dimensions (content area in pane.rs)
            // for mouse selection alignment — but only once the pane has
            // actually been resized to them, so the snapshot worker is
            // not restarted for every intermediate drag size.
            if dims_applied {
                app.pane_content_cols = content_cols;
                app.pane_content_rows = content_rows;
            }
        }

        // Periodic re-anchor bounce for live Claude panes. Claude Code's
        // incremental renderer drifts its input-box anchor over time and
        // leaves stale cells in the real tmux grid (input text bleeds into
        // the divider above it); AMF can only display that grid
        // faithfully, so the fix is to force a full agent repaint. A
        // SIGWINCH pair (shrink one row, then restore) does it. Two-phase
        // so the main thread never sleeps: shrink now, restore a later
        // iteration once the dwell has elapsed and tmux has delivered the
        // first SIGWINCH. See VIEW_REANCHOR_BOUNCE_INTERVAL.
        if let Some((session, window, cols, full_rows, shrunk_at)) =
            pending_reanchor_restore.clone()
        {
            if shrunk_at.elapsed() >= app::VIEW_REANCHOR_BOUNCE_DWELL {
                let _ = TmuxManager::resize_pane(&session, &window, cols, full_rows);
                app.request_view_snapshot_refresh();
                // Reveal the repainted pane only after Claude Code has had
                // time to redraw at full height; until then keep showing
                // the last good frame so the bounce stays invisible.
                app.freeze_view_display(app::VIEW_REANCHOR_REVEAL_DELAY);
                pending_reanchor_restore = None;
                last_reanchor_bounce = Instant::now();
            }
        } else if pane_live
            && !app.has_pending_view_input()
            && viewport_stable_since.elapsed() >= VIEWPORT_RESIZE_DEBOUNCE
            && last_reanchor_bounce.elapsed() >= app::VIEW_REANCHOR_BOUNCE_INTERVAL
            && let Some((session, window, cols, full_rows)) = app.reanchor_bounce_target()
        {
            let _ = TmuxManager::resize_pane(
                &session,
                &window,
                cols,
                full_rows.saturating_sub(1).max(1),
            );
            // Freeze the display through the shrink and until the restore
            // phase re-freezes for the precise reveal. Generous bound so a
            // delayed restore still can't strand the freeze.
            app.freeze_view_display(
                app::VIEW_REANCHOR_BOUNCE_DWELL
                    + app::VIEW_REANCHOR_REVEAL_DELAY
                    + Duration::from_millis(250),
            );
            pending_reanchor_restore = Some((session, window, cols, full_rows, Instant::now()));
        }

        app.ensure_view_snapshot_worker();
        let (pane_refreshed, cursor_refreshed) = app.drain_view_snapshots();

        let current_redraw_signature = app.redraw_signature();
        let state_changed = current_redraw_signature != loop_state_signature;
        last_redraw_signature = current_redraw_signature;
        let needs_redraw = force_redraw
            || handled_user_events && !is_viewing
            || pane_refreshed
            || cursor_refreshed
            || state_changed
            || animating && !handled_user_events;

        if needs_redraw {
            let draw_started_at = Instant::now();
            if startup_loading {
                let message = startup_loading_message(
                    startup_sync_statuses_pending,
                    startup_session_status_pending,
                    startup_usage_pending,
                    startup_claude_hooks_pending,
                    startup_opencode_plugins_pending,
                    startup_sidebar_warm_pending,
                    app.session_status_bg.is_some(),
                );
                let hint = startup_loading_hint(startup_loading_started_at.elapsed());
                terminal.draw(|frame| draw_loading(frame, message, hint))?;
            } else {
                terminal.draw(|frame| ui::draw(frame, app))?;
            }
            app.perf
                .record_duration("ui.draw", draw_started_at.elapsed());
            app.perf.note_draw_completed();
            force_redraw = false;

            if !startup_loading && app.diff_viewer_loading() {
                let started_at = Instant::now();
                app.complete_diff_viewer_loading();
                app.perf
                    .record_duration("diff_viewer.load_snapshot", started_at.elapsed());
                force_redraw = true;
            }

            if !startup_loading && app.markdown_loading() {
                let started_at = Instant::now();
                app.complete_markdown_loading();
                app.perf
                    .record_duration("markdown.load", started_at.elapsed());
                force_redraw = true;
            }
        }

        // Always drain (keeps interval sample buffers bounded), but only
        // persist the summaries when profiling is explicitly enabled.
        let summary_lines = app.perf.take_due_summary_lines();
        if perf::perf_logging_enabled() {
            for line in summary_lines {
                app.log_debug("perf", line);
            }
        }

        if app.should_quit {
            app.flush_pending_debug_log_entries();
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{cleanup_hooks_at, startup_loading_pending, startup_sidebar_can_warm};
    use std::fs;
    use tempfile::TempDir;

    fn write_settings(dir: &TempDir, json: &str) -> std::path::PathBuf {
        let path = dir.path().join("settings.json");
        fs::write(&path, json).unwrap();
        path
    }

    fn read_settings(path: &std::path::Path) -> serde_json::Value {
        let s = fs::read_to_string(path).unwrap();
        serde_json::from_str(&s).unwrap()
    }

    #[test]
    fn background_startup_work_does_not_hold_loading_screen() {
        assert!(!startup_loading_pending(false, false, false, false));
        assert!(startup_loading_pending(true, false, false, false));
        assert!(startup_loading_pending(false, true, false, false));
        assert!(startup_loading_pending(false, false, true, false));
        assert!(startup_loading_pending(false, false, false, true));
    }

    #[test]
    fn sidebar_warm_waits_for_transcript_jobs() {
        assert!(!startup_sidebar_can_warm(true, false));
        assert!(!startup_sidebar_can_warm(false, true));
        assert!(!startup_sidebar_can_warm(true, true));
        assert!(startup_sidebar_can_warm(false, false));
    }

    #[test]
    fn removes_static_amf_thinking_commands() {
        let dir = TempDir::new().unwrap();
        let path = write_settings(
            &dir,
            r#"{
            "hooks": {
                "PreToolUse": [{"matcher":"","hooks":[
                    {"type":"command","command":"[ -n \"$AMF_SESSION\" ] && mkdir -p /tmp/amf-thinking && touch \"/tmp/amf-thinking/$AMF_SESSION\" || true"}
                ]}],
                "Stop": [{"matcher":"","hooks":[
                    {"type":"command","command":"[ -n \"$AMF_SESSION\" ] && rm -f \"/tmp/amf-thinking/$AMF_SESSION\" || true"}
                ]}]
            }
        }"#,
        );

        cleanup_hooks_at(&path, &[]);

        let s = read_settings(&path);
        assert!(
            s["hooks"].get("PreToolUse").is_none(),
            "PreToolUse should be gone"
        );
        assert!(s["hooks"].get("Stop").is_none(), "Stop should be gone");
    }

    #[test]
    fn removes_extra_cmd_path() {
        let dir = TempDir::new().unwrap();
        let path = write_settings(
            &dir,
            r#"{
            "hooks": {
                "Stop": [{"matcher":"","hooks":[
                    {"type":"command","command":"/home/user/.config/amf/notify.sh"}
                ]}]
            }
        }"#,
        );

        cleanup_hooks_at(&path, &["/home/user/.config/amf/notify.sh"]);

        let s = read_settings(&path);
        assert!(
            s["hooks"].get("Stop").is_none(),
            "Stop entry for notify.sh should be removed"
        );
    }

    #[test]
    fn preserves_non_amf_hooks() {
        let dir = TempDir::new().unwrap();
        let path = write_settings(
            &dir,
            r#"{
            "hooks": {
                "Stop": [
                    {"matcher":"","hooks":[{"type":"command","command":"/my/custom/hook.sh"}]},
                    {"matcher":"","hooks":[{"type":"command","command":"[ -n \"$AMF_SESSION\" ] && rm -f \"/tmp/amf-thinking/$AMF_SESSION\" || true"}]}
                ]
            }
        }"#,
        );

        cleanup_hooks_at(&path, &[]);

        let s = read_settings(&path);
        let stop = s["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "only the AMF entry should be removed");
        assert_eq!(
            stop[0]["hooks"][0]["command"].as_str().unwrap(),
            "/my/custom/hook.sh"
        );
    }

    #[test]
    fn idempotent_when_nothing_to_remove() {
        let dir = TempDir::new().unwrap();
        let json = r#"{"hooks":{"Stop":[{"matcher":"","hooks":[{"type":"command","command":"/my/hook.sh"}]}]}}"#;
        let path = write_settings(&dir, json);

        cleanup_hooks_at(&path, &[]);
        let after = fs::read_to_string(&path).unwrap();

        // File should be unchanged (function returns early without writing).
        let v1: serde_json::Value = serde_json::from_str(json).unwrap();
        let v2: serde_json::Value = serde_json::from_str(&after).unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn no_op_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        // Should not panic.
        cleanup_hooks_at(&path, &[]);
    }

    #[test]
    fn removes_debug_hook() {
        let dir = TempDir::new().unwrap();
        let path = write_settings(
            &dir,
            r#"{
            "hooks": {
                "Stop": [{"matcher":"","hooks":[
                    {"type":"command","command":"/tmp/debug-hook.sh"}
                ]}]
            }
        }"#,
        );

        cleanup_hooks_at(&path, &[]);

        let s = read_settings(&path);
        assert!(
            s["hooks"].get("Stop").is_none(),
            "debug-hook.sh entry should be removed"
        );
    }
}
