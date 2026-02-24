#![allow(dead_code)]

mod app;
mod claude;
mod extension;
mod handlers;
mod project;
mod tmux;
mod ui;
mod usage;
mod worktree;

use anyhow::Result;
use crossterm::{
    event::{
        self, DisableBracketedPaste, EnableBracketedPaste,
        Event,
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

    setup_thinking_hooks();

    let store_path = project::store_path();
    let mut app = App::new(store_path)?;
    app.sync_statuses();
    app.scan_notifications();
    app.usage.refresh();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
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
fn setup_thinking_hooks() {
    use serde_json::{json, Value};
    use std::fs;
    use std::path::PathBuf;

    let pre_tool_cmd =
        "[ -n \"$AMF_SESSION\" ] \
         && mkdir -p /tmp/amf-thinking \
         && touch \"/tmp/amf-thinking/$AMF_SESSION\" \
         || true";
    let stop_cmd =
        "[ -n \"$AMF_SESSION\" ] \
         && rm -f \"/tmp/amf-thinking/$AMF_SESSION\" \
         || true";

    let settings_path: PathBuf = match std::env::var("HOME") {
        Ok(h) => {
            PathBuf::from(h).join(".claude").join("settings.json")
        }
        Err(_) => return,
    };

    let mut root: Value = if settings_path.exists() {
        match fs::read_to_string(&settings_path) {
            Ok(s) => {
                serde_json::from_str(&s).unwrap_or(json!({}))
            }
            Err(_) => return,
        }
    } else {
        json!({})
    };

    let Some(root_obj) = root.as_object_mut() else {
        return;
    };
    let hooks = root_obj
        .entry("hooks")
        .or_insert(json!({}));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return;
    };

    for (event, cmd) in
        [("PreToolUse", pre_tool_cmd), ("Stop", stop_cmd)]
    {
        let event_arr =
            hooks_obj.entry(event).or_insert(json!([]));
        let already_present = event_arr
            .as_array()
            .map(|arr| {
                arr.iter().any(|entry| {
                    entry["hooks"]
                        .as_array()
                        .map(|hs| {
                            hs.iter()
                                .any(|h| h["command"] == cmd)
                        })
                        .unwrap_or(false)
                })
            })
            .unwrap_or(false);
        if !already_present
            && let Some(arr) = event_arr.as_array_mut()
        {
            arr.push(json!({
                "matcher": "",
                "hooks": [
                    {"type": "command", "command": cmd}
                ]
            }));
        }
    }

    if let Ok(serialized) = serde_json::to_string_pretty(&root)
    {
        let _ = fs::write(
            &settings_path,
            serialized + "\n",
        );
    }
}

fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    let mut last_sync = std::time::Instant::now();
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
                app.tmux_cursor =
                    TmuxManager::cursor_position(&session, &window)
                        .ok();
            }
        }

        app.throbber_state.calc_next();

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
                app.sync_thinking_status();
            }
            app.scan_notifications();
            app.usage.refresh();
            last_sync = std::time::Instant::now();
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
