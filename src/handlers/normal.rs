use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, Selection};

pub fn handle_normal_key(
    app: &mut App,
    key: KeyEvent,
) -> Result<()> {
    if app.leader_active {
        return handle_normal_leader_key(app, key);
    }

    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char(' ')
    {
        app.activate_leader();
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) {
        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                app.select_next_feature();
                app.message = None;
                return Ok(());
            }
            KeyCode::Up | KeyCode::Char('k') => {
                app.select_prev_feature();
                app.message = None;
                return Ok(());
            }
            _ => {}
        }
    }

    // Apply keybinding remaps from extension config.
    // Build a map from the user-defined key → the canonical
    // action string, then convert pressed char → canonical
    // char using default_key_for_action().
    let raw_key = key.code;
    let remapped_key = if let KeyCode::Char(c) = raw_key {
        let bindings = &app.active_extension.keybindings;
        let canonical_char = bindings
            .iter()
            .find(|&(_, &v)| v == c)
            .and_then(|(action, _)| {
                default_key_for_action(action)
            });
        if let Some(canonical) = canonical_char {
            KeyCode::Char(canonical)
        } else {
            raw_key
        }
    } else {
        raw_key
    };
    let key = remapped_key;
    match key {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
        }
        KeyCode::Char('N') => {
            app.start_create_project();
        }
        KeyCode::Char('O') => {
            app.open_settings_project()?;
        }
        KeyCode::Char('n') => {
            if app.selected_project().is_some() {
                app.start_create_feature();
            }
        }
        KeyCode::Enter => {
            match &app.selection {
                Selection::Project(_) => {
                    app.toggle_collapse();
                }
                Selection::Feature(_, _) => {
                    app.toggle_collapse();
                }
                Selection::Session(_, _, _) => {
                    app.enter_view()?;
                }
            }
        }
        KeyCode::Char('c') => {
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
                    app.remove_session()?;
                }
                Selection::Feature(_, _) => {
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
                    app.remove_session()?;
                }
            }
        }
        KeyCode::Char('s') => {
            app.open_session_picker()?;
        }
        KeyCode::Char('m') => {
            match &app.selection {
                Selection::Feature(_, _)
                | Selection::Session(_, _, _) => {
                    app.create_memo()?;
                }
                _ => {}
            }
        }
        KeyCode::Char('h') | KeyCode::Left => {
            if let Selection::Project(pi) = &app.selection
                && let Some(project) =
                    app.store.projects.get(*pi)
                && !project.collapsed
            {
                app.toggle_collapse();
            }
        }
        KeyCode::Char('l') | KeyCode::Right => {
            if let Selection::Project(pi) = &app.selection
                && let Some(project) =
                    app.store.projects.get(*pi)
                && project.collapsed
            {
                app.toggle_collapse();
            }
        }
        KeyCode::Char('?') => {
            app.mode = AppMode::Help;
        }
        KeyCode::Char('/') => {
            app.start_search();
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
        KeyCode::Char('f') => {
            app.session_filter = app.session_filter.next();
            app.message = Some(format!(
                "Filter: {}",
                app.session_filter.display_name()
            ));
        }
        _ => {}
    }
    Ok(())
}

/// Returns the default canonical key character for a named
/// action. These correspond to the hardcoded keys in
/// handle_normal_key().
fn default_key_for_action(action: &str) -> Option<char> {
    match action {
        "quit" => Some('q'),
        "create_project" => Some('N'),
        "create_feature" => Some('n'),
        "start_session" => Some('c'),
        "stop_session" => Some('x'),
        "delete" => Some('d'),
        "sessions" => Some('s'),
        "help" => Some('?'),
        "search" => Some('/'),
        "refresh" => Some('r'),
        "filter" => Some('f'),
        _ => None,
    }
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
        KeyCode::Char('/') => {
            app.open_command_picker(None);
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
