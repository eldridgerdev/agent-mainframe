use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::App;
use crate::app::AppMode;
use crate::project::SessionKind;
use crate::tmux::TmuxManager;

enum TmuxKey {
    Literal(String),
    Named(String),
}

fn crossterm_key_to_tmux(key: &KeyEvent) -> Option<TmuxKey> {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && let KeyCode::Char(c) = key.code
    {
        return Some(TmuxKey::Named(format!("C-{}", c)));
    }

    if key.modifiers.contains(KeyModifiers::ALT)
        && let KeyCode::Char(c) = key.code
    {
        return Some(TmuxKey::Named(format!("M-{}", c)));
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

pub fn handle_view_key(app: &mut App, key: KeyEvent, visible_rows: u16) -> Result<()> {
    if app.leader_active {
        return handle_leader_key(app, key, visible_rows);
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        app.exit_view();
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(' ') {
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
        AppMode::Viewing(view) => (view.session.clone(), view.window.clone()),
        _ => return Ok(()),
    };

    if let Some(tmux_key) = crossterm_key_to_tmux(&key) {
        let result = match tmux_key {
            TmuxKey::Literal(text) => TmuxManager::send_literal(&session, &window, &text),
            TmuxKey::Named(name) => TmuxManager::send_key_name(&session, &window, &name),
        };
        if let Err(e) = result {
            app.show_error(e);
        } else if key.code == KeyCode::Enter
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::ALT)
        {
            let is_codex_window = app
                .store
                .projects
                .iter()
                .flat_map(|p| p.features.iter())
                .filter(|f| f.tmux_session == session)
                .flat_map(|f| f.sessions.iter())
                .any(|s| s.kind == SessionKind::Codex && s.tmux_window == window);
            if is_codex_window {
                app.note_codex_prompt_submit(&session, &window);
            }
        }
    }

    Ok(())
}

fn handle_scroll_key(app: &mut App, key: KeyEvent, visible_rows: u16) -> Result<()> {
    let (session, window, passthrough) = match &app.mode {
        AppMode::Viewing(view) => (
            view.session.clone(),
            view.window.clone(),
            view.scroll_passthrough,
        ),
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
            if passthrough && let Some(tmux_key) = crossterm_key_to_tmux(&key) {
                let _ = match tmux_key {
                    TmuxKey::Literal(text) => TmuxManager::send_literal(&session, &window, &text),
                    TmuxKey::Named(name) => TmuxManager::send_key_name(&session, &window, &name),
                };
            }
        }
    }
    Ok(())
}

fn handle_leader_key(app: &mut App, key: KeyEvent, visible_rows: u16) -> Result<()> {
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
        KeyCode::Char('n') => {
            app.view_next_feature()?;
        }
        KeyCode::Char('p') => {
            app.view_prev_feature()?;
        }
        KeyCode::Char('r') => {
            app.sync_statuses();
            app.message = Some("Refreshed statuses".into());
        }
        KeyCode::Char('x') => {
            let session = match &app.mode {
                AppMode::Viewing(view) => view.session.clone(),
                _ => return Ok(()),
            };
            let _ = TmuxManager::kill_session(&session);
            app.exit_view();
            app.sync_statuses();
            app.message = Some("Stopped session".into());
        }
        KeyCode::Char('i') => {
            if app.pending_inputs.is_empty() {
                app.message = Some("No pending input requests".into());
            } else {
                let view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                    AppMode::Viewing(v) => v,
                    other => {
                        app.mode = other;
                        return Ok(());
                    }
                };
                app.mode = AppMode::NotificationPicker(0, Some(view));
            }
        }
        KeyCode::Char('s') => {
            app.open_steering_prompt_from_view()?;
        }
        KeyCode::Char('g') => {
            app.trigger_summary_for_selected()?;
        }
        KeyCode::Char('w') => {
            app.open_session_switcher();
        }
        KeyCode::Char('h') => {
            let view_state = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::Viewing(v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            app.open_bookmark_picker(Some(view_state));
        }
        KeyCode::Char('H') => {
            app.bookmark_current_session()?;
        }
        KeyCode::Char('M') => {
            app.unbookmark_current_session()?;
        }
        KeyCode::Char(c @ '1'..='9') => {
            let slot = (c as u8 - b'0') as usize;
            app.jump_to_bookmark(slot)?;
        }
        KeyCode::Char('/') => {
            let view_state = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::Viewing(v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            app.open_command_picker(Some(view_state));
        }
        KeyCode::Char('a') => {
            let view_state = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::Viewing(v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            app.open_command_picker_with_focus(
                Some(view_state),
                crate::app::CommandPickerFocus::Local,
            );
        }
        KeyCode::Char('?') => {
            let view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::Viewing(v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            app.mode = AppMode::Help(Some(view));
        }
        KeyCode::Char('o') | KeyCode::Char('S') => {
            app.toggle_scroll_mode(visible_rows);
        }
        KeyCode::Char('f') => {
            app.trigger_final_review()?;
        }
        KeyCode::Char('d') => {
            app.open_diff_viewer()?;
        }
        KeyCode::Char('D') => {
            let view = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::Viewing(v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            app.open_debug_log(Some(view));
        }
        KeyCode::Char('l') => {
            app.open_latest_prompt_from_view();
        }
        KeyCode::Char('b') => {
            app.toggle_sidebar_in_view();
        }
        KeyCode::Char('v') => {
            app.toggle_expanded_todos_in_view();
        }
        KeyCode::Char('m') => {
            app.open_markdown_viewer_from_view()?;
        }
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::Path;
    use std::process::Command;

    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tempfile::TempDir;

    use crate::app::{CommandAction, ViewState, analyze_prompt};
    use crate::project::{
        AgentKind, Feature, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode,
    };
    use crate::traits::{MockTmuxOps, MockWorktreeOps};

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

    #[test]
    fn leader_d_opens_diff_viewer_and_escape_closes_it() {
        let repo = init_repo_with_branch_change();
        let mut app = app_for_viewing_repo(repo.path());

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('d')), 20).unwrap();

        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if state.branch == "feature"
                    && state.base_ref == "main"
                    && state.files.iter().any(|file| file.path == "src.txt")
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, KeyCode::Char('v')).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if matches!(state.layout, crate::app::DiffViewerLayout::SideBySide)
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, KeyCode::Esc).unwrap();
        assert!(matches!(app.mode, AppMode::Viewing(_)));

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('d')), 20).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if matches!(state.layout, crate::app::DiffViewerLayout::SideBySide)
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, KeyCode::Esc).unwrap();
        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn new_file_forces_unified_without_losing_side_by_side_preference() {
        let repo = init_repo_with_branch_change();
        let mut app = app_for_viewing_repo(repo.path());

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('d')), 20).unwrap();
        crate::handlers::handle_diff_viewer_key(&mut app, KeyCode::Char('v')).unwrap();

        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if matches!(state.layout, crate::app::DiffViewerLayout::SideBySide)
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, KeyCode::Char('j')).unwrap();

        assert!(app.diff_viewer_selected_file_is_new());
        assert!(matches!(
            app.diff_viewer_layout(),
            Some(crate::app::DiffViewerLayout::Unified)
        ));
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if matches!(state.layout, crate::app::DiffViewerLayout::SideBySide)
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, KeyCode::Char('v')).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if matches!(state.layout, crate::app::DiffViewerLayout::SideBySide)
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, KeyCode::Char('k')).unwrap();
        assert!(!app.diff_viewer_selected_file_is_new());
        assert!(matches!(
            app.diff_viewer_layout(),
            Some(crate::app::DiffViewerLayout::SideBySide)
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, KeyCode::Esc).unwrap();
        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('d')), 20).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if matches!(state.layout, crate::app::DiffViewerLayout::SideBySide)
        ));
    }

    #[test]
    fn leader_s_opens_steering_prompt_from_view() {
        let repo = TempDir::new().unwrap();
        std::fs::create_dir_all(repo.path().join(".claude")).unwrap();
        std::fs::write(
            repo.path().join(".claude").join("latest-prompt.txt"),
            "Scope the change.\nDone when cargo check passes.",
        )
        .unwrap();

        let mut app = app_for_viewing_repo(repo.path());

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('s')), 20).unwrap();

        match &app.mode {
            AppMode::SteeringPrompt(state) => {
                assert_eq!(state.view.session, "amf-feature");
                assert_eq!(state.workdir, repo.path());
                assert_eq!(
                    state.editor.text(),
                    "Scope the change.\nDone when cargo check passes."
                );
                assert_eq!(
                    state.prompt_analysis.score,
                    analyze_prompt("Scope the change.\nDone when cargo check passes.").score
                );
            }
            _ => panic!("expected SteeringPrompt mode"),
        }
    }

    #[test]
    fn leader_l_opens_latest_prompt_dialog_with_saved_prompt() {
        let repo = init_repo_with_branch_change();
        let claude_dir = repo.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("latest-prompt.txt"),
            "Resume the current task from the saved prompt.",
        )
        .unwrap();

        let mut app = app_for_viewing_repo(repo.path());
        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('l')), 20).unwrap();

        match &app.mode {
            AppMode::LatestPrompt(state) => {
                assert_eq!(
                    state.prompts.first().map(|entry| entry.text.as_str()),
                    Some("Resume the current task from the saved prompt.")
                );
                assert_eq!(state.view.session, "amf-feature");
            }
            _ => panic!("expected LatestPrompt mode"),
        }
    }

    #[test]
    fn leader_a_opens_command_picker_focused_on_local_actions() {
        let repo = init_repo_with_branch_change();
        let mut app = app_for_viewing_repo(repo.path());

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('a')), 20).unwrap();

        match &app.mode {
            AppMode::CommandPicker(state) => assert!(matches!(
                state.commands.get(state.selected).map(|entry| &entry.action),
                Some(CommandAction::Local { .. })
            )),
            _ => panic!("expected command picker"),
        }
    }

    #[test]
    fn leader_v_toggles_expanded_todos() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('v')), 20).unwrap();

        match &app.mode {
            AppMode::Viewing(view) => {
                assert!(view.todos_expanded);
            }
            _ => panic!("expected Viewing mode"),
        }

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('v')), 20).unwrap();

        match &app.mode {
            AppMode::Viewing(view) => assert!(!view.todos_expanded),
            _ => panic!("expected Viewing mode"),
        }
    }

    #[test]
    fn leader_b_toggles_sidebar_visibility() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('b')), 20).unwrap();

        match &app.mode {
            AppMode::Viewing(view) => {
                assert!(!view.sidebar_visible);
                assert!(!view.todos_expanded);
            }
            _ => panic!("expected Viewing mode"),
        }

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('b')), 20).unwrap();

        match &app.mode {
            AppMode::Viewing(view) => assert!(view.sidebar_visible),
            _ => panic!("expected Viewing mode"),
        }
    }

    fn init_repo_with_branch_change() -> TempDir {
        let repo = TempDir::new().unwrap();
        git(repo.path(), &["init", "--initial-branch=main"]);
        git(repo.path(), &["config", "user.name", "AMF Test"]);
        git(repo.path(), &["config", "user.email", "amf@example.com"]);
        std::fs::write(repo.path().join("src.txt"), "base\n").unwrap();
        git(repo.path(), &["add", "src.txt"]);
        git(repo.path(), &["commit", "-m", "initial"]);
        git(repo.path(), &["checkout", "-b", "feature"]);
        std::fs::write(repo.path().join("src.txt"), "base\nfeature\n").unwrap();
        std::fs::write(repo.path().join("z_new.txt"), "brand new\n").unwrap();
        repo
    }

    fn app_for_viewing_repo(repo: &Path) -> App {
        let mut feature = Feature::new(
            "feature".to_string(),
            "feature".to_string(),
            repo.to_path_buf(),
            false,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
        );
        feature.status = ProjectStatus::Active;
        let session = feature.add_session(SessionKind::Claude).clone();

        let mut project = Project::new(
            "demo".to_string(),
            repo.to_path_buf(),
            true,
            AgentKind::Claude,
        );
        project.features.push(feature);

        let store = ProjectStore {
            version: 5,
            projects: vec![project],
            session_bookmarks: vec![],
            extra: HashMap::new(),
        };

        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.mode = AppMode::Viewing(ViewState::new(
            "demo".to_string(),
            "feature".to_string(),
            "amf-feature".to_string(),
            session.tmux_window.clone(),
            session.label.clone(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            false,
        ));
        app
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
