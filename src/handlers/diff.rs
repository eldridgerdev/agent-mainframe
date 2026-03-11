use anyhow::Result;
use crossterm::event::KeyCode;

use crate::app::{App, DiffViewerFocus};

const PATCH_SCROLL_STEP: usize = 1;
const PATCH_PAGE_STEP: usize = 20;

pub fn handle_diff_viewer_key(app: &mut App, key: KeyCode) -> Result<()> {
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
    use crate::project::VibeMode;
    use crate::project::ProjectStore;
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn i_opens_syntax_picker_for_selected_diff_file() {
        let mut app = crate::app::App::new_for_test(
            ProjectStore {
                version: 5,
                projects: vec![],
                session_bookmarks: vec![],
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
            other => panic!("expected syntax picker, got {:?}", std::mem::discriminant(other)),
        }
    }
}
