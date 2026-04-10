#![allow(dead_code)]

mod app;
mod automation;
mod claude;
mod codex;
mod debug;
mod diff;
mod editor;
mod extension;
mod handlers;
mod highlight;
mod http_client;
mod ipc;
mod markdown;
mod perf;
mod pi;
mod project;
mod summary;
mod theme;
mod tmux;
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
        Event,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::prelude::*;
use std::io;
use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::time::Duration;
use std::time::Instant;

use app::App;
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

    if let Err(e) = TmuxManager::check_available() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }

    debug::install_panic_hook();
    cleanup_global_hooks();
    app::App::cleanup_stale_thinking_files();

    let store_path = project::store_path();
    let mut app = App::new(store_path)?;
    app.log_startup();

    if !app.store.has_any_harnesses() {
        app.open_harness_setup(true);
    }

    // Start IPC socket server for push-based hook notifications.
    let socket = ipc::socket_path();
    match ipc::start(&socket) {
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

    let result = panic::catch_unwind(AssertUnwindSafe(|| run_loop(&mut terminal, &mut app)));

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        SetCursorStyle::DefaultUserShape,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    if let Some(session) = &app.should_switch {
        TmuxManager::attach_session(session)?;
    }

    match result {
        Ok(result) => result,
        Err(_) => Err(anyhow::anyhow!(
            "AMF panicked; see {}",
            debug::global_log_path().display()
        )),
    }
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

fn run_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let mut last_sync = std::time::Instant::now();
    let mut last_thinking_sync = std::time::Instant::now();
    let mut last_usage_debug: Option<(Option<i64>, Option<i64>, u64, u64)> = None;
    let mut last_claude_usage_debug: Option<String> = None;
    // Only used when no IPC socket is available (fallback).
    let mut last_notif_scan = std::time::Instant::now();
    let mut last_resize: Option<(u16, u16, String, String)> = None;
    let mut force_redraw = true;
    let startup_grace_until = Instant::now() + Duration::from_secs(3);
    let mut startup_task_spacing_until = Instant::now();
    let mut startup_sync_statuses_pending = true;
    let mut startup_session_status_pending = true;
    let mut startup_notifications_pending = true;
    let mut startup_usage_pending = true;
    let mut startup_claude_hooks_pending = true;
    let mut startup_opencode_plugins_pending = true;
    let mut startup_sidebar_warm_pending = true;
    const ANIMATED_REDRAW_INTERVAL: Duration = Duration::from_millis(125);

    loop {
        let loop_state_signature = app.redraw_signature();
        let is_viewing = matches!(app.mode, app::AppMode::Viewing(_));
        let animating = app.has_visible_animation();

        let size = terminal.size()?;
        let visible_rows = size.height.saturating_sub(3);
        app.viewport_cols = size.width;
        app.viewport_rows = visible_rows;

        app.throbber_state.calc_next();

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
            app.poll_codex_sidebar_metadata();
        }

        if let Some(alert) = debug::take_user_alert() {
            app.message = Some(alert);
            force_redraw = true;
        }

        if app.leader_active && app.leader_timed_out() {
            app.deactivate_leader();
            force_redraw = true;
        }

        let poll_duration = if is_viewing {
            Duration::from_millis(5)
        } else if animating {
            ANIMATED_REDRAW_INTERVAL
        } else {
            Duration::from_millis(250)
        };
        let mut handled_user_events = false;

        if event::poll(poll_duration)? {
            handled_user_events = true;
            let mut events = vec![event::read()?];

            if is_viewing {
                while event::poll(Duration::ZERO)? {
                    events.push(event::read()?);
                }
            }

            for ev in events {
                match ev {
                    Event::Key(key) => {
                        if is_viewing {
                            app.note_view_activity();
                        }
                        app.perf.increment_counter("event.key");
                        app.perf.note_input_for_next_draw();
                        let started_at = Instant::now();
                        if let Err(e) = handlers::handle_key(app, key, visible_rows) {
                            app.show_error(e);
                        }
                        app.perf.record_duration("main.handle_key", started_at.elapsed());
                    }
                    Event::Mouse(mouse) => {
                        if is_viewing {
                            app.note_view_activity();
                        }
                        app.perf.increment_counter("event.mouse");
                        let started_at = Instant::now();
                        if let Err(e) = handlers::handle_mouse(app, mouse, visible_rows) {
                            app.show_error(e);
                        }
                        app.perf.record_duration("main.handle_mouse", started_at.elapsed());
                    }
                    Event::Paste(text) => {
                        if is_viewing {
                            app.note_view_activity();
                        }
                        app.perf.increment_counter("event.paste");
                        app.perf.note_input_for_next_draw();
                        let started_at = Instant::now();
                        if let Err(e) = handlers::handle_paste(app, &text) {
                            app.show_error(e);
                        }
                        app.perf.record_duration("main.handle_paste", started_at.elapsed());
                    }
                    Event::Resize(_, _) => {
                        last_resize = None;
                        force_redraw = true;
                    }
                    _ => {}
                }
            }
        }

        if app.should_quit || app.should_switch.is_some() {
            return Ok(());
        }

        if app.has_pending_view_input() {
            if let Err(e) = app.flush_view_input_batch() {
                app.show_error(e);
            }
        }

        let startup_tasks_pending = startup_sync_statuses_pending
            || startup_session_status_pending
            || startup_notifications_pending
            || startup_usage_pending
            || startup_claude_hooks_pending
            || startup_opencode_plugins_pending
            || startup_sidebar_warm_pending;
        let defer_background_sync = app.should_defer_view_background_sync();

        if !handled_user_events
            && last_sync.elapsed() >= Duration::from_secs(5)
            && !defer_background_sync
            && !startup_tasks_pending
        {
            if !is_viewing {
                let started_at = Instant::now();
                app.sync_statuses();
                app.perf.record_duration("sync.statuses", started_at.elapsed());
            }
            let session_status_started_at = Instant::now();
            app.sync_session_status();
            app.perf.record_duration("sync.session_status", session_status_started_at.elapsed());
            let usage_refresh_started_at = Instant::now();
            app.usage.refresh();
            app.perf.record_duration("usage.refresh", usage_refresh_started_at.elapsed());
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

        if app.ipc.is_some() {
            // Drain all buffered socket messages each iteration.
            app.drain_ipc_messages();
        }

        if !handled_user_events && last_notif_scan.elapsed() >= Duration::from_millis(500) {
            if app.ipc.is_none() && !app.ipc_fallback_logged {
                app.log_warn(
                    "ipc",
                    "IPC unavailable; using file-based notification polling".to_string(),
                );
                app.ipc_fallback_logged = true;
            }
            // Always scan file notifications as compatibility fallback.
            // Some producers (for example plugin runtimes) may not be
            // able to call `amf notify` even while IPC is available.
            let started_at = Instant::now();
            let notifications_changed = app.scan_notifications();
            app.perf.record_duration("scan.notifications", started_at.elapsed());
            last_notif_scan = std::time::Instant::now();
            force_redraw |= notifications_changed;
        }

        if !handled_user_events && last_thinking_sync.elapsed() >= Duration::from_millis(500) {
            let started_at = Instant::now();
            let thinking_changed = app.sync_thinking_status();
            app.perf.record_duration("sync.thinking_status", started_at.elapsed());
            last_thinking_sync = std::time::Instant::now();
            force_redraw |= thinking_changed;
        }

        let summary_poll_started_at = Instant::now();
        if let Err(e) = app.poll_summary_result() {
            app.show_error(e);
        }
        app.perf
            .record_duration("summary.poll_result", summary_poll_started_at.elapsed());

        if app.has_active_sidebar() {
            app.poll_sidebar_load_results();
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
                app.perf.record_duration("startup.sync_statuses", started_at.elapsed());
                startup_sync_statuses_pending = false;
                force_redraw = true;
            } else if startup_session_status_pending {
                let started_at = Instant::now();
                app.sync_session_status();
                app.perf
                    .record_duration("startup.sync_session_status", started_at.elapsed());
                startup_session_status_pending = false;
                force_redraw = true;
            } else if startup_notifications_pending {
                let started_at = Instant::now();
                let notifications_changed = app.scan_notifications();
                app.perf
                    .record_duration("startup.scan_notifications", started_at.elapsed());
                startup_notifications_pending = false;
                force_redraw |= notifications_changed;
            } else if startup_usage_pending {
                let started_at = Instant::now();
                app.usage.refresh();
                app.perf.record_duration("startup.usage_refresh", started_at.elapsed());
                startup_usage_pending = false;
                force_redraw = true;
            } else if startup_claude_hooks_pending {
                let started_at = Instant::now();
                let refreshed = app::setup::refresh_claude_hooks_for_store(&app.store, &app.config);
                app.perf
                    .record_duration("startup.refresh_claude_hooks", started_at.elapsed());
                app.log_info("setup", format!("Refreshed Claude hooks for {refreshed} feature(s)"));
                startup_claude_hooks_pending = false;
            } else if startup_opencode_plugins_pending {
                let started_at = Instant::now();
                let refreshed = app::setup::refresh_opencode_plugins_for_store(&app.store);
                app.perf.record_duration(
                    "startup.refresh_opencode_plugins",
                    started_at.elapsed(),
                );
                app.log_info(
                    "setup",
                    format!("Refreshed opencode plugins for {refreshed} feature(s)"),
                );
                startup_opencode_plugins_pending = false;
            } else if startup_sidebar_warm_pending {
                let started_at = Instant::now();
                app.schedule_sidebar_loads_for_all_features();
                app.perf
                    .record_duration("startup.schedule_sidebar_warm", started_at.elapsed());
                startup_sidebar_warm_pending = false;
            }

            startup_task_spacing_until = Instant::now() + Duration::from_millis(250);
        }

        if let app::AppMode::Viewing(ref view) = app.mode {
            let content_rows = visible_rows;
            let content_cols = ui::viewing_main_width(view, size.width);
            let current_resize = (
                content_cols,
                content_rows,
                view.session.clone(),
                view.window.clone(),
            );

            if last_resize.as_ref() != Some(&current_resize) {
                let _ = TmuxManager::resize_pane(
                    &view.session,
                    &view.window,
                    content_cols,
                    content_rows,
                );
                last_resize = Some(current_resize);
                app.request_view_snapshot_refresh();
                force_redraw = true;
            }
            // Store the rendering dimensions (content area in pane.rs),
            // not the tmux capture dimensions, so mouse selection
            // coordinates align correctly.
            app.pane_content_cols = content_cols;
            app.pane_content_rows = size.height.saturating_sub(1);
        }
        app.ensure_view_snapshot_worker();
        let (pane_refreshed, cursor_refreshed) = app.drain_view_snapshots();

        let state_changed = app.redraw_signature() != loop_state_signature;
        let needs_redraw =
            force_redraw
                || handled_user_events && !is_viewing
                || pane_refreshed
                || cursor_refreshed
                || state_changed
                || animating && !handled_user_events;

        if needs_redraw {
            let draw_started_at = Instant::now();
            terminal.draw(|frame| ui::draw(frame, app))?;
            app.perf.record_duration("ui.draw", draw_started_at.elapsed());
            app.perf.note_draw_completed();
            force_redraw = false;
        }

        for line in app.perf.take_due_summary_lines() {
            app.log_debug("perf", line);
        }

        if app.should_quit || app.should_switch.is_some() {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::cleanup_hooks_at;
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
