#![allow(dead_code)]

mod app;
mod claude;
mod debug;
mod extension;
mod handlers;
mod project;
mod summary;
mod theme;
mod tmux;
mod traits;
mod transcript;
mod ui;
mod usage;
mod worktree;

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste,
        DisableMouseCapture, EnableMouseCapture, Event,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode,
        EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use ratatui::prelude::*;
use std::io;
use std::time::Duration;

use app::App;
use tmux::TmuxManager;

fn main() -> Result<()> {
    if let Err(e) = TmuxManager::check_available() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }

    cleanup_global_hooks();

    let store_path = project::store_path();
    let mut app = App::new(store_path)?;
    app.log_startup();
    app.sync_statuses();
    app.sync_session_status();
    app.scan_notifications();
    app.usage.refresh();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;

    if let Some(session) = &app.should_switch {
        TmuxManager::attach_session(session)?;
    }

    result
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
    let extra_cmds =
        [format!("{home}/.config/amf/notify.sh")];
    let extra: Vec<&str> =
        extra_cmds.iter().map(|s| s.as_str()).collect();
    cleanup_hooks_at(&settings_path, &extra);
}

/// Inner logic for `cleanup_global_hooks`, factored out for
/// testability.  `extra_cmds` are host-specific command
/// strings (e.g. absolute paths) to remove in addition to
/// the static AMF commands.
pub fn cleanup_hooks_at(
    settings_path: &std::path::Path,
    extra_cmds: &[&str],
) {
    use serde_json::Value;

    let mut root: Value =
        match std::fs::read_to_string(settings_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
        {
            Some(v) => v,
            None => return,
        };

    let Some(hooks_obj) = root
        .get_mut("hooks")
        .and_then(|h| h.as_object_mut())
    else {
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
                    h["command"].as_str().is_some_and(|c| {
                        static_cmds.contains(&c)
                            || extra_cmds.contains(&c)
                    })
                })
            })
        });
        if arr.len() != before {
            changed = true;
        }
    }

    // Drop empty event arrays.
    hooks_obj.retain(|_, v| {
        v.as_array().is_none_or(|a| !a.is_empty())
    });

    if !changed {
        return;
    }

    if let Ok(serialized) =
        serde_json::to_string_pretty(&root)
    {
        let _ = std::fs::write(
            settings_path,
            serialized + "\n",
        );
    }
}

fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    let mut last_sync = std::time::Instant::now();
    let mut last_thinking_sync = std::time::Instant::now();
    let mut last_notif_scan = std::time::Instant::now();
    let mut last_resize: Option<(u16, u16, String, String)> =
        None;

    loop {
        let is_viewing =
            matches!(app.mode, app::AppMode::Viewing(_));

        let size = terminal.size()?;
        let visible_rows = size.height.saturating_sub(3);

        if is_viewing {
            let content_rows = visible_rows;
            let content_cols = size.width;
            if let app::AppMode::Viewing(ref view) = app.mode
            {
                let current_resize = (
                    content_cols,
                    content_rows,
                    view.session.clone(),
                    view.window.clone(),
                );

                if last_resize.as_ref()
                    != Some(&current_resize)
                {
                    let _ = TmuxManager::resize_pane(
                        &view.session,
                        &view.window,
                        content_cols,
                        content_rows,
                    );
                    last_resize = Some(current_resize);
                }
            }

            if let app::AppMode::Viewing(ref view) = app.mode {
                let session = view.session.clone();
                let window = view.window.clone();
                app.pane_content =
                    TmuxManager::capture_pane_ansi(
                        &session, &window,
                    )
                    .unwrap_or_default();
                // Store the rendering dimensions (content area in pane.rs),
                // not the tmux capture dimensions, so mouse selection
                // coordinates align correctly.
                app.pane_content_cols = size.width;
                app.pane_content_rows = size.height.saturating_sub(1);
                app.tmux_cursor =
                    TmuxManager::cursor_position(&session, &window)
                        .ok();
            }
        }

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

        terminal.draw(|frame| ui::draw(frame, app))?;

        if app.should_quit || app.should_switch.is_some() {
            return Ok(());
        }

        if app.leader_active && app.leader_timed_out() {
            app.deactivate_leader();
        }

        if last_sync.elapsed() >= Duration::from_secs(5) {
            if !is_viewing {
                app.sync_statuses();
            }
            app.sync_session_status();
            app.usage.refresh();
            last_sync = std::time::Instant::now();
        }

        if last_thinking_sync.elapsed() >= Duration::from_millis(500) {
            app.sync_thinking_status();
            last_thinking_sync = std::time::Instant::now();
        }

        if last_notif_scan.elapsed() >= Duration::from_millis(500) {
            app.scan_notifications();
            last_notif_scan = std::time::Instant::now();
        }

        if let Err(e) = app.poll_summary_result() {
            app.show_error(e);
        }

        let poll_duration = if is_viewing {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(250)
        };

        if event::poll(poll_duration)? {
            let mut events = vec![event::read()?];

            if is_viewing {
                while event::poll(Duration::ZERO)? {
                    events.push(event::read()?);
                }
            }

            for ev in events {
                match ev {
                    Event::Key(key) => {
                        if let Err(e) = handlers::handle_key(app, key, visible_rows) {
                            app.show_error(e);
                        }
                    }
                    Event::Mouse(mouse) => {
                        if let Err(e) = handlers::handle_mouse(app, mouse, visible_rows) {
                            app.show_error(e);
                        }
                    }
                    Event::Paste(text) => {
                        if let Err(e) =
                            handlers::handle_paste(app, &text)
                        {
                            app.show_error(e);
                        }
                    }
                    Event::Resize(_, _) => {
                        last_resize = None;
                    }
                    _ => {}
                }
            }
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
        let path = write_settings(&dir, r#"{
            "hooks": {
                "PreToolUse": [{"matcher":"","hooks":[
                    {"type":"command","command":"[ -n \"$AMF_SESSION\" ] && mkdir -p /tmp/amf-thinking && touch \"/tmp/amf-thinking/$AMF_SESSION\" || true"}
                ]}],
                "Stop": [{"matcher":"","hooks":[
                    {"type":"command","command":"[ -n \"$AMF_SESSION\" ] && rm -f \"/tmp/amf-thinking/$AMF_SESSION\" || true"}
                ]}]
            }
        }"#);

        cleanup_hooks_at(&path, &[]);

        let s = read_settings(&path);
        assert!(
            s["hooks"].get("PreToolUse").is_none(),
            "PreToolUse should be gone"
        );
        assert!(
            s["hooks"].get("Stop").is_none(),
            "Stop should be gone"
        );
    }

    #[test]
    fn removes_extra_cmd_path() {
        let dir = TempDir::new().unwrap();
        let path = write_settings(&dir, r#"{
            "hooks": {
                "Stop": [{"matcher":"","hooks":[
                    {"type":"command","command":"/home/user/.config/amf/notify.sh"}
                ]}]
            }
        }"#);

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
        let path = write_settings(&dir, r#"{
            "hooks": {
                "Stop": [
                    {"matcher":"","hooks":[{"type":"command","command":"/my/custom/hook.sh"}]},
                    {"matcher":"","hooks":[{"type":"command","command":"[ -n \"$AMF_SESSION\" ] && rm -f \"/tmp/amf-thinking/$AMF_SESSION\" || true"}]}
                ]
            }
        }"#);

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
        let path = write_settings(&dir, r#"{
            "hooks": {
                "Stop": [{"matcher":"","hooks":[
                    {"type":"command","command":"/tmp/debug-hook.sh"}
                ]}]
            }
        }"#);

        cleanup_hooks_at(&path, &[]);

        let s = read_settings(&path);
        assert!(
            s["hooks"].get("Stop").is_none(),
            "debug-hook.sh entry should be removed"
        );
    }
}
