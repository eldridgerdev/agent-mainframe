use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::time::Instant;

use crate::app::App;
use crate::app::AppMode;
use crate::project::SessionKind;
use crate::tmux::TmuxManager;

const VIEW_INPUT_BATCH_MAX_LEN: usize = 64;
const VIEW_FAST_SCROLL_STEP: usize = 10;

enum TmuxKey {
    Literal(String),
    Named(String),
}

fn crossterm_key_to_tmux(key: &KeyEvent) -> Option<TmuxKey> {
    if key.code == KeyCode::Enter && key.kind == crossterm::event::KeyEventKind::Repeat {
        return None;
    }

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
        KeyCode::Char('\n' | '\r') => None,
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

fn send_key_name(
    app: &mut App,
    session: &str,
    window: &str,
    key_name: &str,
    refresh_after_send: bool,
) -> Result<()> {
    let started_at = Instant::now();
    let result = app.tmux.send_key_name(session, window, key_name);
    app.perf
        .record_duration("view.send_key_name", started_at.elapsed());
    if result.is_ok() && refresh_after_send {
        app.request_view_snapshot_pane_burst();
    }
    result
}

fn flush_view_input_batch(app: &mut App) -> Result<()> {
    let _ = app.flush_view_input_batch()?;
    Ok(())
}

fn forward_tmux_key(app: &mut App, key: &KeyEvent, session: &str, window: &str) -> Result<bool> {
    let Some(tmux_key) = crossterm_key_to_tmux(key) else {
        return Ok(false);
    };

    match tmux_key {
        TmuxKey::Literal(text) => {
            if !app.pending_view_input_targets(session, window) {
                flush_view_input_batch(app)?;
            }
            app.queue_view_literal_input(session, window, &text);
            if app.pending_view_input_len() >= VIEW_INPUT_BATCH_MAX_LEN {
                flush_view_input_batch(app)?;
            }
            Ok(false)
        }
        TmuxKey::Named(name) => {
            flush_view_input_batch(app)?;
            let refresh_after_send = !(key.code == KeyCode::Backspace
                && key.kind == crossterm::event::KeyEventKind::Repeat);
            send_key_name(app, session, window, &name, refresh_after_send)?;
            Ok(key.code == KeyCode::Enter
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::ALT))
        }
    }
}

pub fn handle_view_key(app: &mut App, key: KeyEvent, visible_rows: u16) -> Result<()> {
    if app.leader_active {
        flush_view_input_batch(app)?;
        return handle_leader_key(app, key, visible_rows);
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q') {
        flush_view_input_batch(app)?;
        app.exit_view();
        return Ok(());
    }

    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(' ') {
        flush_view_input_batch(app)?;
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

    // Compose interception: printable keys in an agent view open the
    // local compose box instead of typing into the harness's input.
    // Navigation, Enter, Esc, and modified keys still pass through so
    // harness-owned dialogs stay drivable.
    if let AppMode::Viewing(view) = &app.mode
        && app.compose_intercept_active(view)
        && let KeyCode::Char(c) = key.code
        && !key.modifiers.contains(KeyModifiers::CONTROL)
        && !key.modifiers.contains(KeyModifiers::ALT)
    {
        flush_view_input_batch(app)?;
        return app.open_compose_from_view(Some(c));
    }

    let (session, window) = match &app.mode {
        AppMode::Viewing(view) => (view.session.clone(), view.window.clone()),
        _ => return Ok(()),
    };

    let result = forward_tmux_key(app, &key, &session, &window);
    if let Err(e) = result {
        app.show_error(e);
    } else if result.unwrap_or(false) {
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
            flush_view_input_batch(app)?;
            app.toggle_scroll_mode(visible_rows);
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                if passthrough {
                    send_key_name(app, &session, &window, "PPage", true)?;
                } else {
                    flush_view_input_batch(app)?;
                    app.scroll_up(VIEW_FAST_SCROLL_STEP);
                }
                return Ok(());
            }
            if passthrough {
                send_key_name(app, &session, &window, "PPage", true)?;
            } else {
                flush_view_input_batch(app)?;
                app.scroll_up(1);
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                if passthrough {
                    send_key_name(app, &session, &window, "NPage", true)?;
                } else {
                    flush_view_input_batch(app)?;
                    app.scroll_down(VIEW_FAST_SCROLL_STEP, visible_rows);
                }
                return Ok(());
            }
            if passthrough {
                send_key_name(app, &session, &window, "NPage", true)?;
            } else {
                flush_view_input_batch(app)?;
                app.scroll_down(1, visible_rows);
            }
        }
        KeyCode::PageUp => {
            if passthrough {
                send_key_name(app, &session, &window, "PPage", true)?;
            } else {
                flush_view_input_batch(app)?;
                app.scroll_up(visible_rows as usize);
            }
        }
        KeyCode::PageDown => {
            if passthrough {
                send_key_name(app, &session, &window, "NPage", true)?;
            } else {
                flush_view_input_batch(app)?;
                app.scroll_down(visible_rows as usize, visible_rows);
            }
        }
        KeyCode::Home => {
            if passthrough {
                send_key_name(app, &session, &window, "Home", true)?;
            } else {
                flush_view_input_batch(app)?;
                app.scroll_to_top();
            }
        }
        KeyCode::End => {
            if passthrough {
                send_key_name(app, &session, &window, "End", true)?;
            } else {
                flush_view_input_batch(app)?;
                app.scroll_to_bottom(visible_rows);
            }
        }
        _ => {
            if passthrough {
                let _ = forward_tmux_key(app, &key, &session, &window);
            }
        }
    }
    Ok(())
}

fn handle_leader_key(app: &mut App, key: KeyEvent, visible_rows: u16) -> Result<()> {
    app.deactivate_leader();

    // Next/prev feature navigation has no default binding. Dispatch it only
    // when the user has configured a key for it. This runs before the static
    // match so a configured key wins over any static binding on that char.
    if let KeyCode::Char(c) = key.code {
        let kb = &app.active_extension.keybindings;
        if kb.get("next_feature") == Some(&c) {
            app.view_next_feature()?;
            return Ok(());
        }
        if kb.get("prev_feature") == Some(&c) {
            app.view_prev_feature()?;
            return Ok(());
        }
    }

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
        KeyCode::Char('r') => {
            app.sync_statuses();
            app.push_toast_success("Refreshed statuses");
        }
        KeyCode::Char('V') => {
            app.check_pending_diff_review()?;
        }
        KeyCode::Char('R') => {
            app.refresh_view_sizing()?;
        }
        KeyCode::Char('x') => {
            let session = match &app.mode {
                AppMode::Viewing(view) => view.session.clone(),
                _ => return Ok(()),
            };
            let _ = TmuxManager::kill_session(&session);
            app.exit_view();
            app.sync_statuses();
            app.push_toast_success("Stopped session");
        }
        KeyCode::Char('i') => {
            if app.pending_inputs.is_empty() {
                app.push_toast_warning("No pending input requests");
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
        KeyCode::Char('e') => {
            app.toggle_compose_intercept();
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
            app.mode = AppMode::Help(crate::app::HelpState {
                from_view: Some(view),
                scroll_offset: 0,
            });
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
        KeyCode::Char('p') => {
            let view_state = match std::mem::replace(&mut app.mode, AppMode::Normal) {
                AppMode::Viewing(v) => v,
                other => {
                    app.mode = other;
                    return Ok(());
                }
            };
            app.open_prompt_library(Some(view_state));
        }
        KeyCode::Char('b') => {
            app.toggle_sidebar_in_view();
        }
        KeyCode::Char('v') => {
            app.toggle_expanded_todos_in_view();
        }
        KeyCode::Char('N') => {
            app.open_todo_quick_capture();
        }
        KeyCode::Char('m') => {
            app.open_markdown_viewer_from_view()?;
        }
        KeyCode::Char('A') => {
            app.exit_view();
            app.open_harness_setup(false);
        }
        KeyCode::Char('c') => {
            app.copy_remote_control_url()?;
        }
        KeyCode::Char('C') => {
            app.toggle_remote_control_in_view()?;
        }
        KeyCode::Char('O') => {
            app.open_remote_control_url()?;
        }
        KeyCode::Char('P') => {
            app.pr_review_return_to_pane();
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
    use mockall::Sequence;
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
    fn repeated_enter_is_ignored() {
        let k = KeyEvent::new_with_kind(
            KeyCode::Enter,
            KeyModifiers::NONE,
            crossterm::event::KeyEventKind::Repeat,
        );
        assert!(crossterm_key_to_tmux(&k).is_none());
    }

    #[test]
    fn raw_newline_chars_are_not_forwarded_as_literals() {
        assert!(crossterm_key_to_tmux(&key(KeyCode::Char('\n'))).is_none());
        assert!(crossterm_key_to_tmux(&key(KeyCode::Char('\r'))).is_none());
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
        // Opening the diff viewer is async (DiffViewerLoading); drive the
        // load to completion as the event loop does before asserting.
        app.complete_diff_viewer_loading();

        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if state.branch == "feature"
                    && state.base_ref == "main"
                    && state.files.iter().any(|file| file.path == "src.txt")
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if matches!(state.layout, crate::app::DiffViewerLayout::SideBySide)
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert!(matches!(app.mode, AppMode::Viewing(_)));

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('d')), 20).unwrap();
        // Opening the diff viewer is async (DiffViewerLoading); drive the
        // load to completion as the event loop does before asserting.
        app.complete_diff_viewer_loading();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if matches!(state.layout, crate::app::DiffViewerLayout::SideBySide)
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn new_file_forces_unified_without_losing_side_by_side_preference() {
        let repo = init_repo_with_branch_change();
        let mut app = app_for_viewing_repo(repo.path());

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('d')), 20).unwrap();
        // Opening the diff viewer is async (DiffViewerLoading); drive the
        // load to completion as the event loop does before asserting.
        app.complete_diff_viewer_loading();
        crate::handlers::handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();

        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if matches!(state.layout, crate::app::DiffViewerLayout::SideBySide)
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();

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

        crate::handlers::handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state)
                if matches!(state.layout, crate::app::DiffViewerLayout::SideBySide)
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, key(KeyCode::Char('k'))).unwrap();
        assert!(!app.diff_viewer_selected_file_is_new());
        assert!(matches!(
            app.diff_viewer_layout(),
            Some(crate::app::DiffViewerLayout::SideBySide)
        ));

        crate::handlers::handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('d')), 20).unwrap();
        // Opening the diff viewer is async (DiffViewerLoading); drive the
        // load to completion as the event loop does before asserting.
        app.complete_diff_viewer_loading();
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
    fn leader_toggle_remote_control_blocked_by_zai_does_not_send() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());
        // z.ai sessions can't use Remote Control; the toggle must short
        // circuit before sending anything to tmux. The MockTmuxOps has no
        // send expectations, so any tmux send here would panic the test.
        app.config.zai = Some(crate::app::ZaiPlanConfig {
            plan: "coding".to_string(),
            ..Default::default()
        });

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('C')), 20).unwrap();

        assert!(matches!(&app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn typing_printable_char_opens_compose_seeded() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());

        handle_view_key(&mut app, key(KeyCode::Char('h')), 20).unwrap();

        match &app.mode {
            AppMode::Compose(state) => {
                assert_eq!(state.editor.text(), "h");
                assert_eq!(state.view.session, "amf-feature");
                assert_eq!(state.workdir, repo.path());
            }
            _ => panic!("expected Compose mode"),
        }
    }

    #[test]
    fn typing_printable_char_opens_compose_for_every_agent_harness() {
        for kind in [
            SessionKind::Claude,
            SessionKind::Opencode,
            SessionKind::Codex,
            SessionKind::Pi,
        ] {
            let repo = TempDir::new().unwrap();
            let mut app = app_for_viewing_repo(repo.path());
            if let AppMode::Viewing(view) = &mut app.mode {
                view.session_kind = kind.clone();
                view.window = kind_label(&kind).to_string();
            }

            handle_view_key(&mut app, key(KeyCode::Char('h')), 20).unwrap();

            assert!(
                matches!(&app.mode, AppMode::Compose(state) if state.editor.text() == "h" && state.view.session_kind == kind),
                "composer did not open for {kind:?}"
            );
        }
    }

    fn kind_label(kind: &SessionKind) -> &'static str {
        match kind {
            SessionKind::Claude => "claude",
            SessionKind::Opencode => "opencode",
            SessionKind::Codex => "codex",
            SessionKind::Pi => "pi",
            _ => "other",
        }
    }

    #[test]
    fn compose_draft_survives_close_and_reopen() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());

        handle_view_key(&mut app, key(KeyCode::Char('h')), 20).unwrap();
        crate::handlers::handle_compose_key(&mut app, key(KeyCode::Char('i'))).unwrap();
        crate::handlers::handle_compose_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert!(matches!(&app.mode, AppMode::Viewing(_)));

        handle_view_key(&mut app, key(KeyCode::Char('!')), 20).unwrap();
        match &app.mode {
            AppMode::Compose(state) => assert_eq!(state.editor.text(), "hi!"),
            _ => panic!("expected Compose mode"),
        }
    }

    #[test]
    fn ctrl_e_in_compose_switches_to_direct_input_and_keeps_draft() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());

        handle_view_key(&mut app, key(KeyCode::Char('h')), 20).unwrap();
        crate::handlers::handle_compose_key(&mut app, ctrl(KeyCode::Char('e'))).unwrap();

        assert!(matches!(&app.mode, AppMode::Viewing(_)));
        assert!(app.compose_direct_targets.contains("amf-feature:claude"));

        // leader+e re-enables compose; the draft comes back.
        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('e')), 20).unwrap();
        assert!(!app.compose_direct_targets.contains("amf-feature:claude"));

        handle_view_key(&mut app, key(KeyCode::Char('i')), 20).unwrap();
        match &app.mode {
            AppMode::Compose(state) => assert_eq!(state.editor.text(), "hi"),
            _ => panic!("expected Compose mode"),
        }
    }

    #[test]
    fn ctrl_space_in_compose_opens_leader_menu_and_keeps_draft() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());

        handle_view_key(&mut app, key(KeyCode::Char('h')), 20).unwrap();
        crate::handlers::handle_compose_key(&mut app, key(KeyCode::Char('i'))).unwrap();
        crate::handlers::handle_compose_key(&mut app, ctrl(KeyCode::Char(' '))).unwrap();

        assert!(matches!(&app.mode, AppMode::Viewing(_)));
        assert!(app.leader_active);

        // Leader command runs against the view, then typing restores
        // the draft.
        handle_view_key(&mut app, key(KeyCode::Char('e')), 20).unwrap();
        assert!(app.compose_direct_targets.contains("amf-feature:claude"));
        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('e')), 20).unwrap();

        handle_view_key(&mut app, key(KeyCode::Char('!')), 20).unwrap();
        match &app.mode {
            AppMode::Compose(state) => assert_eq!(state.editor.text(), "hi!"),
            _ => panic!("expected Compose mode"),
        }
    }

    #[test]
    fn enter_passes_through_to_tmux_with_intercept_on() {
        let repo = TempDir::new().unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_send_key_name()
            .withf(|session, window, name| {
                session == "amf-feature" && window == "claude" && name == "Enter"
            })
            .times(1)
            .returning(|_, _, _| Ok(()));

        let mut app = app_for_viewing_repo(repo.path());
        app.tmux = Box::new(tmux);

        handle_view_key(&mut app, key(KeyCode::Enter), 20).unwrap();
        assert!(matches!(&app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn terminal_sessions_are_not_intercepted() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());
        if let AppMode::Viewing(view) = &mut app.mode {
            view.session_kind = SessionKind::Terminal;
        }

        handle_view_key(&mut app, key(KeyCode::Char('h')), 20).unwrap();

        assert!(matches!(&app.mode, AppMode::Viewing(_)));
        assert_eq!(app.pending_view_input_len(), 1);
    }

    #[test]
    fn leader_e_toggles_direct_input_for_claude_view() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());

        // Re-activating the leader flushes the buffered "h" below.
        let mut tmux = MockTmuxOps::new();
        tmux.expect_send_literal()
            .withf(|session, window, text| {
                session == "amf-feature" && window == "claude" && text == "h"
            })
            .times(1)
            .returning(|_, _, _| Ok(()));
        app.tmux = Box::new(tmux);

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('e')), 20).unwrap();
        assert!(app.compose_direct_targets.contains("amf-feature:claude"));

        // With direct input on, printable keys forward (buffer) again.
        handle_view_key(&mut app, key(KeyCode::Char('h')), 20).unwrap();
        assert!(matches!(&app.mode, AppMode::Viewing(_)));
        assert_eq!(app.pending_view_input_len(), 1);

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('e')), 20).unwrap();
        assert!(!app.compose_direct_targets.contains("amf-feature:claude"));
    }

    #[test]
    fn leader_e_toggles_direct_input_for_every_agent_harness() {
        for kind in [
            SessionKind::Claude,
            SessionKind::Opencode,
            SessionKind::Codex,
            SessionKind::Pi,
        ] {
            let repo = TempDir::new().unwrap();
            let mut app = app_for_viewing_repo(repo.path());
            let window = kind_label(&kind).to_string();
            if let AppMode::Viewing(view) = &mut app.mode {
                view.session_kind = kind.clone();
                view.window = window.clone();
            }

            app.activate_leader();
            handle_view_key(&mut app, key(KeyCode::Char('e')), 20).unwrap();
            assert!(
                app.compose_direct_targets
                    .contains(&format!("amf-feature:{window}")),
                "direct input did not enable for {kind:?}"
            );

            app.activate_leader();
            handle_view_key(&mut app, key(KeyCode::Char('e')), 20).unwrap();
            assert!(
                !app.compose_direct_targets
                    .contains(&format!("amf-feature:{window}")),
                "composer did not re-enable for {kind:?}"
            );
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
                state
                    .commands
                    .get(state.selected)
                    .map(|entry| &entry.action),
                Some(CommandAction::Local { .. })
            )),
            _ => panic!("expected command picker"),
        }
    }

    #[test]
    fn leader_v_opens_pending_diff_review_from_view() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());

        let notify_dir = repo.path().join(".claude").join("notifications");
        std::fs::create_dir_all(&notify_dir).unwrap();
        let notification = serde_json::json!({
            "session_id": "amf-feature",
            "cwd": repo.path().display().to_string(),
            "message": "Review: src/lib.rs",
            "type": "diff-review",
            "file_path": repo.path().join("src/lib.rs").display().to_string(),
            "relative_path": "src/lib.rs",
            "tool": "write",
            "change_id": "chg-view",
            "old_snippet": "",
            "new_snippet": "new body",
            "response_file": repo.path().join("response.json").display().to_string(),
            "proceed_signal": repo.path().join("proceed").display().to_string()
        });
        std::fs::write(
            notify_dir.join("diff-review.json"),
            serde_json::to_string(&notification).unwrap(),
        )
        .unwrap();

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('V')), 20).unwrap();

        match &app.mode {
            AppMode::DiffReviewPrompt(state) => {
                assert_eq!(state.relative_path, "src/lib.rs");
            }
            _ => panic!("expected diff review prompt"),
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

    #[test]
    fn literal_keys_are_buffered_until_flush() {
        let repo = TempDir::new().unwrap();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_send_literal()
            .withf(|session, window, text| {
                session == "amf-feature" && window == "claude" && text == "abc"
            })
            .times(1)
            .returning(|_, _, _| Ok(()));

        let mut app = app_for_viewing_repo(repo.path());
        app.tmux = Box::new(tmux);
        // Literal forwarding only happens in direct mode; compose
        // interception would otherwise capture these keys.
        app.compose_direct_targets
            .insert("amf-feature:claude".to_string());

        handle_view_key(&mut app, key(KeyCode::Char('a')), 20).unwrap();
        handle_view_key(&mut app, key(KeyCode::Char('b')), 20).unwrap();
        handle_view_key(&mut app, key(KeyCode::Char('c')), 20).unwrap();

        assert_eq!(app.pending_view_input_len(), 3);
        assert!(app.has_pending_view_input());

        app.flush_view_input_batch().unwrap();

        assert!(!app.has_pending_view_input());
    }

    #[test]
    fn named_key_flushes_buffered_literals_before_forwarding() {
        let repo = TempDir::new().unwrap();
        let mut seq = Sequence::new();
        let mut tmux = MockTmuxOps::new();
        tmux.expect_send_literal()
            .withf(|session, window, text| {
                session == "amf-feature" && window == "claude" && text == "ab"
            })
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _, _| Ok(()));
        tmux.expect_send_key_name()
            .withf(|session, window, key| {
                session == "amf-feature" && window == "claude" && key == "Enter"
            })
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_, _, _| Ok(()));

        let mut app = app_for_viewing_repo(repo.path());
        app.tmux = Box::new(tmux);
        // Literal forwarding only happens in direct mode; compose
        // interception would otherwise capture these keys.
        app.compose_direct_targets
            .insert("amf-feature:claude".to_string());

        handle_view_key(&mut app, key(KeyCode::Char('a')), 20).unwrap();
        handle_view_key(&mut app, key(KeyCode::Char('b')), 20).unwrap();
        handle_view_key(&mut app, key(KeyCode::Enter), 20).unwrap();

        assert!(!app.has_pending_view_input());
    }

    #[test]
    fn leader_shift_r_repaints_pane_with_resize_bounce() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());
        let mut tmux = MockTmuxOps::new();
        let mut seq = mockall::Sequence::new();
        // Bounce: one row shorter first, then back to the real size,
        // forcing the agent to fully repaint.
        tmux.expect_resize_pane()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|session, window, cols, rows| {
                session == "amf-feature" && window == "claude" && *cols == 88 && *rows == 23
            })
            .returning(|_, _, _, _| Ok(()));
        tmux.expect_resize_pane()
            .times(1)
            .in_sequence(&mut seq)
            .withf(|session, window, cols, rows| {
                session == "amf-feature" && window == "claude" && *cols == 88 && *rows == 24
            })
            .returning(|_, _, _, _| Ok(()));
        app.tmux = Box::new(tmux);
        app.viewport_cols = 120;
        app.viewport_rows = 24;
        app.viewport_total_rows = 25;

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('R')), 24).unwrap();

        assert!(matches!(app.mode, AppMode::Viewing(_)));
        assert_eq!(app.message.as_deref(), Some("Repainted agent pane"));
    }

    #[test]
    fn reanchor_bounce_target_only_for_claude_panes() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());
        app.viewport_cols = 120;
        app.viewport_rows = 24;
        app.viewport_total_rows = 25;

        // Claude pane: target is the live pane minus the sidebar (cols)
        // and the header (rows), matching the leader-R bounce dimensions.
        assert_eq!(
            app.reanchor_bounce_target(),
            Some(("amf-feature".to_string(), "claude".to_string(), 88, 24)),
        );

        // Other harnesses fully repaint and must be excluded so they
        // never take the bounce's flicker.
        if let AppMode::Viewing(view) = &mut app.mode {
            view.session_kind = crate::project::SessionKind::Codex;
        }
        assert_eq!(app.reanchor_bounce_target(), None);
    }

    #[test]
    fn scroll_mode_ctrl_j_scrolls_faster() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());
        if let AppMode::Viewing(view) = &mut app.mode {
            view.scroll_mode = true;
            view.scroll_passthrough = false;
            view.scroll_offset = 4;
            view.scroll_total_lines = 200;
            view.scroll_content = (0..200)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n");
        }

        handle_view_key(
            &mut app,
            KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL),
            20,
        )
        .unwrap();

        match &app.mode {
            AppMode::Viewing(view) => assert_eq!(view.scroll_offset, 4 + VIEW_FAST_SCROLL_STEP),
            _ => panic!("expected Viewing mode"),
        }
    }

    #[test]
    fn scroll_mode_ctrl_up_scrolls_back_faster() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());
        if let AppMode::Viewing(view) = &mut app.mode {
            view.scroll_mode = true;
            view.scroll_passthrough = false;
            view.scroll_offset = 12;
            view.scroll_total_lines = 200;
            view.scroll_content = (0..200)
                .map(|i| format!("line {i}"))
                .collect::<Vec<_>>()
                .join("\n");
        }

        handle_view_key(
            &mut app,
            KeyEvent::new(KeyCode::Up, KeyModifiers::CONTROL),
            20,
        )
        .unwrap();

        match &app.mode {
            AppMode::Viewing(view) => assert_eq!(view.scroll_offset, 12 - VIEW_FAST_SCROLL_STEP),
            _ => panic!("expected Viewing mode"),
        }
    }

    #[test]
    fn leader_shift_n_opens_todo_quick_capture() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('N')), 20).unwrap();

        match &app.mode {
            AppMode::TodoQuickCapture(state) => {
                assert_eq!(state.project_name, "demo");
                assert_eq!(state.input, "");
                assert_eq!(state.view.session, "amf-feature");
            }
            _ => panic!("expected TodoQuickCapture mode"),
        }
    }

    #[test]
    fn todo_quick_capture_commit_creates_todos_session_and_returns_to_view() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());
        assert!(!app.store.projects[0].has_todos_session());

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('N')), 20).unwrap();
        for c in "ship it".chars() {
            crate::handlers::handle_todo_quick_capture_key(&mut app, key(KeyCode::Char(c)))
                .unwrap();
        }
        crate::handlers::handle_todo_quick_capture_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert!(matches!(&app.mode, AppMode::Viewing(_)));
        // The project gains a TODOs session, auto-created under the current
        // feature (there was none before quick-capture).
        assert!(app.store.projects[0].has_todos_session());
    }

    #[test]
    fn todo_quick_capture_escape_cancels_without_creating_session() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('N')), 20).unwrap();
        crate::handlers::handle_todo_quick_capture_key(&mut app, key(KeyCode::Char('x'))).unwrap();
        crate::handlers::handle_todo_quick_capture_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert!(matches!(&app.mode, AppMode::Viewing(_)));
        assert!(!app.store.projects[0].has_todos_session());
    }

    #[test]
    fn todo_quick_capture_empty_title_is_a_noop() {
        let repo = TempDir::new().unwrap();
        let mut app = app_for_viewing_repo(repo.path());

        app.activate_leader();
        handle_view_key(&mut app, key(KeyCode::Char('N')), 20).unwrap();
        // Enter with only whitespace typed: nothing is added.
        crate::handlers::handle_todo_quick_capture_key(&mut app, key(KeyCode::Char(' '))).unwrap();
        crate::handlers::handle_todo_quick_capture_key(&mut app, key(KeyCode::Enter)).unwrap();

        assert!(matches!(&app.mode, AppMode::Viewing(_)));
        assert!(!app.store.projects[0].has_todos_session());
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
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
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
