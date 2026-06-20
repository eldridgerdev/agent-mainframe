use anyhow::Result;
use crossterm::event::KeyCode;

use crate::app::{App, AppMode, DiffViewerFocus};

const PATCH_SCROLL_STEP: usize = 1;
const PATCH_PAGE_STEP: usize = 20;

pub fn handle_diff_viewer_key(app: &mut App, key: KeyCode) -> Result<()> {
    let review = matches!(&app.mode, AppMode::DiffViewer(state) if state.review);
    let editing_general =
        matches!(&app.mode, AppMode::DiffViewer(state) if state.editing_general);
    let editing_feedback = matches!(
        &app.mode,
        AppMode::DiffViewer(state) if state.feedback_editing || state.editing_general
    );

    // While typing feedback (per-file rejection or general), every key is text
    // input. Enter routes to whichever editor is open.
    if editing_feedback {
        match key {
            KeyCode::Esc => app.diff_review_cancel_feedback(),
            KeyCode::Enter if editing_general => app.diff_review_submit_general_feedback(),
            KeyCode::Enter => app.diff_review_submit_feedback(),
            KeyCode::Backspace => app.diff_review_pop_feedback_char(),
            KeyCode::Char(c) => app.diff_review_push_feedback_char(c),
            _ => {}
        }
        return Ok(());
    }

    // Review verdict / completion keys take precedence over the read-only
    // bindings below; everything they don't handle falls through to the
    // shared navigation match.
    let notes_expanded =
        matches!(&app.mode, AppMode::DiffViewer(state) if state.notes_expanded);

    if review {
        // With the notes panel expanded, navigation scrolls the note rather
        // than the diff.
        if notes_expanded {
            match key {
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

        match key {
            KeyCode::Char('e') => {
                app.toggle_review_notes_expanded();
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
            KeyCode::Char('q') | KeyCode::Esc => {
                app.finish_final_review()?;
                return Ok(());
            }
            _ => {}
        }
    }

    match key {
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
                while matches!(app.diff_viewer_focus(), Some(DiffViewerFocus::FileList)) {
                    let before = match &app.mode {
                        crate::app::AppMode::DiffViewer(state) => state.selected_file,
                        _ => break,
                    };
                    if before == 0 {
                        break;
                    }
                    app.diff_viewer_select_prev_file();
                }
            }
            Some(DiffViewerFocus::Patch) => app.diff_viewer_scroll_patch_top(),
            None => {}
        },
        KeyCode::Char('G') => match app.diff_viewer_focus() {
            Some(DiffViewerFocus::FileList) => {
                while matches!(app.diff_viewer_focus(), Some(DiffViewerFocus::FileList)) {
                    let (before, len) = match &app.mode {
                        crate::app::AppMode::DiffViewer(state) => {
                            (state.selected_file, state.files.len())
                        }
                        _ => break,
                    };
                    if before + 1 >= len {
                        break;
                    }
                    app.diff_viewer_select_next_file();
                }
            }
            Some(DiffViewerFocus::Patch) => app.diff_viewer_scroll_patch_bottom(),
            None => {}
        },
        _ => {}
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{AppMode, DiffViewerLayout, DiffViewerState, ViewState};
    use crate::diff::{DiffFile, DiffFileStatus};
    use crate::project::{AgentKind, Feature, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode};
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

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

        handle_diff_viewer_key(&mut app, KeyCode::Char('a')).unwrap(); // approve a.rs -> b.rs
        handle_diff_viewer_key(&mut app, KeyCode::Char('a')).unwrap(); // approve b.rs
        handle_diff_viewer_key(&mut app, KeyCode::Char('q')).unwrap(); // finish

        assert!(matches!(app.mode, AppMode::Viewing(_)));
        assert!(!dir.path().join(".claude/final-review-feedback.md").exists());
    }

    #[test]
    fn rejecting_with_feedback_writes_feedback_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

        // Reject a.rs with feedback "fix it".
        handle_diff_viewer_key(&mut app, KeyCode::Char('r')).unwrap();
        for c in "fix it".chars() {
            handle_diff_viewer_key(&mut app, KeyCode::Char(c)).unwrap();
        }
        handle_diff_viewer_key(&mut app, KeyCode::Enter).unwrap(); // submit -> advance to b.rs
        handle_diff_viewer_key(&mut app, KeyCode::Char('a')).unwrap(); // approve b.rs
        handle_diff_viewer_key(&mut app, KeyCode::Char('q')).unwrap(); // finish

        let feedback =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(feedback.contains("### a.rs"));
        assert!(feedback.contains("fix it"));
        assert!(feedback.contains("**Approved:** 1"));
        assert!(feedback.contains("**Needs work:** 1"));
        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn general_feedback_is_written_even_when_all_approved() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        // f opens the general-feedback editor; type, submit, approve, finish.
        handle_diff_viewer_key(&mut app, KeyCode::Char('f')).unwrap();
        for c in "tighten error handling".chars() {
            handle_diff_viewer_key(&mut app, KeyCode::Char(c)).unwrap();
        }
        handle_diff_viewer_key(&mut app, KeyCode::Enter).unwrap(); // save general note
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.editing_general);
                assert_eq!(state.selected_file, 0, "general feedback must not advance");
                assert_eq!(state.general_feedback, "tighten error handling");
            }
            _ => panic!("expected diff viewer"),
        }
        handle_diff_viewer_key(&mut app, KeyCode::Char('a')).unwrap(); // approve a.rs
        handle_diff_viewer_key(&mut app, KeyCode::Char('q')).unwrap(); // finish

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
        handle_diff_viewer_key(&mut app, KeyCode::Char('r')).unwrap();
        for c in "fix".chars() {
            handle_diff_viewer_key(&mut app, KeyCode::Char(c)).unwrap();
        }
        handle_diff_viewer_key(&mut app, KeyCode::Enter).unwrap();
        handle_diff_viewer_key(&mut app, KeyCode::Char('q')).unwrap();

        assert!(dir.path().join(".claude/final-review-feedback.md").exists());
        assert!(matches!(app.mode, AppMode::Viewing(_)));
        // Mock .times(1) expectations are verified when `app` (and its tmux) drop.
    }

    #[test]
    fn reject_then_escape_cancels_feedback_without_recording() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        handle_diff_viewer_key(&mut app, KeyCode::Char('r')).unwrap();
        handle_diff_viewer_key(&mut app, KeyCode::Char('x')).unwrap();
        handle_diff_viewer_key(&mut app, KeyCode::Esc).unwrap(); // cancel feedback editor

        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.feedback_editing);
                assert!(state.decisions.is_empty());
            }
            _ => panic!("expected diff viewer"),
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

        handle_diff_viewer_key(&mut app, KeyCode::Char('i')).unwrap();

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
