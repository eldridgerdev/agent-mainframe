mod app;
mod claude;
mod project;
mod tmux;
mod ui;
mod worktree;

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
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
    loop {
        terminal.draw(|frame| ui::draw(frame, app))?;

        if app.should_quit || app.should_switch.is_some() {
            return Ok(());
        }

        if event::poll(Duration::from_millis(250))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                handle_key(app, key.code)?;
            }
        }
    }
}

fn handle_key(app: &mut App, key: KeyCode) -> Result<()> {
    match &app.mode {
        AppMode::Normal => handle_normal_key(app, key),
        AppMode::Creating(_) => handle_create_key(app, key),
        AppMode::Deleting(_) => handle_delete_key(app, key),
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
        KeyCode::Enter | KeyCode::Char('s') => {
            app.switch_to_selected()?;
        }
        KeyCode::Char('t') => {
            app.open_terminal()?;
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
