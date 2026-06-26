use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, DiffViewerFocus};

const PATCH_SCROLL_STEP: usize = 1;
const PATCH_PAGE_STEP: usize = 20;
const FEEDBACK_PAGE_STEP: usize = 10;

pub fn handle_diff_viewer_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let code = key.code;

    // The base-ref prompt is a single-line text input that takes precedence over
    // every other binding while it is open.
    let editing_base_ref =
        matches!(&app.mode, AppMode::DiffViewer(state) if state.editing_base_ref);
    if editing_base_ref {
        match code {
            KeyCode::Enter => app.diff_viewer_submit_base_ref(),
            KeyCode::Esc => app.diff_viewer_cancel_base_ref(),
            KeyCode::Backspace => app.diff_viewer_base_ref_backspace(),
            KeyCode::Char(c) => app.diff_viewer_base_ref_input(c),
            _ => {}
        }
        return Ok(());
    }

    let review = matches!(&app.mode, AppMode::DiffViewer(state) if state.review);
    let editing_general =
        matches!(&app.mode, AppMode::DiffViewer(state) if state.editing_general);
    let editing_feedback = matches!(
        &app.mode,
        AppMode::DiffViewer(state)
            if state.feedback_editing || state.editing_general || state.editing_line_comment
    );

    // While typing feedback (per-file rejection or general) the keys drive a
    // multi-line `TextEditor`; Enter inserts a newline, so Tab submits.
    if editing_feedback {
        return handle_feedback_editor_key(app, key, editing_general);
    }

    // Review verdict / completion keys take precedence over the read-only
    // bindings below; everything they don't handle falls through to the
    // shared navigation match.
    let notes_expanded =
        matches!(&app.mode, AppMode::DiffViewer(state) if state.notes_expanded);

    if review {
        // A pending finish confirmation (some files have no verdict) takes
        // precedence: y/q finish anyway, Esc cancels, and any other key clears
        // the prompt and is handled normally (so e.g. deciding the last file
        // then pressing q finishes cleanly).
        let finish_confirm =
            matches!(&app.mode, AppMode::DiffViewer(state) if state.finish_confirm);
        if finish_confirm {
            match code {
                KeyCode::Char('y') | KeyCode::Char('q') => {
                    app.finish_final_review()?;
                    return Ok(());
                }
                KeyCode::Esc => {
                    if let AppMode::DiffViewer(state) = &mut app.mode {
                        state.finish_confirm = false;
                    }
                    return Ok(());
                }
                _ => {
                    if let AppMode::DiffViewer(state) = &mut app.mode {
                        state.finish_confirm = false;
                    }
                }
            }
        }

        // With the notes panel expanded, navigation scrolls the note rather
        // than the diff.
        if notes_expanded {
            match code {
                KeyCode::Char('j') | KeyCode::Down => {
                    app.review_notes_scroll_down(PATCH_SCROLL_STEP);
                    return Ok(());
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.review_notes_scroll_up(PATCH_SCROLL_STEP);
                    return Ok(());
                }
                KeyCode::PageDown => {
                    app.review_notes_scroll_down(PATCH_PAGE_STEP);
                    return Ok(());
                }
                KeyCode::PageUp => {
                    app.review_notes_scroll_up(PATCH_PAGE_STEP);
                    return Ok(());
                }
                KeyCode::Home | KeyCode::Char('g') => {
                    app.review_notes_scroll_top();
                    return Ok(());
                }
                KeyCode::End | KeyCode::Char('G') => {
                    app.review_notes_scroll_bottom();
                    return Ok(());
                }
                _ => {}
            }
        }

        // With the line cursor active, navigation moves the cursor and Enter
        // opens the comment editor for the cursored line. Esc exits cursor mode
        // (q still finishes the review). Inactive, these keys keep their
        // existing meaning, falling through to the bindings below.
        let cursor_active =
            matches!(&app.mode, AppMode::DiffViewer(state) if state.comment_cursor.is_some());
        if cursor_active && !notes_expanded {
            match code {
                KeyCode::Char('j') | KeyCode::Down => {
                    app.diff_review_cursor_move(1);
                    return Ok(());
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.diff_review_cursor_move(-1);
                    return Ok(());
                }
                KeyCode::PageDown => {
                    app.diff_review_cursor_move(PATCH_PAGE_STEP as isize);
                    return Ok(());
                }
                KeyCode::PageUp => {
                    app.diff_review_cursor_move(-(PATCH_PAGE_STEP as isize));
                    return Ok(());
                }
                KeyCode::Char('g') => {
                    app.diff_review_cursor_move(isize::MIN / 2);
                    return Ok(());
                }
                KeyCode::Char('G') => {
                    app.diff_review_cursor_move(isize::MAX / 2);
                    return Ok(());
                }
                KeyCode::Char('v') => {
                    app.diff_review_toggle_range_anchor();
                    return Ok(());
                }
                KeyCode::Enter | KeyCode::Char('C') => {
                    app.diff_review_start_line_comment();
                    return Ok(());
                }
                KeyCode::Esc | KeyCode::Char('c') => {
                    app.diff_review_toggle_line_cursor();
                    return Ok(());
                }
                _ => {}
            }
        }

        match code {
            KeyCode::Char('c') => {
                app.diff_review_toggle_line_cursor();
                return Ok(());
            }
            KeyCode::Char('b') => {
                app.diff_viewer_start_base_ref_edit();
                return Ok(());
            }
            KeyCode::Char('e') => {
                app.toggle_review_notes_expanded();
                return Ok(());
            }
            KeyCode::Char('w') => {
                app.generate_review_walkthrough();
                return Ok(());
            }
            KeyCode::Char('a') => {
                app.diff_review_approve_current();
                return Ok(());
            }
            KeyCode::Char('r') => {
                app.diff_review_start_feedback();
                return Ok(());
            }
            KeyCode::Char('s') => {
                app.diff_review_skip_current();
                return Ok(());
            }
            KeyCode::Char('f') => {
                app.diff_review_start_general_feedback();
                return Ok(());
            }
            KeyCode::Char('n') => {
                app.diff_viewer_select_next_file();
                return Ok(());
            }
            KeyCode::Char('p') => {
                app.diff_viewer_select_prev_file();
                return Ok(());
            }
            KeyCode::Char('u') => {
                app.diff_review_jump_next_undecided();
                return Ok(());
            }
            KeyCode::Char('F') => {
                app.diff_review_cycle_file_filter();
                return Ok(());
            }
            KeyCode::Char('t') => {
                app.diff_review_toggle_fix_target();
                return Ok(());
            }
            KeyCode::Char('q') | KeyCode::Esc => {
                app.confirm_or_finish_review()?;
                return Ok(());
            }
            _ => {}
        }
    }

    match code {
        KeyCode::Esc | KeyCode::Char('q') => {
            app.close_diff_viewer();
        }
        KeyCode::Tab => {
            app.diff_viewer_toggle_focus();
        }
        KeyCode::Char('v') => {
            app.diff_viewer_toggle_layout();
        }
        KeyCode::Char('r') => {
            app.refresh_diff_viewer();
        }
        KeyCode::Char('b') => {
            app.diff_viewer_start_base_ref_edit();
        }
        KeyCode::Char('i') => {
            app.open_syntax_language_picker_for_selected_diff_file();
        }
        KeyCode::Char('j') | KeyCode::Down => match app.diff_viewer_focus() {
            Some(DiffViewerFocus::FileList) => app.diff_viewer_select_next_file(),
            Some(DiffViewerFocus::Patch) => app.diff_viewer_scroll_patch_down(PATCH_SCROLL_STEP),
            None => {}
        },
        KeyCode::Char('k') | KeyCode::Up => match app.diff_viewer_focus() {
            Some(DiffViewerFocus::FileList) => app.diff_viewer_select_prev_file(),
            Some(DiffViewerFocus::Patch) => app.diff_viewer_scroll_patch_up(PATCH_SCROLL_STEP),
            None => {}
        },
        KeyCode::PageDown => {
            app.diff_viewer_scroll_patch_down(PATCH_PAGE_STEP);
        }
        KeyCode::PageUp => {
            app.diff_viewer_scroll_patch_up(PATCH_PAGE_STEP);
        }
        KeyCode::Char('g') => match app.diff_viewer_focus() {
            Some(DiffViewerFocus::FileList) => {
                // Walk to the first (visible) file. Break when the selection
                // stops moving so a filter whose first file isn't index 0 can't
                // loop forever.
                while matches!(app.diff_viewer_focus(), Some(DiffViewerFocus::FileList)) {
                    let before = match &app.mode {
                        crate::app::AppMode::DiffViewer(state) => state.selected_file,
                        _ => break,
                    };
                    app.diff_viewer_select_prev_file();
                    let after = match &app.mode {
                        crate::app::AppMode::DiffViewer(state) => state.selected_file,
                        _ => break,
                    };
                    if after == before {
                        break;
                    }
                }
            }
            Some(DiffViewerFocus::Patch) => app.diff_viewer_scroll_patch_top(),
            None => {}
        },
        KeyCode::Char('G') => match app.diff_viewer_focus() {
            Some(DiffViewerFocus::FileList) => {
                while matches!(app.diff_viewer_focus(), Some(DiffViewerFocus::FileList)) {
                    let before = match &app.mode {
                        crate::app::AppMode::DiffViewer(state) => state.selected_file,
                        _ => break,
                    };
                    app.diff_viewer_select_next_file();
                    let after = match &app.mode {
                        crate::app::AppMode::DiffViewer(state) => state.selected_file,
                        _ => break,
                    };
                    if after == before {
                        break;
                    }
                }
            }
            Some(DiffViewerFocus::Patch) => app.diff_viewer_scroll_patch_bottom(),
            None => {}
        },
        _ => {}
    }

    Ok(())
}

/// Drive the multi-line feedback editor (per-file rejection or general
/// feedback). Tab submits, Esc cancels in plain mode, Ctrl+Q always cancels,
/// Ctrl+V toggles vim, and Ctrl+J/K plus PgUp/PgDn scroll the editor.
fn handle_feedback_editor_key(
    app: &mut App,
    key: KeyEvent,
    editing_general: bool,
) -> Result<()> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    if ctrl && key.code == KeyCode::Char('q') {
        app.diff_review_cancel_feedback();
        return Ok(());
    }
    if ctrl && key.code == KeyCode::Char('v') {
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.feedback_editor.toggle_vim();
        }
        return Ok(());
    }

    if let AppMode::DiffViewer(state) = &mut app.mode {
        match key.code {
            KeyCode::Char('j') if ctrl => {
                state.feedback_scroll += 1;
                state.feedback_sync_to_cursor = false;
                return Ok(());
            }
            KeyCode::Char('k') if ctrl => {
                state.feedback_scroll = state.feedback_scroll.saturating_sub(1);
                state.feedback_sync_to_cursor = false;
                return Ok(());
            }
            KeyCode::PageDown => {
                state.feedback_scroll += FEEDBACK_PAGE_STEP;
                state.feedback_sync_to_cursor = false;
                return Ok(());
            }
            KeyCode::PageUp => {
                state.feedback_scroll = state.feedback_scroll.saturating_sub(FEEDBACK_PAGE_STEP);
                state.feedback_sync_to_cursor = false;
                return Ok(());
            }
            _ => {}
        }
    }

    let editing_line_comment =
        matches!(&app.mode, AppMode::DiffViewer(state) if state.editing_line_comment);

    match key.code {
        KeyCode::Tab if editing_general => app.diff_review_submit_general_feedback(),
        KeyCode::Tab if editing_line_comment => app.diff_review_submit_line_comment(),
        KeyCode::Tab => app.diff_review_submit_feedback(),
        KeyCode::Esc
            if matches!(
                &app.mode,
                AppMode::DiffViewer(state) if state.feedback_editor.vim_mode().is_none()
            ) =>
        {
            app.diff_review_cancel_feedback();
        }
        _ => {
            if let AppMode::DiffViewer(state) = &mut app.mode {
                let outcome = state.feedback_editor.handle_key(key);
                if outcome.text_changed || outcome.cursor_moved {
                    state.feedback_sync_to_cursor = true;
                }
            }
        }
    }

    Ok(())
}

/// Key handling for the post-review harness picker: choose which harness runs a
/// fresh dedicated session for the review's fixes (j/k move, Enter confirms,
/// q/Esc cancels — the feedback file is already written either way).
pub fn handle_review_harness_pick_key(app: &mut App, key: KeyCode) -> Result<()> {
    match key {
        KeyCode::Char('j') | KeyCode::Down => app.review_harness_pick_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.review_harness_pick_move(-1),
        KeyCode::Enter => app.review_harness_pick_select()?,
        KeyCode::Esc | KeyCode::Char('q') => app.review_harness_pick_cancel(),
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppMode, DiffViewerLayout, DiffViewerState, ViewState};
    use crate::diff::{DiffFile, DiffFileStatus, DiffHunk, DiffLine, DiffLineKind};
    use crate::project::{AgentKind, Feature, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn make_review_app(workdir: &Path, paths: &[&str]) -> App {
        let mut app = crate::app::App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        let mut state = DiffViewerState::new(
            ViewState::new(
                "proj".into(),
                "feat".into(),
                "sess".into(),
                "claude".into(),
                "Claude".into(),
                crate::project::SessionKind::Claude,
                VibeMode::Vibeless,
                true,
            ),
            workdir.to_path_buf(),
        );
        state.review = true;
        state.files = paths
            .iter()
            .map(|p| DiffFile {
                old_path: Some((*p).to_string()),
                path: (*p).to_string(),
                status: DiffFileStatus::Modified,
                additions: 1,
                deletions: 1,
                is_binary: false,
                old_content: None,
                new_content: None,
                patch: String::new(),
                hunks: vec![],
            })
            .collect();
        app.mode = AppMode::DiffViewer(state);
        app
    }

    #[test]
    fn approving_all_files_finishes_without_feedback_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve a.rs -> b.rs
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve b.rs
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap(); // finish

        assert!(matches!(app.mode, AppMode::Viewing(_)));
        assert!(!dir.path().join(".claude/final-review-feedback.md").exists());
    }

    #[test]
    fn rejecting_with_feedback_writes_feedback_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

        // Reject a.rs with feedback "fix it".
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
        for c in "fix it".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // submit -> advance to b.rs
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve b.rs
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap(); // finish

        let feedback =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(feedback.contains("### a.rs"));
        assert!(feedback.contains("fix it"));
        assert!(feedback.contains("**Approved:** 1"));
        assert!(feedback.contains("**Needs work:** 1"));
        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn t_toggles_fix_target_between_live_and_dedicated() {
        use crate::app::pr_review::FixTarget;
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        let target = |app: &App| match &app.mode {
            AppMode::DiffViewer(s) => s.fix_target,
            _ => panic!("not in review"),
        };
        // Default is the existing pane; t flips to dedicated and back.
        assert_eq!(target(&app), FixTarget::ExistingLive);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('t'))).unwrap();
        assert_eq!(target(&app), FixTarget::DedicatedReview);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('t'))).unwrap();
        assert_eq!(target(&app), FixTarget::ExistingLive);
    }

    #[test]
    fn dedicated_target_with_no_session_opens_harness_pick() {
        use crate::app::pr_review::FixTarget;
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        // The dispatch resolves the feature by the view's project/feature names,
        // so the store must hold a matching feature with no agent session yet.
        let mut project = Project::new("proj".into(), dir.path().to_path_buf(), true, AgentKind::Claude);
        project.features.push(Feature::new(
            "feat".into(),
            "branch".into(),
            dir.path().to_path_buf(),
            false,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        ));
        app.store.projects.push(project);
        app.store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];

        // Choose the dedicated target, reject the file, then finish.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('t'))).unwrap();
        if let AppMode::DiffViewer(s) = &app.mode {
            assert_eq!(s.fix_target, FixTarget::DedicatedReview);
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
        for c in "fix it".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // submit rejection
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap(); // finish

        // The feedback file is written and, because the dedicated session must be
        // spun up, the harness picker is shown rather than returning to the view.
        assert!(dir.path().join(".claude/final-review-feedback.md").exists());
        match &app.mode {
            AppMode::ReviewHarnessPick(state) => {
                assert_eq!(state.harnesses, vec![AgentKind::Claude, AgentKind::Codex]);
            }
            _ => panic!("expected harness pick mode after finishing"),
        }

        // Cancelling keeps the feedback and returns to the feature view.
        handle_review_harness_pick_key(&mut app, KeyCode::Esc).unwrap();
        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn feedback_editor_accepts_multiline_input_and_tab_submits() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        // Reject a.rs with a two-line note: Enter inserts a newline (not submit),
        // Tab submits.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
        for c in "first".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for c in "second".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        // Still editing after Enter (newline, not submit).
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state) if state.feedback_editing
        ));
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.feedback_editing);
                match state.decisions.get("a.rs") {
                    Some(crate::app::ReviewDecision::Reject { feedback }) => {
                        assert_eq!(feedback, "first\nsecond");
                    }
                    other => panic!("expected rejection with feedback, got {other:?}"),
                }
            }
            _ => panic!("expected diff viewer"),
        }
    }

    #[test]
    fn general_feedback_is_written_even_when_all_approved() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        // f opens the general-feedback editor; type, submit, approve, finish.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('f'))).unwrap();
        for c in "tighten error handling".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // save general note
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.editing_general);
                assert_eq!(state.selected_file, 0, "general feedback must not advance");
                assert_eq!(state.general_feedback, "tighten error handling");
            }
            _ => panic!("expected diff viewer"),
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve a.rs
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap(); // finish

        let feedback =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(feedback.contains("## General Feedback"));
        assert!(feedback.contains("tighten error handling"));
        assert!(!feedback.contains("## Files Needing Revision"));
    }

    #[test]
    fn finishing_with_feedback_prompts_the_agent() {
        let dir = tempfile::TempDir::new().unwrap();

        let mut feature = Feature::new(
            "feature".to_string(),
            "feature".to_string(),
            dir.path().to_path_buf(),
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
        let agent_session = feature.tmux_session.clone();
        let agent_window = session.tmux_window.clone();

        let mut project = Project::new(
            "demo".to_string(),
            dir.path().to_path_buf(),
            true,
            AgentKind::Claude,
        );
        project.features.push(feature);

        // The agent must be pasted the feedback prompt and have Enter sent.
        let mut tmux = MockTmuxOps::new();
        let (ps, pw) = (agent_session.clone(), agent_window.clone());
        tmux.expect_paste_text()
            .withf(move |session, window, text| {
                session == ps && window == pw && text.contains("final-review-feedback.md")
            })
            .times(1)
            .returning(|_, _, _| Ok(()));
        let (ks, kw) = (agent_session.clone(), agent_window.clone());
        tmux.expect_send_key_name()
            .withf(move |session, window, name| {
                session == ks && window == kw && name == "Enter"
            })
            .times(1)
            .returning(|_, _, _| Ok(()));

        let store = ProjectStore {
            version: 5,
            projects: vec![project],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        };
        let mut app =
            App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));

        let mut state = DiffViewerState::new(
            ViewState::new(
                "demo".into(),
                "feature".into(),
                agent_session.clone(),
                agent_window.clone(),
                "Claude".into(),
                SessionKind::Claude,
                VibeMode::Vibeless,
                true,
            ),
            dir.path().to_path_buf(),
        );
        state.review = true;
        state.files = vec![DiffFile {
            old_path: Some("a.rs".into()),
            path: "a.rs".into(),
            status: DiffFileStatus::Modified,
            additions: 1,
            deletions: 1,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        }];
        app.mode = AppMode::DiffViewer(state);

        // Reject the file with feedback, then finish.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
        for c in "fix".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();

        assert!(dir.path().join(".claude/final-review-feedback.md").exists());
        assert!(matches!(app.mode, AppMode::Viewing(_)));
        // Mock .times(1) expectations are verified when `app` (and its tmux) drop.
    }

    #[test]
    fn paste_without_submit_does_not_send_enter() {
        let dir = tempfile::TempDir::new().unwrap();

        let mut feature = Feature::new(
            "feature".to_string(),
            "feature".to_string(),
            dir.path().to_path_buf(),
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
        let agent_session = feature.tmux_session.clone();
        let agent_window = session.tmux_window.clone();

        let mut project = Project::new(
            "demo".to_string(),
            dir.path().to_path_buf(),
            true,
            AgentKind::Claude,
        );
        project.features.push(feature);

        // The prompt is pasted but Enter must NOT be sent: no send_key_name
        // expectation is registered, so mockall fails if it is called.
        let mut tmux = MockTmuxOps::new();
        let (ps, pw) = (agent_session.clone(), agent_window.clone());
        tmux.expect_paste_text()
            .withf(move |session, window, text| {
                session == ps && window == pw && text.contains("final-review-feedback.md")
            })
            .times(1)
            .returning(|_, _, _| Ok(()));

        let store = ProjectStore {
            version: 5,
            projects: vec![project],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        };
        let mut app =
            App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
        app.config.final_review_submit_prompt = false;

        let mut state = DiffViewerState::new(
            ViewState::new(
                "demo".into(),
                "feature".into(),
                agent_session.clone(),
                agent_window.clone(),
                "Claude".into(),
                SessionKind::Claude,
                VibeMode::Vibeless,
                true,
            ),
            dir.path().to_path_buf(),
        );
        state.review = true;
        state.files = vec![DiffFile {
            old_path: Some("a.rs".into()),
            path: "a.rs".into(),
            status: DiffFileStatus::Modified,
            additions: 1,
            deletions: 1,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        }];
        app.mode = AppMode::DiffViewer(state);

        // Reject the file with feedback, then finish.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
        for c in "fix".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();

        assert!(dir.path().join(".claude/final-review-feedback.md").exists());
        assert!(matches!(app.mode, AppMode::Viewing(_)));
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.contains("not submitted"))
        );
    }

    #[test]
    fn reject_then_escape_cancels_feedback_without_recording() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('x'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap(); // cancel feedback editor

        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.feedback_editing);
                assert!(state.decisions.is_empty());
            }
            _ => panic!("expected diff viewer"),
        }
    }

    /// Give the review app's single file a real hunk (one context + one added
    /// line) so it has addressable lines to put a cursor on.
    fn set_single_hunk(app: &mut App) {
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.files[0].hunks = vec![DiffHunk {
                header: "@@ -1,1 +1,2 @@".into(),
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 2,
                lines: vec![
                    DiffLine {
                        kind: DiffLineKind::Context,
                        text: " ctx".into(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: "+added line".into(),
                    },
                ],
            }];
        }
    }

    #[test]
    fn line_cursor_activates_on_first_change_and_navigates() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        // `c` activates the cursor on the first changed line (the added line,
        // index 1; index 0 is context).
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        let cursor = |app: &App| match &app.mode {
            AppMode::DiffViewer(s) => s.comment_cursor,
            _ => panic!("expected diff viewer"),
        };
        assert_eq!(cursor(&app), Some(1));

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('k'))).unwrap();
        assert_eq!(cursor(&app), Some(0));
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('k'))).unwrap();
        assert_eq!(cursor(&app), Some(0), "clamps at the top");
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
        assert_eq!(cursor(&app), Some(1), "clamps at the bottom");

        // `c` again deactivates the cursor.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        assert_eq!(cursor(&app), None);
    }

    #[test]
    fn line_comment_is_written_to_feedback_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        // Activate cursor (on the added line), open the comment editor, type,
        // and submit with Tab.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(s) if s.editing_line_comment
        ));
        for c in "bug here".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.editing_line_comment);
                let comments = state.line_comments.get("a.rs").expect("comment stored");
                assert_eq!(comments.len(), 1);
                assert_eq!(comments[0].text, "bug here");
                assert_eq!(comments[0].location.new_line, Some(2));
            }
            _ => panic!("expected diff viewer"),
        }

        // Finishing with only a line comment still writes the feedback file.
        // The file has no verdict, so the first q asks to confirm; the second
        // finishes.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        let feedback =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(feedback.contains("## Line Comments"));
        assert!(feedback.contains("### a.rs:2"));
        assert!(feedback.contains("bug here"));
        assert!(feedback.contains("**Line comments:** 1"));
        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }

    /// Give the review app's single file a hunk with two added lines (plus a
    /// leading context line) so a multi-line selection has room to span.
    fn set_two_added_hunk(app: &mut App) {
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.files[0].hunks = vec![DiffHunk {
                header: "@@ -1,1 +1,3 @@".into(),
                old_start: 1,
                old_lines: 1,
                new_start: 1,
                new_lines: 3,
                lines: vec![
                    DiffLine {
                        kind: DiffLineKind::Context,
                        text: " ctx".into(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: "+first".into(),
                    },
                    DiffLine {
                        kind: DiffLineKind::Added,
                        text: "+second".into(),
                    },
                ],
            }];
        }
    }

    #[test]
    fn multiline_comment_spans_selected_range() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_two_added_hunk(&mut app);

        // Activate the cursor (lands on the first added line, index 1), anchor a
        // selection with `v`, extend down one line, then comment the span.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for ch in "whole block".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        match &app.mode {
            AppMode::DiffViewer(state) => {
                // The selection anchor is cleared once the comment is stored.
                assert!(state.comment_anchor.is_none());
                let comments = state.line_comments.get("a.rs").expect("comment stored");
                assert_eq!(comments.len(), 1);
                assert_eq!(comments[0].text, "whole block");
                // Span covers new lines 2..3 (the two added lines).
                assert_eq!(comments[0].location.new_line, Some(3));
                assert_eq!(
                    comments[0].start.and_then(|s| s.new_line),
                    Some(2),
                    "start anchors the first selected line"
                );
                assert!(comments[0].is_range());
            }
            _ => panic!("expected diff viewer"),
        }

        // The feedback file records the range anchor `a.rs:2-3`.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        let feedback =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(feedback.contains("### a.rs:2-3"));
        assert!(feedback.contains("whole block"));
    }

    #[test]
    fn editing_a_commented_line_preserves_its_span() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_two_added_hunk(&mut app);

        // Create a 2-line comment over the added lines.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for ch in "v1".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        // Move the cursor to the *end* of the span and re-open: editing must snap
        // onto the existing span and replace (not duplicate) the comment.
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for ch in " edited".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        match &app.mode {
            AppMode::DiffViewer(state) => {
                let comments = state.line_comments.get("a.rs").expect("comment stored");
                assert_eq!(comments.len(), 1, "edit replaces rather than duplicates");
                assert_eq!(comments[0].text, "v1 edited");
                assert!(comments[0].is_range());
                assert_eq!(comments[0].start.and_then(|s| s.new_line), Some(2));
                assert_eq!(comments[0].location.new_line, Some(3));
            }
            _ => panic!("expected diff viewer"),
        }
    }

    #[test]
    fn walkthrough_is_noop_when_developer_note_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state
                .review_notes
                .insert("a.rs".into(), "hand-written".into());
        }

        // `w` must not spawn a walkthrough when a developer note is present.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('w'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(state.walkthrough_child.is_none());
                assert!(state.generated_notes.is_empty());
            }
            _ => panic!("expected diff viewer"),
        }
    }

    #[test]
    fn walkthrough_for_binary_file_sets_message_without_spawning() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["img.png"]);
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.files[0].is_binary = true;
        }

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('w'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(state.walkthrough_child.is_none());
                assert!(
                    state
                        .generated_notes
                        .get("img.png")
                        .is_some_and(|n| n.contains("Binary"))
                );
            }
            _ => panic!("expected diff viewer"),
        }
    }

    #[test]
    fn poll_walkthrough_without_child_is_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        app.poll_review_walkthrough().unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => assert!(state.generated_notes.is_empty()),
            _ => panic!("expected diff viewer"),
        }
    }

    #[test]
    fn finishing_with_undecided_files_requires_confirmation() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

        // Approve only a.rs; b.rs has no verdict.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        // q shows the confirmation rather than finishing.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.finish_confirm));
        // A second q finishes.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn escape_cancels_finish_confirmation() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap(); // both undecided
        assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.finish_confirm));
        handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap(); // cancel
        match &app.mode {
            AppMode::DiffViewer(s) => assert!(!s.finish_confirm),
            _ => panic!("should still be reviewing"),
        }
    }

    #[test]
    fn deciding_during_confirmation_clears_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap(); // undecided -> confirm
        assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.finish_confirm));
        // Approving clears the confirmation and records the verdict.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(s) => {
                assert!(!s.finish_confirm);
                assert!(s.decisions.contains_key("a.rs"));
            }
            _ => panic!("expected diff viewer"),
        }
        // q now finishes immediately (all files decided).
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn jump_to_next_undecided_skips_decided_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs", "c.rs"]);

        // Decide a.rs and b.rs, then return to a.rs.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve a -> b
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve b -> c
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('p'))).unwrap(); // c -> b
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('p'))).unwrap(); // b -> a

        // u jumps past the decided a.rs/b.rs to the only undecided file, c.rs.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('u'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(s) => assert_eq!(s.selected_file, 2),
            _ => panic!("expected diff viewer"),
        }
    }

    #[test]
    fn file_filter_cycles_and_navigation_skips_hidden_files() {
        use crate::app::FileFilter;
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs", "c.rs"]);

        let filter = |app: &App| match &app.mode {
            AppMode::DiffViewer(s) => s.file_filter,
            _ => panic!("expected diff viewer"),
        };
        let selected = |app: &App| match &app.mode {
            AppMode::DiffViewer(s) => s.selected_file,
            _ => panic!("expected diff viewer"),
        };

        // Approve a.rs (-> b.rs) and reject c.rs, leaving b.rs undecided.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve a -> b
        // Reject c.rs: move to c, then reject with feedback.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap(); // b -> c
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
        for ch in "nope".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // submit, c is last so stays

        // F cycles All -> Undecided. Only b.rs (index 1) is undecided, so the
        // selection snaps to it.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
        assert_eq!(filter(&app), FileFilter::Undecided);
        assert_eq!(selected(&app), 1, "snaps onto the only undecided file");

        // Navigation stays put: b.rs is the only visible file.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
        assert_eq!(selected(&app), 1);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('p'))).unwrap();
        assert_eq!(selected(&app), 1);

        // F again -> Rejected: only c.rs (index 2) is visible.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
        assert_eq!(filter(&app), FileFilter::Rejected);
        assert_eq!(selected(&app), 2, "snaps onto the only rejected file");

        // F again wraps back to All.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
        assert_eq!(filter(&app), FileFilter::All);
    }

    #[test]
    fn base_ref_prompt_opens_types_and_submits_into_override() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        // `b` opens the prompt; verdict keys are suppressed while it is open.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('b'))).unwrap();
        assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.editing_base_ref));

        for c in "develop".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Backspace)).unwrap(); // -> "develo"

        // Enter submits: the override is recorded and the viewer re-enters
        // loading so the diff reloads against it.
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        match &app.mode {
            AppMode::DiffViewerLoading(s) => {
                assert_eq!(s.override_base_ref.as_deref(), Some("develo"));
                assert!(!s.editing_base_ref);
            }
            _ => panic!("expected loading state after submit"),
        }
    }

    #[test]
    fn base_ref_prompt_escape_cancels_without_override() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('b'))).unwrap();
        for c in "main".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();

        match &app.mode {
            AppMode::DiffViewer(s) => {
                assert!(!s.editing_base_ref);
                assert!(s.override_base_ref.is_none());
                assert!(s.base_ref_input.is_empty());
            }
            _ => panic!("expected diff viewer"),
        }
    }

    #[test]
    fn base_ref_prompt_blank_submit_clears_override() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.override_base_ref = Some("develop".into());
        }

        // Open (pre-fills with the current override), clear it, submit blank.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('b'))).unwrap();
        for _ in 0.."develop".len() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Backspace)).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

        match &app.mode {
            AppMode::DiffViewerLoading(s) => assert!(s.override_base_ref.is_none()),
            _ => panic!("expected loading state after submit"),
        }
    }

    #[test]
    fn i_opens_syntax_picker_for_selected_diff_file() {
        let mut app = crate::app::App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
                available_harnesses: vec![],
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            },
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        let mut state = DiffViewerState::new(
            ViewState::new(
                "proj".into(),
                "feat".into(),
                "sess".into(),
                "claude".into(),
                "Claude".into(),
                crate::project::SessionKind::Claude,
                VibeMode::Vibe,
                false,
            ),
            PathBuf::from("/tmp/project"),
        );
        state.layout = DiffViewerLayout::Unified;
        state.files = vec![DiffFile {
            old_path: None,
            path: "src/main.rs".into(),
            status: DiffFileStatus::Modified,
            additions: 1,
            deletions: 1,
            is_binary: false,
            old_content: None,
            new_content: None,
            patch: String::new(),
            hunks: vec![],
        }];
        app.mode = AppMode::DiffViewer(state);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('i'))).unwrap();

        match &app.mode {
            AppMode::SyntaxLanguagePicker(state) => {
                assert_eq!(
                    state.languages[state.selected].language,
                    crate::highlight::HighlightLanguage::Rust
                );
            }
            other => panic!(
                "expected syntax picker, got {:?}",
                std::mem::discriminant(other)
            ),
        }
    }
}
