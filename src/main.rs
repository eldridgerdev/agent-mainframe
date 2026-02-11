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
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::prelude::*;
use std::io;
use std::time::Duration;

use app::{App, AppMode, Selection};
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

fn run_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    let mut last_sync = std::time::Instant::now();
    let mut last_size: Option<(u16, u16)> = None;

    loop {
        let is_viewing = matches!(app.mode, AppMode::Viewing(_));

        if is_viewing {
            // Resize tmux pane BEFORE capture so content matches
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
                app.pane_content =
                    TmuxManager::capture_pane_ansi(&session, &window)
                        .unwrap_or_default();
            }
        }

        terminal.draw(|frame| ui::draw(frame, app))?;

        if app.should_quit || app.should_switch.is_some() {
            return Ok(());
        }

        // Periodic status refresh (every 5 seconds)
        if !is_viewing && last_sync.elapsed() >= Duration::from_secs(5)
        {
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
        AppMode::CreatingProject(_) => {
            handle_create_project_key(app, key.code)
        }
        AppMode::CreatingFeature(_) => {
            handle_create_feature_key(app, key.code)
        }
        AppMode::DeletingProject(_) => {
            handle_delete_project_key(app, key.code)
        }
        AppMode::DeletingFeature(_, _) => {
            handle_delete_feature_key(app, key.code)
        }
        AppMode::Viewing(_) => handle_view_key(app, key),
    }
}

fn handle_normal_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
        }
        KeyCode::Char('N') => {
            app.start_create_project();
        }
        KeyCode::Char('n') => {
            // Add feature to the current/parent project
            if app.selected_project().is_some() {
                app.start_create_feature();
            }
        }
        KeyCode::Enter => {
            match &app.selection {
                Selection::Project(_) => {
                    // Toggle collapse on project
                    app.toggle_collapse();
                }
                Selection::Feature(_, _) => {
                    // Enter view on feature
                    app.enter_view()?;
                }
            }
        }
        KeyCode::Char('c') => {
            // Start claude session (feature only)
            if matches!(app.selection, Selection::Feature(_, _)) {
                app.start_feature()?;
            }
        }
        KeyCode::Char('x') => {
            // Stop feature (feature only)
            if matches!(app.selection, Selection::Feature(_, _)) {
                app.stop_feature()?;
            }
        }
        KeyCode::Char('d') => {
            match &app.selection {
                Selection::Project(pi) => {
                    if let Some(project) = app.store.projects.get(*pi)
                    {
                        let name = project.name.clone();
                        app.mode = AppMode::DeletingProject(name);
                    }
                }
                Selection::Feature(pi, fi) => {
                    if let Some(project) = app.store.projects.get(*pi)
                    {
                        if let Some(feature) =
                            project.features.get(*fi)
                        {
                            let pn = project.name.clone();
                            let fn_ = feature.name.clone();
                            app.mode =
                                AppMode::DeletingFeature(pn, fn_);
                        }
                    }
                }
            }
        }
        KeyCode::Char('s') => {
            if matches!(app.selection, Selection::Feature(_, _)) {
                app.switch_to_selected()?;
            }
        }
        KeyCode::Char('t') => {
            if matches!(app.selection, Selection::Feature(_, _)) {
                app.open_terminal()?;
            }
        }
        KeyCode::Char('r') => {
            app.sync_statuses();
            app.message = Some("Refreshed statuses".into());
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
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        if let KeyCode::Char(c) = key.code {
            return Some(TmuxKey::Named(format!("C-{}", c)));
        }
    }

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
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('q')
    {
        app.exit_view();
        return Ok(());
    }

    // Forward everything else to tmux
    let (session, window) = match &app.mode {
        AppMode::Viewing(view) => {
            (view.session.clone(), view.window.clone())
        }
        _ => return Ok(()),
    };

    if let Some(tmux_key) = crossterm_key_to_tmux(&key) {
        match tmux_key {
            TmuxKey::Literal(text) => {
                let _ = TmuxManager::send_literal(
                    &session, &window, &text,
                );
            }
            TmuxKey::Named(name) => {
                let _ = TmuxManager::send_key_name(
                    &session, &window, &name,
                );
            }
        }
    }

    Ok(())
}

fn handle_create_project_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    use app::CreateProjectStep;

    match key {
        KeyCode::Esc => {
            app.cancel_create();
        }
        KeyCode::Enter => {
            let should_advance = match &app.mode {
                AppMode::CreatingProject(state) => {
                    matches!(state.step, CreateProjectStep::Name)
                }
                _ => false,
            };

            if should_advance {
                if let AppMode::CreatingProject(state) = &mut app.mode
                {
                    state.step = CreateProjectStep::Path;
                }
            } else {
                app.create_project()?;
            }
        }
        KeyCode::Tab => {
            if let AppMode::CreatingProject(state) = &mut app.mode {
                state.step = match state.step {
                    CreateProjectStep::Name => CreateProjectStep::Path,
                    CreateProjectStep::Path => CreateProjectStep::Name,
                };
            }
        }
        KeyCode::Backspace => {
            if let AppMode::CreatingProject(state) = &mut app.mode {
                match state.step {
                    CreateProjectStep::Name => {
                        state.name.pop();
                    }
                    CreateProjectStep::Path => {
                        state.path.pop();
                    }
                }
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::CreatingProject(state) = &mut app.mode {
                match state.step {
                    CreateProjectStep::Name => state.name.push(c),
                    CreateProjectStep::Path => state.path.push(c),
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_create_feature_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.cancel_create();
        }
        KeyCode::Enter => {
            app.create_feature()?;
        }
        KeyCode::Backspace => {
            if let AppMode::CreatingFeature(state) = &mut app.mode {
                state.branch.pop();
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::CreatingFeature(state) = &mut app.mode {
                state.branch.push(c);
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_delete_project_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Char('y') => {
            app.delete_project()?;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
    Ok(())
}

fn handle_delete_feature_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Char('y') => {
            app.delete_feature()?;
        }
        KeyCode::Char('n') | KeyCode::Esc => {
            app.mode = AppMode::Normal;
        }
        _ => {}
    }
    Ok(())
}
