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
