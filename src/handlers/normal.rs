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

    let key = key.code;
    match key {
        KeyCode::Char('q') | KeyCode::Esc => {
            app.should_quit = true;
        }
        KeyCode::Char('N') => {
            app.start_create_project();
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
            match &app.selection {
                Selection::Feature(_, _)
                | Selection::Session(_, _, _) => {
                    app.switch_to_selected()?;
                }
                _ => {}
            }
        }
        KeyCode::Char('S') => {
            app.pick_session();
        }
        KeyCode::Char('t') => {
            match &app.selection {
                Selection::Feature(_, _)
                | Selection::Session(_, _, _) => {
                    app.add_terminal_session()?;
                }
                _ => {}
            }
        }
        KeyCode::Char('a') => {
            match &app.selection {
                Selection::Feature(_, _)
                | Selection::Session(_, _, _) => {
                    app.add_claude_session()?;
                }
                _ => {}
            }
        }
        KeyCode::Char('v') => {
            match &app.selection {
                Selection::Feature(_, _)
                | Selection::Session(_, _, _) => {
                    app.add_nvim_session()?;
                }
                _ => {}
            }
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
        KeyCode::Char('h') => {
            match &app.selection {
                Selection::Project(pi) => {
                    if let Some(project) =
                        app.store.projects.get(*pi)
                        && !project.collapsed
                    {
                        app.toggle_collapse();
                    }
                }
                Selection::Feature(pi, _)
                | Selection::Session(pi, _, _) => {
                    app.selection = Selection::Project(*pi);
                }
            }
        }
        KeyCode::Char('l') => {
            if let Selection::Project(pi) = &app.selection {
                if let Some(project) =
                    app.store.projects.get(*pi)
                    && project.collapsed
                {
                    app.toggle_collapse();
                }
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
