use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, DiffViewerFocus};

const PATCH_SCROLL_STEP: usize = 1;
const PATCH_PAGE_STEP: usize = 20;
const FEEDBACK_PAGE_STEP: usize = 10;

pub fn handle_diff_picker_key(app: &mut App, key: KeyEvent) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char('q') => app.close_diff_picker(),
        KeyCode::Down | KeyCode::Char('j') => app.diff_picker_select_next(),
        KeyCode::Up | KeyCode::Char('k') => app.diff_picker_select_prev(),
        KeyCode::Enter => app.diff_picker_choose(),
        _ => {}
    }
    Ok(())
}

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

    // The diff-search prompt is likewise a single-line input that takes
    // precedence while open. Incremental: every keystroke re-runs the search and
    // jumps the line cursor.
    let editing_search = matches!(&app.mode, AppMode::DiffViewer(state) if state.editing_search);
    if editing_search {
        match code {
            KeyCode::Enter => app.diff_search_submit(),
            KeyCode::Esc => app.diff_search_cancel(),
            KeyCode::Backspace => app.diff_search_backspace(),
            KeyCode::Char(c) => app.diff_search_input(c),
            _ => {}
        }
        return Ok(());
    }

    // The review timeline/history browser is read-only and captures every key
    // while open. Horizontal navigation changes rounds; vertical navigation
    // scrolls the selected round's independently rendered markdown body.
    let history_open =
        matches!(&app.mode, AppMode::DiffViewer(state) if state.review_history.is_some());
    if history_open {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => app.close_review_history(),
            KeyCode::Enter => {
                let current = matches!(
                    &app.mode,
                    AppMode::DiffViewer(state)
                        if state.review_history.as_ref().is_some_and(|h| h.selected == 0)
                );
                if current {
                    app.close_review_history();
                }
            }
            KeyCode::Char('h') | KeyCode::Left => app.review_history_move(-1),
            KeyCode::Char('l') | KeyCode::Right => app.review_history_move(1),
            KeyCode::Char('j') | KeyCode::Down => app.review_history_scroll_down(PATCH_SCROLL_STEP),
            KeyCode::Char('k') | KeyCode::Up => app.review_history_scroll_up(PATCH_SCROLL_STEP),
            KeyCode::PageDown => app.review_history_scroll_down(PATCH_PAGE_STEP),
            KeyCode::PageUp => app.review_history_scroll_up(PATCH_PAGE_STEP),
            KeyCode::Home | KeyCode::Char('g') => app.review_history_scroll_top(),
            KeyCode::End | KeyCode::Char('G') => app.review_history_scroll_bottom(),
            _ => {}
        }
        return Ok(());
    }

    // The changeset-overview modal is a read-only summary layered over the
    // diff, but it still takes full precedence while open so none of the
    // underlying review verdict/navigation keys leak through underneath it.
    let changeset_overview_open =
        matches!(&app.mode, AppMode::DiffViewer(state) if state.changeset_overview_open);
    if changeset_overview_open {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => app.close_changeset_overview(),
            KeyCode::Char('O') => app.generate_changeset_overview(),
            KeyCode::Char('j') | KeyCode::Down => {
                app.changeset_overview_scroll_down(PATCH_SCROLL_STEP)
            }
            KeyCode::Char('k') | KeyCode::Up => app.changeset_overview_scroll_up(PATCH_SCROLL_STEP),
            KeyCode::PageDown => app.changeset_overview_scroll_down(PATCH_PAGE_STEP),
            KeyCode::PageUp => app.changeset_overview_scroll_up(PATCH_PAGE_STEP),
            KeyCode::Home | KeyCode::Char('g') => app.changeset_overview_scroll_top(),
            KeyCode::End | KeyCode::Char('G') => app.changeset_overview_scroll_bottom(),
            _ => {}
        }
        return Ok(());
    }

    // The interdiff modal is a read-only "since last review" diff for the
    // current file, layered over the viewer like the changeset overview; it
    // also takes full precedence while open.
    let interdiff_open = matches!(&app.mode, AppMode::DiffViewer(state) if state.interdiff_open);
    if interdiff_open {
        match code {
            KeyCode::Esc | KeyCode::Char('q') => app.close_interdiff(),
            KeyCode::Char('j') | KeyCode::Down => app.interdiff_scroll_down(PATCH_SCROLL_STEP),
            KeyCode::Char('k') | KeyCode::Up => app.interdiff_scroll_up(PATCH_SCROLL_STEP),
            KeyCode::PageDown => app.interdiff_scroll_down(PATCH_PAGE_STEP),
            KeyCode::PageUp => app.interdiff_scroll_up(PATCH_PAGE_STEP),
            KeyCode::Home | KeyCode::Char('g') => app.interdiff_scroll_top(),
            KeyCode::End | KeyCode::Char('G') => app.interdiff_scroll_bottom(),
            _ => {}
        }
        return Ok(());
    }

    // The pre-finish summary is a read-only (except for jumping out to edit)
    // overview layered over the diff, taking full key precedence while open —
    // same shape as the changeset-overview and interdiff modals above.
    let summary_open = matches!(&app.mode, AppMode::DiffViewer(state) if state.summary_open);
    if summary_open {
        match code {
            KeyCode::Esc => app.close_review_summary(),
            KeyCode::Char('q') => {
                app.close_review_summary();
                app.finish_final_review()?;
            }
            KeyCode::Enter => app.review_summary_jump_to_selected(),
            KeyCode::Char('j') | KeyCode::Down => app.review_summary_move(1),
            KeyCode::Char('k') | KeyCode::Up => app.review_summary_move(-1),
            KeyCode::PageDown => app.review_summary_move(PATCH_PAGE_STEP as isize),
            KeyCode::PageUp => app.review_summary_move(-(PATCH_PAGE_STEP as isize)),
            KeyCode::Home | KeyCode::Char('g') => app.review_summary_move(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => app.review_summary_move(isize::MAX / 2),
            _ => {}
        }
        return Ok(());
    }

    let review = matches!(&app.mode, AppMode::DiffViewer(state) if state.review);
    let editing_general = matches!(&app.mode, AppMode::DiffViewer(state) if state.editing_general);
    let editing_feedback = matches!(
        &app.mode,
        AppMode::DiffViewer(state)
            if state.feedback_editing
                || state.editing_general
                || state.editing_line_comment
                || state.editing_file_comment
                || state.editing_suggestion
    );

    // While typing feedback (per-file rejection or general) the keys drive a
    // multi-line `TextEditor`; Enter inserts a newline, so Tab submits.
    if editing_feedback {
        return handle_feedback_editor_key(app, key, editing_general);
    }

    // Review verdict / completion keys take precedence over the read-only
    // bindings below; everything they don't handle falls through to the
    // shared navigation match.
    let notes_expanded = matches!(&app.mode, AppMode::DiffViewer(state) if state.notes_expanded);

    if review {
        // A pending finish confirmation (some files have no verdict) takes
        // precedence: y/q finish anyway (opening the pre-finish summary, same
        // as when nothing is undecided), Esc cancels, and any other key clears
        // the prompt and is handled normally (so e.g. deciding the last file
        // then pressing q finishes cleanly).
        let finish_confirm =
            matches!(&app.mode, AppMode::DiffViewer(state) if state.finish_confirm);
        if finish_confirm {
            match code {
                KeyCode::Char('y') | KeyCode::Char('q') => {
                    app.open_review_summary();
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
                KeyCode::Char(']') => {
                    app.diff_review_jump_hunk(1);
                    return Ok(());
                }
                KeyCode::Char('[') => {
                    app.diff_review_jump_hunk(-1);
                    return Ok(());
                }
                KeyCode::Char('v') => {
                    app.diff_review_toggle_range_anchor();
                    return Ok(());
                }
                KeyCode::Char('a') => {
                    // Accept an AI draft under the cursor; otherwise fall through
                    // to the file-level approve binding below.
                    if app.diff_review_accept_draft_under_cursor() {
                        return Ok(());
                    }
                }
                KeyCode::Char('d') => {
                    if app.diff_review_dismiss_draft_under_cursor() {
                        return Ok(());
                    }
                }
                KeyCode::Tab => {
                    app.diff_review_jump_next_draft();
                    return Ok(());
                }
                KeyCode::Enter | KeyCode::Char('C') => {
                    app.diff_review_start_line_comment();
                    return Ok(());
                }
                KeyCode::Char('S') => {
                    app.diff_review_start_suggestion();
                    return Ok(());
                }
                KeyCode::Char('x') => {
                    app.diff_review_apply_suggestion_under_cursor();
                    return Ok(());
                }
                KeyCode::Char('R') => {
                    app.diff_review_toggle_resolved();
                    return Ok(());
                }
                KeyCode::Esc if app.diff_search_active() => {
                    // Unwind a committed search before exiting cursor mode, so
                    // Esc peels back search → cursor → finish predictably.
                    app.diff_search_clear();
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
            KeyCode::Char('H') => {
                app.open_review_history();
                return Ok(());
            }
            KeyCode::Char('c') => {
                app.diff_review_toggle_line_cursor();
                return Ok(());
            }
            KeyCode::Char(']') => {
                app.diff_review_jump_hunk(1);
                return Ok(());
            }
            KeyCode::Char('[') => {
                app.diff_review_jump_hunk(-1);
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
            KeyCode::Char('A') => {
                app.generate_co_review();
                return Ok(());
            }
            KeyCode::Char('O') => {
                app.open_changeset_overview();
                return Ok(());
            }
            KeyCode::Char('I') => {
                app.open_interdiff();
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
            KeyCode::Char('m') => {
                app.diff_review_start_file_comment();
                return Ok(());
            }
            KeyCode::Char('M') => {
                app.diff_review_toggle_file_comment_resolved();
                return Ok(());
            }
            KeyCode::Char('/') => {
                app.diff_review_start_search();
                return Ok(());
            }
            KeyCode::Char('n') => {
                // While a committed search is active, n/N cycle matches instead
                // of files; otherwise n keeps its next-file meaning.
                if app.diff_search_active() {
                    app.diff_search_next(1);
                } else {
                    app.diff_viewer_select_next_file();
                }
                return Ok(());
            }
            KeyCode::Char('N') => {
                if app.diff_search_active() {
                    app.diff_search_next(-1);
                }
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
            KeyCode::Char('X') => {
                app.diff_review_toggle_apply_suggestions_on_finish();
                return Ok(());
            }
            KeyCode::Char('q') => {
                app.confirm_or_finish_review()?;
                return Ok(());
            }
            KeyCode::Esc => {
                app.pause_final_review();
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
        // In the file list, j/k walk *tree rows* (directories included); n/p and
        // the verdict-advance paths still move file-to-file.
        KeyCode::Char('j') | KeyCode::Down => match app.diff_viewer_focus() {
            Some(DiffViewerFocus::FileList) => app.diff_viewer_tree_move(1),
            Some(DiffViewerFocus::Patch) => app.diff_viewer_scroll_patch_down(PATCH_SCROLL_STEP),
            None => {}
        },
        KeyCode::Char('k') | KeyCode::Up => match app.diff_viewer_focus() {
            Some(DiffViewerFocus::FileList) => app.diff_viewer_tree_move(-1),
            Some(DiffViewerFocus::Patch) => app.diff_viewer_scroll_patch_up(PATCH_SCROLL_STEP),
            None => {}
        },
        // Tree folding is a file-list concern only, so it can't steal keys from
        // the patch panel.
        KeyCode::Char('z') | KeyCode::Enter
            if app.diff_viewer_focus() == Some(DiffViewerFocus::FileList) =>
        {
            app.diff_viewer_tree_toggle_collapsed();
        }
        KeyCode::Char('Z') if app.diff_viewer_focus() == Some(DiffViewerFocus::FileList) => {
            app.diff_viewer_tree_toggle_all();
        }
        KeyCode::Char('h') | KeyCode::Left
            if app.diff_viewer_focus() == Some(DiffViewerFocus::FileList) =>
        {
            app.diff_viewer_tree_collapse_or_parent();
        }
        KeyCode::Char('l') | KeyCode::Right
            if app.diff_viewer_focus() == Some(DiffViewerFocus::FileList) =>
        {
            app.diff_viewer_tree_expand();
        }
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
/// Ctrl+T toggles vim, and Ctrl+J/K plus PgUp/PgDn scroll the editor.
fn handle_feedback_editor_key(app: &mut App, key: KeyEvent, editing_general: bool) -> Result<()> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    if ctrl && key.code == KeyCode::Char('q') {
        app.diff_review_cancel_feedback();
        return Ok(());
    }
    if ctrl && key.code == KeyCode::Char('t') {
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.feedback_editor.toggle_vim();
        }
        return Ok(());
    }
    // Ctrl+E cycles the severity of the comment / rejection being composed.
    // Only the line-comment and rejection editors carry a severity — a
    // suggestion inherits its comment's, and general feedback has none.
    if ctrl && key.code == KeyCode::Char('e') {
        if let AppMode::DiffViewer(state) = &mut app.mode
            && (state.editing_line_comment || state.editing_file_comment || state.feedback_editing)
            && !state.editing_suggestion
            && !state.editing_general
        {
            state.comment_severity = state.comment_severity.next();
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
    let editing_suggestion =
        matches!(&app.mode, AppMode::DiffViewer(state) if state.editing_suggestion);
    let editing_file_comment =
        matches!(&app.mode, AppMode::DiffViewer(state) if state.editing_file_comment);

    match key.code {
        KeyCode::Tab if editing_general => app.diff_review_submit_general_feedback(),
        KeyCode::Tab if editing_line_comment => app.diff_review_submit_line_comment(),
        KeyCode::Tab if editing_file_comment => app.diff_review_submit_file_comment(),
        KeyCode::Tab if editing_suggestion => app.diff_review_submit_suggestion(),
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
    use crate::app::{AppMode, DiffViewerLayout, DiffViewerState, FileFilter, ViewState};
    use crate::diff::{DiffFile, DiffFileStatus, DiffHunk, DiffLine, DiffLineKind};
    use crate::project::{
        AgentKind, Feature, Project, ProjectStatus, ProjectStore, SessionKind, VibeMode,
    };
    use crate::traits::{MockTmuxOps, MockWorktreeOps};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Finish a fully-decided review: the first `q` opens the pre-finish
    /// summary, the second (`q` again, now handled by the summary modal)
    /// actually writes/dispatches. Tests that only care about the end state
    /// use this instead of a single `q` press.
    fn finish_review(app: &mut App) {
        handle_diff_viewer_key(app, key(KeyCode::Char('q'))).unwrap();
        assert!(
            matches!(&app.mode, AppMode::DiffViewer(s) if s.summary_open),
            "expected the pre-finish summary to open"
        );
        handle_diff_viewer_key(app, key(KeyCode::Char('q'))).unwrap();
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

    /// A review app whose single file carries real hunks parsed from `patch`, so
    /// `addressable_lines()` is non-empty (needed to exercise the line cursor and
    /// diff search).
    fn make_review_app_with_patch(workdir: &Path, patch: &str) -> App {
        let mut app = make_review_app(workdir, &["a.rs"]);
        let files = crate::diff::parse_unified_diff(patch).unwrap();
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.files = files;
        }
        app
    }

    /// Matches `SEARCH_PATCH` in the review.rs tests: addressable lines are
    /// 0 alpha, 1 beta, 2 beta_two, 3 gamma_alpha, 4 delta.
    const SEARCH_PATCH: &str = "\
diff --git a/a.rs b/a.rs
index 1111111..2222222 100644
--- a/a.rs
+++ b/a.rs
@@ -1,3 +1,4 @@
 fn alpha() {}
-fn beta() {}
+fn beta_two() {}
+fn gamma_alpha() {}
 fn delta() {}
";

    #[test]
    fn history_navigation_loads_archive_only_after_live_rounds() {
        let dir = tempfile::TempDir::new().unwrap();
        let claude_dir = dir.path().join(".claude");
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join("final-review-feedback.md"),
            "\
# Final Review Feedback

## Review — r3

newest

## Review — r2

middle
",
        )
        .unwrap();
        std::fs::write(
            claude_dir.join("final-review-feedback-archive.md"),
            "\
# Final Review Feedback Archive

## Review — r1

oldest
",
        )
        .unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('H'))).unwrap();
        let history = match &app.mode {
            AppMode::DiffViewer(state) => state.review_history.as_ref().unwrap(),
            _ => panic!("not in review"),
        };
        assert_eq!(history.rounds.len(), 2);
        assert_eq!(history.selected, 0);
        assert!(!history.archive_loaded);

        // Current -> newest live -> older live does not touch the archive.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('l'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Right)).unwrap();
        let history = match &app.mode {
            AppMode::DiffViewer(state) => state.review_history.as_ref().unwrap(),
            _ => panic!("not in review"),
        };
        assert_eq!(history.selected, 2);
        assert!(!history.archive_loaded);

        // Crossing the loaded tail lazily appends the archive newest-first.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('l'))).unwrap();
        let history = match &app.mode {
            AppMode::DiffViewer(state) => state.review_history.as_ref().unwrap(),
            _ => panic!("not in review"),
        };
        assert_eq!(history.selected, 3);
        assert!(history.archive_loaded);
        assert_eq!(history.rounds.len(), 3);
        assert_eq!(history.rounds[2].title, "Review — r1");

        // History owns the keys while open: a verdict binding must not leak to
        // the underlying review.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        assert!(matches!(&app.mode, AppMode::DiffViewer(state) if state.decisions.is_empty()));

        handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
        assert!(matches!(&app.mode, AppMode::DiffViewer(state) if state.review_history.is_none()));
    }

    fn search_state(app: &App) -> (&str, &[usize], Option<usize>, Option<usize>, bool) {
        match &app.mode {
            AppMode::DiffViewer(s) => (
                s.search_query.as_str(),
                s.search_matches.as_slice(),
                s.search_match_pos,
                s.comment_cursor,
                s.editing_search,
            ),
            _ => panic!("not in review"),
        }
    }

    #[test]
    fn slash_opens_search_and_typing_jumps_cursor() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app_with_patch(dir.path(), SEARCH_PATCH);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('/'))).unwrap();
        assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.editing_search));

        for c in "beta".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        let (query, matches, pos, cursor, editing) = search_state(&app);
        assert_eq!(query, "beta");
        assert_eq!(matches, &[1, 2]);
        assert_eq!(pos, Some(0));
        // Cursor jumped from the start (0) to the first match (line index 1).
        assert_eq!(cursor, Some(1));
        assert!(editing);

        // Enter commits: prompt closes but the query + matches persist.
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        let (query, matches, _, _, editing) = search_state(&app);
        assert_eq!(query, "beta");
        assert_eq!(matches, &[1, 2]);
        assert!(!editing);
    }

    #[test]
    fn n_and_shift_n_cycle_matches_with_wraparound() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app_with_patch(dir.path(), SEARCH_PATCH);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('/'))).unwrap();
        for c in "beta".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        // matches [1, 2], starting at pos 0 (cursor 1).
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
        let (_, _, pos, cursor, _) = search_state(&app);
        assert_eq!((pos, cursor), (Some(1), Some(2)));
        // Wrap forward.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
        let (_, _, pos, cursor, _) = search_state(&app);
        assert_eq!((pos, cursor), (Some(0), Some(1)));
        // Backward wraps to the last match.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('N'))).unwrap();
        let (_, _, pos, cursor, _) = search_state(&app);
        assert_eq!((pos, cursor), (Some(1), Some(2)));
    }

    #[test]
    fn esc_clears_a_committed_search() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app_with_patch(dir.path(), SEARCH_PATCH);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('/'))).unwrap();
        for c in "beta".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        assert!(app.diff_search_active());

        handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
        let (query, matches, pos, _, editing) = search_state(&app);
        assert_eq!(query, "");
        assert!(matches.is_empty());
        assert_eq!(pos, None);
        assert!(!editing);
        // Still in the review viewer (Esc peeled off the search, not the review).
        assert!(matches!(app.mode, AppMode::DiffViewer(_)));
    }

    #[test]
    fn n_navigates_files_when_no_search_is_active() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

        let selected = |app: &App| match &app.mode {
            AppMode::DiffViewer(s) => s.selected_file,
            _ => panic!("not in review"),
        };
        assert_eq!(selected(&app), 0);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
        assert_eq!(selected(&app), 1);
    }

    #[test]
    fn approving_all_files_records_history_without_dispatching_feedback() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve a.rs -> b.rs
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap(); // approve b.rs
        finish_review(&mut app);

        assert!(matches!(app.mode, AppMode::Viewing(_)));
        let history =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(history.contains("**Files reviewed:** 2"));
        assert!(history.contains("**Approved:** 2"));
        assert!(history.contains("**Needs work:** 0"));
        assert!(!history.contains("### Files Needing Revision"));
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
        finish_review(&mut app);

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
        let mut project = Project::new(
            "proj".into(),
            dir.path().to_path_buf(),
            true,
            AgentKind::Claude,
        );
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
        finish_review(&mut app);

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
                    Some(crate::app::ReviewDecision::Reject { feedback, .. }) => {
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
        finish_review(&mut app);

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
            .withf(move |session, window, name| session == ks && window == kw && name == "Enter")
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
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));

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
        finish_review(&mut app);

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
        let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
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
        finish_review(&mut app);

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

    fn run_git(repo: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
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

    fn make_git_suggestion_review(workdir: &Path) -> App {
        run_git(workdir, &["init", "-q", "--initial-branch=main"]);
        run_git(workdir, &["config", "user.name", "AMF Test"]);
        run_git(workdir, &["config", "user.email", "amf@example.com"]);
        std::fs::write(workdir.join("a.rs"), "ctx\n").unwrap();
        std::fs::write(workdir.join(".gitignore"), ".claude/\n").unwrap();
        run_git(workdir, &["add", "a.rs", ".gitignore"]);
        run_git(workdir, &["commit", "-q", "-m", "base"]);
        run_git(workdir, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(workdir.join("a.rs"), "ctx\nadded line\n").unwrap();

        let snapshot = crate::diff::load_snapshot(workdir, None).unwrap();
        let mut app = make_review_app(workdir, &["a.rs"]);
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.files = snapshot.files;
            state.branch = snapshot.branch;
            state.base_ref = snapshot.base_ref;
            state.base_commit = snapshot.base_commit;
        }
        app
    }

    fn add_replacement_suggestion(app: &mut App, replacement: &str) {
        handle_diff_viewer_key(app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(app, key(KeyCode::Char('S'))).unwrap();
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.feedback_editor = crate::editor::TextEditor::new(replacement.to_string());
        }
        handle_diff_viewer_key(app, key(KeyCode::Tab)).unwrap();
    }

    #[test]
    fn x_applies_the_suggestion_under_the_cursor_and_refreshes() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut app = make_git_suggestion_review(repo.path());
        add_replacement_suggestion(&mut app, "replacement");

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('x'))).unwrap();

        assert_eq!(
            std::fs::read_to_string(repo.path().join("a.rs")).unwrap(),
            "ctx\nreplacement\n"
        );
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert_eq!(state.applied_suggestions, vec!["a.rs:2"]);
                assert_eq!(state.pending_suggestion_count(), 0);
                assert!(!state.decisions.contains_key("a.rs"));
                let file = state
                    .files
                    .iter()
                    .find(|file| file.path == "a.rs")
                    .expect("refreshed source file");
                assert_eq!(file.new_content.as_deref(), Some("ctx\nreplacement\n"));
            }
            _ => panic!("expected refreshed diff viewer"),
        }
        assert!(
            app.message
                .as_deref()
                .is_some_and(|message| message.contains("Applied suggestion locally"))
        );
    }

    #[test]
    fn finish_toggle_applies_all_suggestions_before_completing() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut app = make_git_suggestion_review(repo.path());
        add_replacement_suggestion(&mut app, "replacement");

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('X'))).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state) if state.apply_suggestions_on_finish
        ));
        finish_review(&mut app);

        assert_eq!(
            std::fs::read_to_string(repo.path().join("a.rs")).unwrap(),
            "ctx\nreplacement\n"
        );
        assert!(matches!(app.mode, AppMode::Viewing(_)));
        let message = app.message.as_deref().unwrap_or_default();
        assert!(
            message.contains("1 suggestion(s) applied locally (a.rs:2)"),
            "{message}"
        );
        let history =
            std::fs::read_to_string(repo.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(history.contains("**Local suggestion application:** 1 applied (a.rs:2)"));
        assert!(
            !history.contains("### Line Comments"),
            "a successfully applied suggestion should be recorded in history, not dispatched as feedback"
        );
    }

    #[test]
    fn finish_reports_dirty_file_and_leaves_suggestion_for_the_agent() {
        let repo = tempfile::TempDir::new().unwrap();
        let mut app = make_git_suggestion_review(repo.path());
        add_replacement_suggestion(&mut app, "replacement");
        // Simulate an agent/user edit after the viewer loaded but before finish.
        std::fs::write(repo.path().join("a.rs"), "ctx\nchanged concurrently\n").unwrap();

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('X'))).unwrap();
        finish_review(&mut app);

        assert_eq!(
            std::fs::read_to_string(repo.path().join("a.rs")).unwrap(),
            "ctx\nchanged concurrently\n"
        );
        let message = app.message.as_deref().unwrap_or_default();
        assert!(
            message.contains("1 suggestion(s) not applied locally"),
            "{message}"
        );
        let feedback =
            std::fs::read_to_string(repo.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(feedback.contains("file changed since the diff was loaded"));
        assert!(feedback.contains("```suggestion\nreplacement\n```"));
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

        // The comment auto-rejected the file (it now has a verdict), so q
        // opens the summary rather than the undecided-files confirmation, and
        // a second q writes the feedback file.
        finish_review(&mut app);
        let feedback =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(feedback.contains("## Line Comments"));
        assert!(feedback.contains("### a.rs:2"));
        assert!(feedback.contains("bug here"));
        assert!(feedback.contains("**Line comments:** 1"));
        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn resolving_the_only_comment_clears_auto_reject_and_omits_it_from_feedback() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        // Write a comment: it auto-rejects the file.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for c in "bug here".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(s) if s.decisions.contains_key("a.rs")
        ));

        // Resolving the only comment clears the auto-set verdict — no open
        // thread means nothing left forcing "needs revision".
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('R'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.decisions.contains_key("a.rs"));
                let comments = state.line_comments.get("a.rs").unwrap();
                assert!(comments[0].resolved);
            }
            _ => panic!("expected diff viewer"),
        }

        // Reopening it puts the auto-reject back.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('R'))).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(s) if s.decisions.contains_key("a.rs")
        ));

        // Resolve again, then approve explicitly so the file has a verdict.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('R'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        finish_review(&mut app);

        // A resolved thread never reaches the round's actionable sections.
        // The all-approved round itself is still recorded for the history
        // browser.
        let history =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(history.contains("**Approved:** 1"));
        assert!(!history.contains("### Line Comments"));
        assert!(!history.contains("bug here"));
    }

    #[test]
    fn unresolved_filter_is_skipped_when_no_open_thread_but_shown_once_one_exists() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        // With no comments at all, cycling through the filters must skip
        // straight past `Unresolved` (mirroring the `Changed` skip when there's
        // no prior snapshot).
        for _ in 0..4 {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
            assert!(matches!(
                &app.mode,
                AppMode::DiffViewer(s) if s.file_filter != FileFilter::Unresolved
            ));
        }

        // Land back on `All`, then add an open comment.
        while !matches!(&app.mode, AppMode::DiffViewer(s) if s.file_filter == FileFilter::All) {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for c in "needs work".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        // Now the cycle should land on `Unresolved` with the file visible.
        let mut saw_unresolved = false;
        for _ in 0..5 {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
            if let AppMode::DiffViewer(state) = &app.mode
                && state.file_filter == FileFilter::Unresolved
            {
                saw_unresolved = true;
                assert_eq!(state.visible_file_indices(), vec![0]);
            }
        }
        assert!(saw_unresolved, "Unresolved filter never appeared");
    }

    #[test]
    fn carried_unresolved_comment_is_tagged_in_the_feedback_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for c in "from last round".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        // Mark it carried, mirroring how a restored thread arrives from the
        // previous round's snapshot.
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.line_comments.get_mut("a.rs").unwrap()[0].carried = true;
        }

        finish_review(&mut app);
        let feedback =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(
            feedback.contains("(unresolved from a previous round)"),
            "{feedback}"
        );
    }

    #[test]
    fn suggestion_prefills_line_and_writes_suggestion_block() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        // Activate the cursor (on the added line), open the suggestion editor with
        // `S`: it pre-fills with the line's current text (prefix stripped).
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('S'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(state.editing_suggestion);
                assert_eq!(state.feedback_editor.text(), "added line");
            }
            _ => panic!("expected diff viewer"),
        }

        // Replace the text and submit with Tab: a suggestion-only comment is
        // stored (empty prose).
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.feedback_editor = crate::editor::TextEditor::new("replaced line".to_string());
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.editing_suggestion);
                let comments = state.line_comments.get("a.rs").expect("comment stored");
                assert_eq!(comments.len(), 1);
                assert_eq!(comments[0].text, "");
                assert_eq!(comments[0].suggestion.as_deref(), Some("replaced line"));
            }
            _ => panic!("expected diff viewer"),
        }

        // Finishing writes the suggestion as a fenced block in the feedback
        // file. The suggestion auto-rejected the file, so q opens the summary
        // (rather than the undecided confirmation) and a second q finishes.
        finish_review(&mut app);
        let feedback =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(feedback.contains("### a.rs:2"));
        assert!(feedback.contains("```suggestion\nreplaced line\n```"));
    }

    #[test]
    fn emptying_a_suggestion_only_comment_deletes_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        // Create a suggestion-only comment.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('S'))).unwrap();
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.feedback_editor = crate::editor::TextEditor::new("x".to_string());
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(s) if s.line_comments.get("a.rs").is_some_and(|c| c.len() == 1)
        ));

        // Re-open the suggestion and clear it: the prose-less comment is removed.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('S'))).unwrap();
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.feedback_editor = crate::editor::TextEditor::new(String::new());
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(state.line_comments.get("a.rs").is_none_or(|c| c.is_empty()));
            }
            _ => panic!("expected diff viewer"),
        }
    }

    #[test]
    fn suggestion_preserves_existing_comment_prose() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        // Write a prose comment first.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for ch in "prose".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        // Now add a suggestion on the same line: the prose survives.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('S'))).unwrap();
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.feedback_editor = crate::editor::TextEditor::new("newcode".to_string());
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                let comments = state.line_comments.get("a.rs").expect("comment stored");
                assert_eq!(comments.len(), 1);
                assert_eq!(comments[0].text, "prose");
                assert_eq!(comments[0].suggestion.as_deref(), Some("newcode"));
            }
            _ => panic!("expected diff viewer"),
        }
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

        // The feedback file records the range anchor `a.rs:2-3`. The comment
        // auto-rejected the file, so q opens the summary without the
        // undecided-files confirm prompt, and a second q finishes.
        finish_review(&mut app);
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
    fn shift_o_opens_changeset_overview_modal_and_q_closes_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('O'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => assert!(state.changeset_overview_open),
            _ => panic!("expected diff viewer"),
        }

        // While the modal is open, q closes it rather than finishing the
        // review (the modal takes full precedence over the underlying keys).
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => assert!(!state.changeset_overview_open),
            _ => panic!("expected diff viewer, not finished"),
        }
    }

    #[test]
    fn capital_i_opens_interdiff_modal_and_q_closes_it() {
        let dir = tempfile::TempDir::new().unwrap();

        // Round 1: finish a review so a snapshot with content lands on disk.
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.files[0].new_content = Some("fn a() {}\n".into());
        }
        app.finish_final_review().unwrap();

        // Round 2: same file, different content.
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.files[0].new_content = Some("fn a() {\n    changed();\n}\n".into());
        }

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('I'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => assert!(state.interdiff_open),
            _ => panic!("expected diff viewer"),
        }

        // While the modal is open, q closes it rather than finishing the
        // review (the modal takes full precedence over the underlying keys).
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => assert!(!state.interdiff_open),
            _ => panic!("expected diff viewer, not finished"),
        }
    }

    #[test]
    fn reopening_a_cached_overview_does_not_regenerate() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.changeset_overview = Some("cached overview".to_string());
        }
        app.message = Some("sentinel".to_string());

        app.open_changeset_overview();
        // Generation is skipped when a cached overview already exists, so the
        // message (which generation would overwrite) is untouched.
        assert_eq!(app.message.as_deref(), Some("sentinel"));
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(state.changeset_overview_open);
                assert!(state.changeset_overview_child.is_none());
            }
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
        // A second q (finish-anyway) opens the pre-finish summary instead of
        // finishing outright.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.summary_open));
        assert!(matches!(&app.mode, AppMode::DiffViewer(s) if !s.finish_confirm));
        // A third q, from the summary, actually finishes.
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
        // q now opens the pre-finish summary immediately (all files decided,
        // no undecided-files confirmation needed).
        finish_review(&mut app);
        assert!(matches!(app.mode, AppMode::Viewing(_)));
    }

    #[test]
    fn summary_lists_every_verdict_and_general_feedback_in_order() {
        use crate::app::SummaryItem;
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

        // Reject a.rs with feedback, approve b.rs, then write general feedback.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
        for c in "fix it".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // advance to b.rs
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('f'))).unwrap();
        for c in "nice work overall".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(state.summary_open);
                assert_eq!(
                    state.summary_items(),
                    vec![
                        SummaryItem::File { file_idx: 0 },
                        SummaryItem::File { file_idx: 1 },
                        SummaryItem::General,
                    ]
                );
            }
            _ => panic!("expected diff viewer with summary open"),
        }
    }

    #[test]
    fn esc_closes_summary_without_finishing() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        assert!(matches!(&app.mode, AppMode::DiffViewer(s) if s.summary_open));

        handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.summary_open);
                // Nothing about the review itself changed.
                assert!(state.decisions.contains_key("a.rs"));
            }
            _ => panic!("expected the review to still be open, not finished"),
        }
        assert!(!dir.path().join(".claude/final-review-feedback.md").exists());
    }

    #[test]
    fn summary_navigation_clamps_at_both_ends() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs", "c.rs"]);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();

        let selected = |app: &App| match &app.mode {
            AppMode::DiffViewer(s) => s.summary_selected,
            _ => panic!("expected diff viewer"),
        };
        assert_eq!(selected(&app), 0);
        // k at the top stays put.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('k'))).unwrap();
        assert_eq!(selected(&app), 0);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('G'))).unwrap();
        assert_eq!(selected(&app), 2);
        // j at the bottom stays put.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
        assert_eq!(selected(&app), 2);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('g'))).unwrap();
        assert_eq!(selected(&app), 0);
    }

    #[test]
    fn summary_enter_on_line_comment_jumps_and_opens_editor_prefilled() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for c in "bug here".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        // The comment auto-rejected the file, so summary_items is
        // [File { 0 }, LineComment { 0, 0 }].
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.summary_open);
                assert!(state.editing_line_comment);
                assert_eq!(state.feedback_editor.text(), "bug here");
                assert_eq!(state.selected_file, 0);
            }
            _ => panic!("expected diff viewer back in the line-comment editor"),
        }
    }

    #[test]
    fn summary_enter_on_rejected_file_jumps_and_opens_feedback_editor_prefilled() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('r'))).unwrap();
        for c in "needs work".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap(); // -> b.rs
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        // Selection starts on the a.rs row (its rejection).
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.summary_open);
                assert!(state.feedback_editing);
                assert_eq!(state.feedback_editor.text(), "needs work");
                assert_eq!(state.selected_file, 0);
            }
            _ => panic!("expected diff viewer back in the rejection editor"),
        }
    }

    #[test]
    fn summary_enter_on_approved_file_just_navigates() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap(); // jump to a.rs's row

        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.summary_open);
                assert!(!state.feedback_editing);
                assert!(!state.editing_general);
                assert_eq!(state.selected_file, 0);
            }
            _ => panic!("expected diff viewer, no editor opened"),
        }
    }

    #[test]
    fn summary_enter_on_file_comment_jumps_and_opens_editor_prefilled() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('m'))).unwrap();
        for c in "consider splitting".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        // File comments are verdict-free, so a.rs is still undecided: q raises
        // the confirmation first.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('y'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap(); // -> FileComment row
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.summary_open);
                assert!(state.editing_file_comment);
                assert_eq!(state.feedback_editor.text(), "consider splitting");
            }
            _ => panic!("expected diff viewer back in the file-comment editor"),
        }
    }

    #[test]
    fn summary_enter_on_general_feedback_jumps_and_opens_editor_prefilled() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('f'))).unwrap();
        for c in "great work".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('q'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap(); // -> General row
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();

        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(!state.summary_open);
                assert!(state.editing_general);
                assert_eq!(state.feedback_editor.text(), "great work");
            }
            _ => panic!("expected diff viewer back in the general-feedback editor"),
        }
    }

    #[test]
    fn esc_pauses_review_without_writing_feedback() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);

        // Both files are undecided; q here would only raise the finish
        // confirmation, but top-level Esc pauses immediately without it.
        handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert!(matches!(app.mode, AppMode::Viewing(_)));
        assert!(!dir.path().join(".claude/final-review-feedback.md").exists());
    }

    #[test]
    fn esc_pauses_review_after_a_decision_and_keeps_progress_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        // Approving persists progress incrementally, independent of pause/finish.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        let progress_path = dir.path().join(".claude/final-review-progress.json");
        assert!(progress_path.exists());

        handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();

        assert!(matches!(app.mode, AppMode::Viewing(_)));
        assert!(!dir.path().join(".claude/final-review-feedback.md").exists());
        // Pausing must not clear the progress a reviewer will resume from.
        let saved = std::fs::read_to_string(&progress_path).unwrap();
        assert!(saved.contains("a.rs"));
    }

    #[test]
    // The child is only waited on in the success-path match arm below; the
    // `_ => panic!(...)` arm exists purely to fail the test and never
    // reaches it, which clippy's static analysis can't prove is fine.
    #[allow(clippy::zombie_processes)]
    fn esc_does_not_pause_while_finish_check_is_running() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();

        // Simulate `q` having already spawned a background finish-check
        // command that hasn't reported back yet.
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let child_id = child.id();
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.finish_check_command = Some("sleep 30".to_string());
            state.finish_check_child = Some(child);
        }

        // Esc must not drop the DiffViewerState here — doing so would orphan
        // the check child and the review would never actually complete.
        handle_diff_viewer_key(&mut app, key(KeyCode::Esc)).unwrap();
        match &mut app.mode {
            AppMode::DiffViewer(state) => {
                let child = state.finish_check_child.as_mut().expect("child kept");
                assert_eq!(child.id(), child_id);
                child.kill().ok();
                child.wait().ok();
            }
            _ => panic!("expected the viewer to stay open while the check runs"),
        }
    }

    #[test]
    fn v_toggles_layout_in_final_review() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state) if state.layout == crate::app::DiffViewerLayout::Unified
        ));

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();

        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state) if state.layout == crate::app::DiffViewerLayout::SideBySide
        ));
    }

    #[test]
    fn v_on_new_file_in_final_review_is_a_no_op_with_a_message() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.files[0].status = DiffFileStatus::Added;
        }

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();

        // The toggle is a no-op (still Unified, the only layout new files can
        // render), but unlike the pre-fix behavior it must explain why rather
        // than silently doing nothing.
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state) if state.layout == crate::app::DiffViewerLayout::Unified
        ));
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.contains("new/untracked"))
        );
    }

    #[test]
    fn layout_preference_survives_a_new_file_in_final_review() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs", "b.rs"]);
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.files[1].status = DiffFileStatus::Untracked;
        }

        // Choose side-by-side on the ordinary file.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('v'))).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state) if state.layout == crate::app::DiffViewerLayout::SideBySide
        ));

        // Moving onto the untracked file forces the *render* to unified, but
        // must not overwrite the stored preference.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
        assert!(app.diff_viewer_selected_file_is_new());
        assert_eq!(
            app.diff_viewer_layout(),
            Some(crate::app::DiffViewerLayout::Unified)
        );
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state) if state.layout == crate::app::DiffViewerLayout::SideBySide
        ));

        // Moving back restores the side-by-side render.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('p'))).unwrap();
        assert!(!app.diff_viewer_selected_file_is_new());
        assert_eq!(
            app.diff_viewer_layout(),
            Some(crate::app::DiffViewerLayout::SideBySide)
        );
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

        // F again -> Blockers: c.rs was rejected explicitly (default Blocker
        // severity), so it stays visible.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
        assert_eq!(filter(&app), FileFilter::Blockers);
        assert_eq!(selected(&app), 2, "snaps onto the only blocker file");

        // F again wraps back to All (Changed is skipped with no prior review).
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
        assert_eq!(filter(&app), FileFilter::All);
    }

    #[test]
    fn file_comment_is_verdict_free_filterable_and_written() {
        use crate::app::FileFilter;
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('m'))).unwrap();
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state) if state.editing_file_comment
        ));
        for ch in "consider splitting this module".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(ch))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(
                    !state.decisions.contains_key("a.rs"),
                    "comment is verdict-free"
                );
                let comment = state.file_comments.get("a.rs").expect("comment stored");
                assert_eq!(comment.text, "consider splitting this module");
                assert!(!comment.resolved);
            }
            _ => panic!("expected diff viewer"),
        }

        // All -> Undecided -> Rejected -> Blockers -> FileComments. Empty
        // filters remain valid states; the file-comment step selects this file.
        for _ in 0..4 {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char('F'))).unwrap();
        }
        assert!(matches!(
            &app.mode,
            AppMode::DiffViewer(state) if state.file_filter == FileFilter::FileComments
        ));

        // Approving remains independent of the comment and finishing emits a
        // dedicated whole-file section.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        finish_review(&mut app);
        let feedback =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(feedback.contains("### File Comments"));
        assert!(feedback.contains("#### a.rs — [suggestion]"));
        assert!(feedback.contains("consider splitting this module"));
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

    /// Give the review app's single file two hunks (context + added line each),
    /// so addressable indices 0,1 belong to the first hunk and 2,3 to the
    /// second. Used by the jump-by-hunk tests.
    fn set_two_hunks(app: &mut App) {
        let make_hunk = |old_start: usize, new_start: usize| DiffHunk {
            header: format!("@@ -{old_start},1 +{new_start},2 @@"),
            old_start,
            old_lines: 1,
            new_start,
            new_lines: 2,
            lines: vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    text: " ctx".into(),
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    text: "+added".into(),
                },
            ],
        };
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.files[0].hunks = vec![make_hunk(1, 1), make_hunk(10, 11)];
        }
    }

    #[test]
    fn bracket_keys_jump_cursor_between_hunks() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_two_hunks(&mut app);

        let cursor = |app: &App| match &app.mode {
            AppMode::DiffViewer(s) => s.comment_cursor,
            _ => panic!("expected diff viewer"),
        };

        // `]` with the cursor off activates it on the first hunk's start.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(']'))).unwrap();
        assert_eq!(cursor(&app), Some(0));
        // `]` hops to the second hunk, then clamps there (no hunk after).
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(']'))).unwrap();
        assert_eq!(cursor(&app), Some(2));
        handle_diff_viewer_key(&mut app, key(KeyCode::Char(']'))).unwrap();
        assert_eq!(cursor(&app), Some(2), "clamps in the last hunk");

        // From mid-hunk, `[` first snaps back to the current hunk's start…
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
        assert_eq!(cursor(&app), Some(3));
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('['))).unwrap();
        assert_eq!(cursor(&app), Some(2));
        // …then to the previous hunk, clamping at the first.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('['))).unwrap();
        assert_eq!(cursor(&app), Some(0));
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('['))).unwrap();
        assert_eq!(cursor(&app), Some(0), "clamps at the first hunk");
    }

    #[test]
    fn line_comment_auto_rejects_file_and_removal_clears_it() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        // Storing a comment defaults the undecided file to "needs revision"
        // with empty feedback (the comment carries the specifics).
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for c in "bug".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert_eq!(
                    state.decisions.get("a.rs"),
                    Some(&crate::app::ReviewDecision::Reject {
                        feedback: String::new(),
                        severity: crate::app::Severity::Suggestion,
                    })
                );
                assert!(state.auto_rejected.contains("a.rs"));
            }
            _ => panic!("expected diff viewer"),
        }

        // Deleting the file's only comment (empty re-submit) clears the
        // auto-set verdict again.
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.feedback_editor = crate::editor::TextEditor::new(String::new());
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(state.decisions.is_empty());
                assert!(state.auto_rejected.is_empty());
            }
            _ => panic!("expected diff viewer"),
        }
    }

    #[test]
    fn explicit_verdict_wins_over_comment_auto_reject() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        // Comment (auto-rejects), then explicitly approve: the approval wins.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for c in "note".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        let decision = |app: &App| match &app.mode {
            AppMode::DiffViewer(s) => s.decisions.get("a.rs").cloned(),
            _ => panic!("expected diff viewer"),
        };
        assert_eq!(decision(&app), Some(crate::app::ReviewDecision::Approve));

        // Editing the comment afterwards does not re-default the verdict.
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for c in " more".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(decision(&app), Some(crate::app::ReviewDecision::Approve));
    }

    #[test]
    fn skip_after_comment_clears_implicit_reject() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for c in "note".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('s'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(state.decisions.is_empty(), "skip clears the verdict");
                assert!(state.auto_rejected.is_empty());
                // The comment itself is kept — only the verdict was cleared.
                assert!(
                    state
                        .line_comments
                        .get("a.rs")
                        .is_some_and(|c| c.len() == 1)
                );
            }
            _ => panic!("expected diff viewer"),
        }
    }

    /// Seed an unaccepted AI draft comment on the review app's added line
    /// (addressable index 1, new line 2 — where `c` places the cursor).
    fn seed_draft_comment(app: &mut App) {
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.line_comments.insert(
                "a.rs".into(),
                vec![crate::app::LineComment {
                    location: crate::diff::DiffLineLocation {
                        old_line: None,
                        new_line: Some(2),
                    },
                    start: None,
                    text: "AI finding".into(),
                    draft: true,
                    suggestion: None,
                    severity: crate::app::Severity::default(),
                    anchor_context: None,
                    start_anchor_context: None,
                    anchor_lost: false,
                    resolved: false,
                    carried: false,
                }],
            );
        }
    }

    #[test]
    fn ctrl_e_cycles_line_comment_severity() {
        use crate::app::Severity;
        let ctrl = |c: char| KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        // Open the line-comment editor (defaults to Suggestion), cycle once with
        // Ctrl+E to Nit, type, and submit.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        handle_diff_viewer_key(&mut app, ctrl('e')).unwrap();
        for c in "typo".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();

        match &app.mode {
            AppMode::DiffViewer(state) => {
                let comment = &state.line_comments.get("a.rs").unwrap()[0];
                assert_eq!(comment.severity, Severity::Nit);
                // A blocker line comment would drive the Blockers filter; a nit
                // does not.
                assert!(!state.file_has_blocker("a.rs"));
            }
            _ => panic!("expected diff viewer"),
        }
    }

    #[test]
    fn accepted_draft_auto_rejects_dismissed_draft_does_not() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        // Dismissing a draft leaves the file undecided (a draft was never a
        // kept comment).
        seed_draft_comment(&mut app);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('d'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert!(state.decisions.is_empty());
            }
            _ => panic!("expected diff viewer"),
        }

        // Accepting a draft is a human-affirmed finding: it counts like a
        // hand-written comment and defaults the verdict.
        seed_draft_comment(&mut app);
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('a'))).unwrap();
        match &app.mode {
            AppMode::DiffViewer(state) => {
                assert_eq!(
                    state.decisions.get("a.rs"),
                    Some(&crate::app::ReviewDecision::Reject {
                        feedback: String::new(),
                        severity: crate::app::Severity::Suggestion,
                    })
                );
                assert!(state.auto_rejected.contains("a.rs"));
            }
            _ => panic!("expected diff viewer"),
        }
    }

    #[test]
    fn implicit_reject_points_agent_at_line_comments() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["a.rs"]);
        set_single_hunk(&mut app);

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('c'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Enter)).unwrap();
        for c in "bug".chars() {
            handle_diff_viewer_key(&mut app, key(KeyCode::Char(c))).unwrap();
        }
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        // Auto-rejected, so the review is fully decided: q opens the summary
        // and a second q finishes.
        finish_review(&mut app);

        let feedback =
            std::fs::read_to_string(dir.path().join(".claude/final-review-feedback.md")).unwrap();
        assert!(feedback.contains("**Needs work:** 1"));
        // The rejection heading carries the (auto-reject default) severity tag.
        assert!(feedback.contains("#### a.rs — [suggestion]\n"));
        assert!(feedback.contains("(Needs revision — see this file's line comments below)"));
        assert!(feedback.contains("bug"));
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
    #[test]
    fn file_list_j_k_walk_tree_rows_and_z_folds_a_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["README.md", "src/a.rs", "src/b.rs"]);

        // Rows: README.md, dir src, a.rs, b.rs. j from the first file lands on
        // the directory header without changing the diffed file.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
        let state = match &app.mode {
            AppMode::DiffViewer(state) => state,
            _ => panic!("left the diff viewer"),
        };
        assert_eq!(state.tree_cursor_dir.as_deref(), Some("src"));
        assert_eq!(state.selected_file, 0);

        // z folds it; the two files beneath lose their rows.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('z'))).unwrap();
        let state = match &app.mode {
            AppMode::DiffViewer(state) => state,
            _ => panic!("left the diff viewer"),
        };
        assert!(state.collapsed_dirs.contains("src"));
        assert_eq!(state.file_tree_rows().len(), 2);

        // n keeps its file meaning and re-expands what it lands inside.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('n'))).unwrap();
        let state = match &app.mode {
            AppMode::DiffViewer(state) => state,
            _ => panic!("left the diff viewer"),
        };
        assert_eq!(state.selected_file, 1);
        assert!(state.collapsed_dirs.is_empty());
    }

    #[test]
    fn tree_fold_keys_are_ignored_while_the_patch_is_focused() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["src/a.rs"]);
        handle_diff_viewer_key(&mut app, key(KeyCode::Tab)).unwrap();
        assert_eq!(app.diff_viewer_focus(), Some(DiffViewerFocus::Patch));

        handle_diff_viewer_key(&mut app, key(KeyCode::Char('z'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('Z'))).unwrap();

        let state = match &app.mode {
            AppMode::DiffViewer(state) => state,
            _ => panic!("left the diff viewer"),
        };
        assert!(
            state.collapsed_dirs.is_empty(),
            "folding belongs to the file list, not the patch panel"
        );
    }
    /// The tree is not a review-mode feature: the plain diff viewer (leader d)
    /// shares `draw_file_list` and the same fold bindings, so it groups and
    /// folds identically.
    #[test]
    fn plain_diff_viewer_gets_the_tree_and_fold_keys_too() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = make_review_app(dir.path(), &["README.md", "src/a.rs", "src/ui/b.rs"]);
        if let AppMode::DiffViewer(state) = &mut app.mode {
            state.review = false;
        }

        let rows = match &app.mode {
            AppMode::DiffViewer(state) => state.file_tree_rows(),
            _ => panic!("left the diff viewer"),
        };
        assert_eq!(
            rows.len(),
            5,
            "expected README + src/ + a.rs + ui/ + b.rs outside review mode: {rows:?}"
        );

        // j walks onto the `src` directory row, z folds it.
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('j'))).unwrap();
        handle_diff_viewer_key(&mut app, key(KeyCode::Char('z'))).unwrap();
        let state = match &app.mode {
            AppMode::DiffViewer(state) => state,
            _ => panic!("left the diff viewer"),
        };
        assert!(state.collapsed_dirs.contains("src"));
        assert_eq!(state.file_tree_rows().len(), 2);
        assert_eq!(
            state.selected_file, 0,
            "folding must not change the previewed file outside review mode either"
        );
    }
}
