#![allow(dead_code)]

mod app;
mod claude;
mod project;
mod tmux;
mod ui;
mod worktree;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::prelude::*;
use std::io;
use std::time::Duration;

use app::{App, AppMode, CreateStep};
use tmux::TmuxManager;

fn main() -> Result<()> {
    // Preflight checks
    TmuxManager::check_available()?;

    let store_path = project::store_path();
    let mut app = App::new(store_path)?;
    app.sync_statuses();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // Handle post-exit switch
    if let Some(session) = &app.should_switch {
        TmuxManager::attach_session(session)?;
    }

    result
}

fn run_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut App) -> Result<()> {
    let mut last_sync = std::time::Instant::now();
    let mut last_size: Option<(u16, u16)> = None;

    loop {
        let is_viewing = matches!(app.mode, AppMode::Viewing(_));

        if is_viewing {
            // Resize tmux pane BEFORE capture so content matches display width
            let size = terminal.size()?;
            let content_rows = size.height.saturating_sub(3);
            let content_cols = size.width;
            let current_size = (content_cols, content_rows);

            if last_size != Some(current_size) {
                if let AppMode::Viewing(ref view) = app.mode {
                    let _ = TmuxManager::resize_pane(
                        &view.session,
                        &view.window,
                        content_cols,
                        content_rows,
                    );
                }
                last_size = Some(current_size);
            }

            // Capture pane content after resize
            if let AppMode::Viewing(ref view) = app.mode {
                let session = view.session.clone();
                let window = view.window.clone();
                app.pane_content = TmuxManager::capture_pane_ansi(&session, &window)
                    .unwrap_or_default();
            }
        }

        terminal.draw(|frame| ui::draw(frame, app))?;

        if app.should_quit || app.should_switch.is_some() {
            return Ok(());
        }

        // Periodic status refresh (every 5 seconds) so returning from a
        // project session shows up-to-date statuses.
        if !is_viewing && last_sync.elapsed() >= Duration::from_secs(5) {
            app.sync_statuses();
            last_sync = std::time::Instant::now();
        }

        let poll_duration = if is_viewing {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(250)
        };

        if event::poll(poll_duration)? {
            let ev = event::read()?;
            match ev {
                Event::Key(key) => {
                    if key.kind != KeyEventKind::Press {
                        continue;
                    }
                    handle_key(app, key)?;
                }
                Event::Resize(_, _) => {
                    // Reset last_size so pane gets resized on next draw
                    last_size = None;
                }
                _ => {}
            }
        }
    }
}

fn handle_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match &app.mode {
        AppMode::Normal => handle_normal_key(app, key.code),
        AppMode::Creating(_) => handle_create_key(app, key.code),
        AppMode::Deleting(_) => handle_delete_key(app, key.code),
        AppMode::Viewing(_) => handle_view_key(app, key),
    }
}

fn handle_normal_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
        }
        KeyCode::Char('n') => {
            app.start_create();
        }
        KeyCode::Char('d') => {
            if let Some(project) = app.selected_project() {
                let name = project.name.clone();
                app.mode = AppMode::Deleting(name);
            }
        }
        KeyCode::Enter => {
            app.enter_view()?;
        }
        KeyCode::Char('s') => {
            app.switch_to_selected()?;
        }
        KeyCode::Char('t') => {
            app.open_terminal()?;
        }
        KeyCode::Char('x') => {
            app.stop_selected()?;
        }
        KeyCode::Char('r') => {
            app.sync_statuses();
            app.message = Some("Refreshed project statuses".into());
        }
        KeyCode::Down | KeyCode::Char('j') => {
            app.select_next();
            app.message = None;
        }
        KeyCode::Up | KeyCode::Char('k') => {
            app.select_prev();
            app.message = None;
        }
        _ => {}
    }
    Ok(())
}

/// Map a crossterm KeyEvent to a tmux key representation
enum TmuxKey {
    Literal(String),
    Named(String),
}

fn crossterm_key_to_tmux(key: &KeyEvent) -> Option<TmuxKey> {
    // Handle Ctrl combinations
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            // Ctrl+letter: tmux accepts C-a through C-z
            return Some(TmuxKey::Named(format!("C-{}", c)));
        }
    }

    // Handle Alt combinations
    if key.modifiers.contains(KeyModifiers::ALT) {
        if let KeyCode::Char(c) = key.code {
            return Some(TmuxKey::Named(format!("M-{}", c)));
        }
    }

    match key.code {
        KeyCode::Char(c) => Some(TmuxKey::Literal(c.to_string())),
        KeyCode::Enter => Some(TmuxKey::Named("Enter".into())),
        KeyCode::Backspace => Some(TmuxKey::Named("BSpace".into())),
        KeyCode::Tab => Some(TmuxKey::Named("Tab".into())),
        KeyCode::Esc => Some(TmuxKey::Named("Escape".into())),
        KeyCode::Up => Some(TmuxKey::Named("Up".into())),
        KeyCode::Down => Some(TmuxKey::Named("Down".into())),
        KeyCode::Left => Some(TmuxKey::Named("Left".into())),
        KeyCode::Right => Some(TmuxKey::Named("Right".into())),
        KeyCode::Home => Some(TmuxKey::Named("Home".into())),
        KeyCode::End => Some(TmuxKey::Named("End".into())),
        KeyCode::PageUp => Some(TmuxKey::Named("PPage".into())),
        KeyCode::PageDown => Some(TmuxKey::Named("NPage".into())),
        KeyCode::Delete => Some(TmuxKey::Named("DC".into())),
        KeyCode::Insert => Some(TmuxKey::Named("IC".into())),
        KeyCode::F(n) => Some(TmuxKey::Named(format!("F{}", n))),
        _ => None,
    }
}

fn handle_view_key(app: &mut App, key: KeyEvent) -> Result<()> {
    // Ctrl+Q exits the view
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        app.exit_view();
        return Ok(());
    }

    // Forward everything else to tmux
    let (session, window) = match &app.mode {
        AppMode::Viewing(view) => (view.session.clone(), view.window.clone()),
        _ => return Ok(()),
    };

    if let Some(tmux_key) = crossterm_key_to_tmux(&key) {
        match tmux_key {
            TmuxKey::Literal(text) => {
                let _ = TmuxManager::send_literal(&session, &window, &text);
            }
            TmuxKey::Named(name) => {
                let _ = TmuxManager::send_key_name(&session, &window, &name);
            }
        }
    }

    Ok(())
}

fn handle_create_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.cancel_create();
        }
        KeyCode::Enter => {
            let should_advance = match &app.mode {
                AppMode::Creating(state) => match state.step {
                    CreateStep::Name => true,
                    CreateStep::Path => true,
                    CreateStep::Branch => false,
                },
                _ => false,
            };

            if should_advance {
                if let AppMode::Creating(state) = &mut app.mode {
                    state.step = match state.step {
                        CreateStep::Name => CreateStep::Path,
                        CreateStep::Path => CreateStep::Branch,
                        CreateStep::Branch => CreateStep::Branch,
                    };
                }
            } else {
                app.create_project()?;
            }
        }
        KeyCode::Tab => {
            if let AppMode::Creating(state) = &mut app.mode {
                state.step = match state.step {
                    CreateStep::Name => CreateStep::Path,
                    CreateStep::Path => CreateStep::Branch,
                    CreateStep::Branch => CreateStep::Name,
                };
            }
        }
        KeyCode::Backspace => {
            if let AppMode::Creating(state) = &mut app.mode {
                match state.step {
                    CreateStep::Name => { state.name.pop(); }
                    CreateStep::Path => { state.path.pop(); }
                    CreateStep::Branch => { state.branch.pop(); }
                }
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::Creating(state) = &mut app.mode {
                match state.step {
                    CreateStep::Name => state.name.push(c),
                    CreateStep::Path => state.path.push(c),
                    CreateStep::Branch => state.branch.push(c),
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_delete_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('y') => {
            app.delete_selected()?;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
    Ok(())
}
