use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::AppMode;
use crate::app::App;
use crate::tmux::TmuxManager;

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

pub fn handle_view_key(
    app: &mut App,
    key: KeyEvent,
    visible_rows: u16,
) -> Result<()> {
    if app.leader_active {
        return handle_leader_key(app, key, visible_rows);
    }

    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char('q')
    {
        app.exit_view();
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL)
        && key.code == KeyCode::Char(' ')
    {
        app.activate_leader();
        return Ok(());
    }

    let scroll_mode = match &app.mode {
        AppMode::Viewing(view) => view.scroll_mode,
        _ => false,
    };

    if scroll_mode {
        return handle_scroll_key(app, key, visible_rows);
    }

    let (session, window) = match &app.mode {
        AppMode::Viewing(view) => {
            (view.session.clone(), view.window.clone())
        }
        _ => return Ok(()),
    };

    if let Some(tmux_key) = crossterm_key_to_tmux(&key) {
        let result = match tmux_key {
            TmuxKey::Literal(text) => {
                TmuxManager::send_literal(
                    &session, &window, &text,
                )
            }
            TmuxKey::Named(name) => {
                TmuxManager::send_key_name(
                    &session, &window, &name,
                )
            }
        };
        if let Err(e) = result {
            app.show_error(e);
        }
    }

    Ok(())
}

fn handle_scroll_key(
    app: &mut App,
    key: KeyEvent,
    visible_rows: u16,
) -> Result<()> {
    let (session, window, passthrough) = match &app.mode {
        AppMode::Viewing(view) => {
            (view.session.clone(), view.window.clone(), view.scroll_passthrough)
        }
        _ => return Ok(()),
    };

    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.toggle_scroll_mode(visible_rows);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if passthrough {
                TmuxManager::send_key_name(&session, &window, "PPage")?;
            } else {
                app.scroll_up(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if passthrough {
                TmuxManager::send_key_name(&session, &window, "NPage")?;
            } else {
                app.scroll_down(1, visible_rows);
            }
        }
        KeyCode::PageUp => {
            if passthrough {
                TmuxManager::send_key_name(&session, &window, "PPage")?;
            } else {
                app.scroll_up(visible_rows as usize);
            }
        }
        KeyCode::PageDown => {
            if passthrough {
                TmuxManager::send_key_name(&session, &window, "NPage")?;
            } else {
                app.scroll_down(visible_rows as usize, visible_rows);
            }
        }
        KeyCode::Home => {
            if passthrough {
                TmuxManager::send_key_name(&session, &window, "Home")?;
            } else {
                app.scroll_to_top();
            }
        }
        KeyCode::End => {
            if passthrough {
                TmuxManager::send_key_name(&session, &window, "End")?;
            } else {
                app.scroll_to_bottom(visible_rows);
            }
        }
        _ => {
            if passthrough
                && let Some(tmux_key) = crossterm_key_to_tmux(&key)
            {
                let _ = match tmux_key {
                    TmuxKey::Literal(text) => {
                        TmuxManager::send_literal(&session, &window, &text)
                    }
                    TmuxKey::Named(name) => {
                        TmuxManager::send_key_name(&session, &window, &name)
                    }
                };
            }
        }
    }
    Ok(())
}

fn handle_leader_key(
    app: &mut App,
    key: KeyEvent,
    visible_rows: u16,
) -> Result<()> {
    app.deactivate_leader();

    match key.code {
        KeyCode::Char('q') => {
            app.exit_view();
        }
        KeyCode::Char('t') => {
            app.view_next_session();
        }
        KeyCode::Char('T') => {
            app.view_prev_session();
        }
        KeyCode::Char('s') => {
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
        KeyCode::Char('/') => {
            let view_state = match std::mem::replace(
                &mut app.mode,
                AppMode::Normal,
            ) {
                AppMode::Viewing(v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            app.open_command_picker(Some(view_state));
        }
        KeyCode::Char('?') => {
            app.exit_view();
            app.mode = AppMode::Help;
        }
        KeyCode::Char('o') | KeyCode::Char('S') => {
            app.toggle_scroll_mode(visible_rows);
        }
        KeyCode::Char('f') => {
            app.trigger_final_review()?;
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    fn alt(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::ALT)
    }

    // ── crossterm_key_to_tmux ─────────────────────────────────

    #[test]
    fn ctrl_c_becomes_named_c_c() {
        let k = ctrl(KeyCode::Char('c'));
        assert!(matches!(
            crossterm_key_to_tmux(&k),
            Some(TmuxKey::Named(s)) if s == "C-c"
        ));
    }

    #[test]
    fn alt_x_becomes_named_m_x() {
        let k = alt(KeyCode::Char('x'));
        assert!(matches!(
            crossterm_key_to_tmux(&k),
            Some(TmuxKey::Named(s)) if s == "M-x"
        ));
    }

    #[test]
    fn regular_char_becomes_literal() {
        let k = key(KeyCode::Char('a'));
        assert!(matches!(
            crossterm_key_to_tmux(&k),
            Some(TmuxKey::Literal(s)) if s == "a"
        ));
    }

    #[test]
    fn enter_becomes_named_enter() {
        let k = key(KeyCode::Enter);
        assert!(matches!(
            crossterm_key_to_tmux(&k),
            Some(TmuxKey::Named(s)) if s == "Enter"
        ));
    }

    #[test]
    fn f5_becomes_named_f5() {
        let k = key(KeyCode::F(5));
        assert!(matches!(
            crossterm_key_to_tmux(&k),
            Some(TmuxKey::Named(s)) if s == "F5"
        ));
    }

    #[test]
    fn backspace_becomes_named_bspace() {
        let k = key(KeyCode::Backspace);
        assert!(matches!(
            crossterm_key_to_tmux(&k),
            Some(TmuxKey::Named(s)) if s == "BSpace"
        ));
    }

    #[test]
    fn unknown_key_returns_none() {
        // Null is not handled in the match
        let k = key(KeyCode::Null);
        assert!(crossterm_key_to_tmux(&k).is_none());
    }
}
