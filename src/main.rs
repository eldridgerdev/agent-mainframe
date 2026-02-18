#![allow(dead_code)]

mod app;
mod claude;
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
