#![allow(dead_code)]

mod app;
mod claude;
mod project;
mod tmux;
mod ui;
mod worktree;

use anyhow::Result;
use crossterm::{
    event::{
        self, Event, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers,
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

use app::{App, AppMode, Selection};
use tmux::TmuxManager;

fn main() -> Result<()> {
    // Preflight checks
    TmuxManager::check_available()?;

    let store_path = project::store_path();
    let mut app = App::new(store_path)?;
    app.sync_statuses();
    app.scan_notifications();

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
        let is_viewing =
            matches!(app.mode, AppMode::Viewing(_));

        if is_viewing {
            // Resize tmux pane BEFORE capture so content matches
            let size = terminal.size()?;
            let content_rows = size.height.saturating_sub(3);
            let content_cols = size.width;
            let current_size = (content_cols, content_rows);

            if last_size != Some(current_size) {
                if let AppMode::Viewing(ref view) = app.mode
                {
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
                    TmuxManager::capture_pane_ansi(
                        &session, &window,
                    )
                    .unwrap_or_default();
            }
        }

        terminal.draw(|frame| ui::draw(frame, app))?;

        if app.should_quit || app.should_switch.is_some() {
            return Ok(());
        }

        // Auto-cancel leader if timed out
        if app.leader_active && app.leader_timed_out() {
            app.deactivate_leader();
        }

        // Periodic refresh (every 5 seconds)
        if last_sync.elapsed() >= Duration::from_secs(5) {
            if !is_viewing {
                app.sync_statuses();
            }
            app.scan_notifications();
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
                    if let Err(e) = handle_key(app, key) {
                        app.show_error(e);
                    }
                }
                Event::Resize(_, _) => {
                    last_size = None;
                }
                _ => {}
            }
        }
    }
}

fn handle_key(
    app: &mut App,
    key: KeyEvent,
) -> Result<()> {
    match &app.mode {
        AppMode::Normal => handle_normal_key(app, key),
        AppMode::CreatingProject(_) => {
            handle_create_project_key(app, key)
        }
        AppMode::BrowsingPath(_) => {
            handle_browse_path_key(app, key)
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
        AppMode::Help => handle_help_key(app, key.code),
        AppMode::NotificationPicker(_) => {
            handle_notification_picker_key(app, key.code)
        }
        AppMode::SessionSwitcher(_) => {
            handle_session_switcher_key(app, key.code)
        }
        AppMode::RenamingSession(_) => {
            handle_rename_session_key(app, key.code)
        }
    }
}

fn handle_normal_key(
    app: &mut App,
    key: KeyEvent,
) -> Result<()> {
    // If leader is active in Normal mode, dispatch to normal leader handler
    if app.leader_active {
        return handle_normal_leader_key(app, key);
    }

    // Ctrl+Space activates leader mode in Normal mode
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char(' ')
    {
        app.activate_leader();
        return Ok(());
    }

    let key = key.code;
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
                    // Toggle collapse on feature
                    // (show/hide sessions)
                    app.toggle_collapse();
                }
                Selection::Session(_, _, _) => {
                    // Enter view on session
                    app.enter_view()?;
                }
            }
        }
        KeyCode::Char('c') => {
            // Start feature (feature or session context)
            match &app.selection {
                Selection::Feature(_, _)
                | Selection::Session(_, _, _) => {
                    app.start_feature()?;
                }
                _ => {}
            }
        }
        KeyCode::Char('x') => {
            match &app.selection {
                Selection::Session(_, _, _) => {
                    // Remove individual session
                    app.remove_session()?;
                }
                Selection::Feature(_, _) => {
                    // Stop feature (kill tmux session)
                    app.stop_feature()?;
                }
                _ => {}
            }
        }
        KeyCode::Char('d') => {
            match &app.selection {
                Selection::Project(pi) => {
                    if let Some(project) =
                        app.store.projects.get(*pi)
                    {
                        let name = project.name.clone();
                        app.mode =
                            AppMode::DeletingProject(name);
                    }
                }
                Selection::Feature(pi, fi) => {
                    if let Some(project) =
                        app.store.projects.get(*pi)
                        && let Some(feature) =
                            project.features.get(*fi)
                        {
                            let pn = project.name.clone();
                            let fn_ = feature.name.clone();
                            app.mode =
                                AppMode::DeletingFeature(
                                    pn, fn_,
                                );
                        }
                }
                Selection::Session(_, _, _) => {
                    // Remove individual session
                    app.remove_session()?;
                }
            }
        }
        KeyCode::Char('s') => {
            match &app.selection {
                Selection::Feature(_, _)
                | Selection::Session(_, _, _) => {
                    app.switch_to_selected()?;
                }
                _ => {}
            }
        }
        KeyCode::Char('t') => {
            // Add terminal session (feature/session context)
            match &app.selection {
                Selection::Feature(_, _)
                | Selection::Session(_, _, _) => {
                    app.add_terminal_session()?;
                }
                _ => {}
            }
        }
        KeyCode::Char('a') => {
            // Add Claude session (feature/session context)
            match &app.selection {
                Selection::Feature(_, _)
                | Selection::Session(_, _, _) => {
                    app.add_claude_session()?;
                }
                _ => {}
            }
        }
        KeyCode::Char('h') => {
            match &app.selection {
                Selection::Project(pi) => {
                    // Collapse the project if expanded
                    if let Some(project) =
                        app.store.projects.get(*pi)
                        && !project.collapsed
                    {
                        app.toggle_collapse();
                    }
                }
                Selection::Feature(pi, _)
                | Selection::Session(pi, _, _) => {
                    // Move selection to parent project
                    app.selection = Selection::Project(*pi);
                }
            }
        }
        KeyCode::Char('l') => {
            match &app.selection {
                Selection::Project(pi) => {
                    // Expand the project if collapsed
                    if let Some(project) =
                        app.store.projects.get(*pi)
                        && project.collapsed
                    {
                        app.toggle_collapse();
                    }
                }
                Selection::Feature(_, _)
                | Selection::Session(_, _, _) => {
                    // Enter view on feature/session
                    app.enter_view()?;
                }
            }
        }
        KeyCode::Char('?') => {
            app.mode = AppMode::Help;
        }
        KeyCode::Char('i') => {
            if !app.pending_inputs.is_empty() {
                app.mode = AppMode::NotificationPicker(0);
            } else {
                app.message =
                    Some("No pending input requests".into());
            }
        }
        KeyCode::Char('r') => {
            if matches!(
                app.selection,
                Selection::Session(_, _, _)
            ) {
                app.start_rename_session();
            } else {
                app.sync_statuses();
                app.scan_notifications();
                app.message =
                    Some("Refreshed statuses".into());
            }
        }
        KeyCode::Char('R') => {
            app.sync_statuses();
            app.scan_notifications();
            app.message =
                Some("Refreshed statuses".into());
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
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && let KeyCode::Char(c) = key.code {
            return Some(TmuxKey::Named(format!("C-{}", c)));
        }

    if key.modifiers.contains(KeyModifiers::ALT)
        && let KeyCode::Char(c) = key.code {
            return Some(TmuxKey::Named(format!("M-{}", c)));
        }

    match key.code {
        KeyCode::Char(c) => {
            Some(TmuxKey::Literal(c.to_string()))
        }
        KeyCode::Enter => {
            Some(TmuxKey::Named("Enter".into()))
        }
        KeyCode::Backspace => {
            Some(TmuxKey::Named("BSpace".into()))
        }
        KeyCode::Tab => Some(TmuxKey::Named("Tab".into())),
        KeyCode::Esc => {
            Some(TmuxKey::Named("Escape".into()))
        }
        KeyCode::Up => Some(TmuxKey::Named("Up".into())),
        KeyCode::Down => Some(TmuxKey::Named("Down".into())),
        KeyCode::Left => Some(TmuxKey::Named("Left".into())),
        KeyCode::Right => {
            Some(TmuxKey::Named("Right".into()))
        }
        KeyCode::Home => Some(TmuxKey::Named("Home".into())),
        KeyCode::End => Some(TmuxKey::Named("End".into())),
        KeyCode::PageUp => {
            Some(TmuxKey::Named("PPage".into()))
        }
        KeyCode::PageDown => {
            Some(TmuxKey::Named("NPage".into()))
        }
        KeyCode::Delete => {
            Some(TmuxKey::Named("DC".into()))
        }
        KeyCode::Insert => {
            Some(TmuxKey::Named("IC".into()))
        }
        KeyCode::F(n) => {
            Some(TmuxKey::Named(format!("F{}", n)))
        }
        _ => None,
    }
}

fn handle_view_key(
    app: &mut App,
    key: KeyEvent,
) -> Result<()> {
    // If leader is active, dispatch to leader handler
    if app.leader_active {
        return handle_leader_key(app, key);
    }

    // Ctrl+Q exits the view
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('q')
    {
        app.exit_view();
        return Ok(());
    }

    // Ctrl+Space activates leader mode
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char(' ')
    {
        app.activate_leader();
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

fn handle_leader_key(
    app: &mut App,
    key: KeyEvent,
) -> Result<()> {
    app.deactivate_leader();

    match key.code {
        KeyCode::Char('q') => {
            app.exit_view();
        }
        KeyCode::Char('t') => {
            // Cycle to next session
            app.view_next_session();
        }
        KeyCode::Char('T') => {
            // Cycle to previous session
            app.view_prev_session();
        }
        KeyCode::Char('s') => {
            // Switch/attach to tmux session directly
            let session = match &app.mode {
                AppMode::Viewing(view) => {
                    view.session.clone()
                }
                _ => return Ok(()),
            };
            app.exit_view();
            if TmuxManager::is_inside_tmux() {
                TmuxManager::switch_client(&session)?;
            } else {
                app.should_switch = Some(session);
            }
        }
        KeyCode::Char('n') => {
            app.view_next_feature()?;
        }
        KeyCode::Char('p') => {
            app.view_prev_feature()?;
        }
        KeyCode::Char('r') => {
            app.sync_statuses();
            app.message =
                Some("Refreshed statuses".into());
        }
        KeyCode::Char('x') => {
            // Stop the current session and exit view
            let session = match &app.mode {
                AppMode::Viewing(view) => {
                    view.session.clone()
                }
                _ => return Ok(()),
            };
            let _ = TmuxManager::kill_session(&session);
            app.exit_view();
            app.sync_statuses();
            app.message = Some("Stopped session".into());
        }
        KeyCode::Char('i') => {
            app.exit_view();
            if !app.pending_inputs.is_empty() {
                app.mode = AppMode::NotificationPicker(0);
            } else {
                app.message =
                    Some("No pending input requests".into());
            }
        }
        KeyCode::Char('w') => {
            app.open_session_switcher();
        }
        KeyCode::Char('?') => {
            app.exit_view();
            app.mode = AppMode::Help;
        }
        // Any unbound key or Esc cancels (already deactivated above)
        _ => {}
    }

    Ok(())
}

fn handle_session_switcher_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.cancel_session_switcher();
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::SessionSwitcher(ref mut state) =
                app.mode
            {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected =
                        (state.selected + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::SessionSwitcher(ref mut state) =
                app.mode
            {
                let len = state.sessions.len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
        }
        KeyCode::Enter => {
            app.switch_from_switcher();
        }
        KeyCode::Char('r') => {
            app.start_rename_from_switcher();
        }
        _ => {}
    }
    Ok(())
}

fn handle_rename_session_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc => {
            app.cancel_rename_session();
        }
        KeyCode::Enter => {
            app.apply_rename_session()?;
        }
        KeyCode::Backspace => {
            if let AppMode::RenamingSession(state) =
                &mut app.mode
            {
                state.input.pop();
            }
        }
        KeyCode::Char(c) => {
            if let AppMode::RenamingSession(state) =
                &mut app.mode
            {
                state.input.push(c);
            }
        }
        _ => {}
    }
    Ok(())
}

fn handle_normal_leader_key(
    app: &mut App,
    key: KeyEvent,
) -> Result<()> {
    app.deactivate_leader();

    match key.code {
        KeyCode::Char('i') => {
            if !app.pending_inputs.is_empty() {
                app.mode = AppMode::NotificationPicker(0);
            } else {
                app.message =
                    Some("No pending input requests".into());
            }
        }
        KeyCode::Char('?') => {
            app.mode = AppMode::Help;
        }
        KeyCode::Char('r') => {
            app.sync_statuses();
            app.scan_notifications();
            app.message =
                Some("Refreshed statuses".into());
        }
        _ => {}
    }

    Ok(())
}

fn handle_notification_picker_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.mode = AppMode::Normal;
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if let AppMode::NotificationPicker(ref mut idx) =
                app.mode
            {
                let len = app.pending_inputs.len();
                if len > 0 {
                    *idx = (*idx + 1) % len;
                }
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if let AppMode::NotificationPicker(ref mut idx) =
                app.mode
            {
                let len = app.pending_inputs.len();
                if len > 0 {
                    *idx =
                        if *idx == 0 { len - 1 } else { *idx - 1 };
                }
            }
        }
        KeyCode::Enter => {
            app.handle_notification_select()?;
        }
        _ => {}
    }
    Ok(())
}

fn handle_browse_path_key(
    app: &mut App,
    key: KeyEvent,
) -> Result<()> {
    match key.code {
        KeyCode::Esc => {
            app.cancel_browse_path();
        }
        KeyCode::Tab => {
            // Return to create project dialog on Name step
            app.cancel_browse_path();
            if let AppMode::CreatingProject(state) =
                &mut app.mode
            {
                state.step =
                    app::CreateProjectStep::Name;
            }
        }
        KeyCode::Char(' ') => {
            app.confirm_browse_path();
        }
        KeyCode::Enter => {
            // If the current entry is a directory, let the
            // explorer handle it (enters the dir). Otherwise
            // confirm with the current cwd.
            let is_dir = match &app.mode {
                AppMode::BrowsingPath(state) => {
                    state.explorer.current().is_dir()
                }
                _ => false,
            };
            if is_dir {
                if let AppMode::BrowsingPath(state) =
                    &mut app.mode
                {
                    let _ = state.explorer.handle(
                        &Event::Key(key),
                    );
                }
            } else {
                app.confirm_browse_path();
            }
        }
        _ => {
            if let AppMode::BrowsingPath(state) = &mut app.mode
            {
                let _ =
                    state.explorer.handle(&Event::Key(key));
            }
        }
    }
    Ok(())
}

fn handle_create_project_key(
    app: &mut App,
    key: KeyEvent,
) -> Result<()> {
    use app::CreateProjectStep;

    // Ctrl+B opens file browser on the Path step
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('b')
    {
        let is_path_step = matches!(
            &app.mode,
            AppMode::CreatingProject(s)
                if matches!(s.step, CreateProjectStep::Path)
        );
        if is_path_step {
            let browse = std::mem::replace(
                &mut app.mode,
                AppMode::Normal,
            );
            if let AppMode::CreatingProject(state) = browse {
                app.start_browse_path(state);
            }
            return Ok(());
        }
    }

    match key.code {
        KeyCode::Esc => {
            app.cancel_create();
        }
        KeyCode::Enter => {
            let should_advance = match &app.mode {
                AppMode::CreatingProject(state) => {
                    matches!(
                        state.step,
                        CreateProjectStep::Name
                    )
                }
                _ => false,
            };

            if should_advance {
                if let AppMode::CreatingProject(state) =
                    &mut app.mode
                {
                    state.step = CreateProjectStep::Path;
                }
            } else {
                app.create_project()?;
            }
        }
        KeyCode::Tab => {
            if let AppMode::CreatingProject(state) =
                &mut app.mode
            {
                state.step = match state.step {
                    CreateProjectStep::Name => {
                        CreateProjectStep::Path
                    }
                    CreateProjectStep::Path => {
                        CreateProjectStep::Name
                    }
                };
            }
        }
        KeyCode::Backspace => {
            if let AppMode::CreatingProject(state) =
                &mut app.mode
            {
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
            if let AppMode::CreatingProject(state) =
                &mut app.mode
            {
                match state.step {
                    CreateProjectStep::Name => {
                        state.name.push(c)
                    }
                    CreateProjectStep::Path => {
                        state.path.push(c)
                    }
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
    use app::CreateFeatureStep;
    use crate::project::VibeMode;

    let step = match &app.mode {
        AppMode::CreatingFeature(state) => {
            state.step.clone()
        }
        _ => return Ok(()),
    };

    match step {
        CreateFeatureStep::Branch => match key {
            KeyCode::Esc => {
                app.cancel_create();
            }
            KeyCode::Enter => {
                // Validate branch then advance to Mode step
                let empty = match &app.mode {
                    AppMode::CreatingFeature(s) => {
                        s.branch.is_empty()
                    }
                    _ => return Ok(()),
                };
                if empty {
                    app.message = Some(
                        "Branch name cannot be empty".into(),
                    );
                } else if let AppMode::CreatingFeature(
                    state,
                ) = &mut app.mode
                {
                    state.step = CreateFeatureStep::Mode;
                }
            }
            KeyCode::Backspace => {
                if let AppMode::CreatingFeature(state) =
                    &mut app.mode
                {
                    state.branch.pop();
                }
            }
            KeyCode::Char(c) => {
                if let AppMode::CreatingFeature(state) =
                    &mut app.mode
                {
                    state.branch.push(c);
                }
            }
            _ => {}
        },
        CreateFeatureStep::Mode => match key {
            KeyCode::Esc => {
                // Go back to Branch step
                if let AppMode::CreatingFeature(state) =
                    &mut app.mode
                {
                    state.step = CreateFeatureStep::Branch;
                }
            }
            KeyCode::Enter => {
                app.create_feature()?;
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let AppMode::CreatingFeature(state) =
                    &mut app.mode
                {
                    state.mode_index = (state.mode_index + 1)
                        % VibeMode::ALL.len();
                    state.mode =
                        VibeMode::ALL[state.mode_index]
                            .clone();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if let AppMode::CreatingFeature(state) =
                    &mut app.mode
                {
                    state.mode_index =
                        if state.mode_index == 0 {
                            VibeMode::ALL.len() - 1
                        } else {
                            state.mode_index - 1
                        };
                    state.mode =
                        VibeMode::ALL[state.mode_index]
                            .clone();
                }
            }
            _ => {}
        },
    }
    Ok(())
}

fn handle_help_key(
    app: &mut App,
    key: KeyCode,
) -> Result<()> {
    match key {
        KeyCode::Esc
        | KeyCode::Char('q')
        | KeyCode::Char('h')
        | KeyCode::Char('?') => {
            app.mode = AppMode::Normal;
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
