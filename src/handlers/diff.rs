use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{App, AppMode, DiffViewerFocus};

const PATCH_SCROLL_STEP: usize = 1;
/// How far Ctrl+J/K (or Ctrl+↓/↑) moves, for covering ground faster than `j`/`k`.
const PATCH_JUMP_STEP: usize = 10;
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

/// Lines a `j`/`k` press moves: one, or [`PATCH_JUMP_STEP`] with Ctrl held.
pub(crate) fn scroll_step(key: &KeyEvent) -> usize {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        PATCH_JUMP_STEP
    } else {
        PATCH_SCROLL_STEP
    }
}

pub fn handle_diff_viewer_key(app: &mut App, key: KeyEvent) -> Result<()> {
    let code = key.code;
    let step = scroll_step(&key);

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

    // The final-review destination picker (`t`) and the companion-feature setup
    // overlay it can open are sub-states of the diff viewer, mirroring PR
    // Triage's `harness_pick` / `new_feature_setup`. Each captures every key
    // while open, before any of the viewer's own bindings.
    if app.review_feature_setup_open() {
        return crate::handlers::handle_review_feature_setup_key(app, key);
    }
    if app.review_destination_pick_open() {
        return crate::handlers::handle_review_destination_pick_key(app, key);
    }

    // The review key-help overlay is read-only and captures every key while
    // open, so a key pressed to dismiss it can never also act on the review
    // underneath. `?` toggles, matching the dashboard's help overlay.
    let help_open = matches!(&app.mode, AppMode::DiffViewer(state) if state.help_open);
    if help_open {
        match code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => app.close_review_help(),
            KeyCode::Char('j') | KeyCode::Down => app.review_help_scroll_down(step),
            KeyCode::Char('k') | KeyCode::Up => app.review_help_scroll_up(step),
            KeyCode::PageDown => app.review_help_scroll_down(PATCH_PAGE_STEP),
            KeyCode::PageUp => app.review_help_scroll_up(PATCH_PAGE_STEP),
            KeyCode::Home | KeyCode::Char('g') => app.review_help_scroll_top(),
            KeyCode::End | KeyCode::Char('G') => app.review_help_scroll_bottom(),
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
            KeyCode::Char('j') | KeyCode::Down => app.review_history_scroll_down(step),
            KeyCode::Char('k') | KeyCode::Up => app.review_history_scroll_up(step),
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
            KeyCode::Char('j') | KeyCode::Down => app.changeset_overview_scroll_down(step),
            KeyCode::Char('k') | KeyCode::Up => app.changeset_overview_scroll_up(step),
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
            KeyCode::Char('j') | KeyCode::Down => app.interdiff_scroll_down(step),
            KeyCode::Char('k') | KeyCode::Up => app.interdiff_scroll_up(step),
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

    // Vim is a session-wide final-review preference, so the same binding works
    // before an editor opens and while any editor is active. Read-only overlays
    // above still retain key precedence.
    if review && key.modifiers.contains(KeyModifiers::CONTROL) && code == KeyCode::Char('t') {
        app.diff_review_toggle_vim();
        return Ok(());
    }

    // While typing feedback (per-file rejection or general) the keys drive a
    // multi-line `TextEditor`; Enter inserts a newline, so Tab submits.
    if editing_feedback {
        return handle_feedback_editor_key(app, key, editing_general);
    }

    // The PR review's submit dialog takes every key while it is open (its `e`
    // opens the general-feedback editor above, which then has precedence).
    if app.pr_submit_open() {
        match code {
            KeyCode::Esc => app.pr_submit_close(),
            KeyCode::Left | KeyCode::Up | KeyCode::Char('h') | KeyCode::Char('k') => {
                app.pr_submit_cycle_event(-1)
            }
            KeyCode::Right
            | KeyCode::Down
            | KeyCode::Tab
            | KeyCode::Char('l')
            | KeyCode::Char('j') => app.pr_submit_cycle_event(1),
            KeyCode::BackTab => app.pr_submit_cycle_event(-1),
            KeyCode::Char('e') => app.pr_submit_edit_summary(),
            KeyCode::Enter => app.pr_submit_post(),
            KeyCode::Char('o') => app.pr_submit_reopen_at_new_head(),
            _ => {}
        }
        return Ok(());
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
                    app.review_notes_scroll_down(step);
                    return Ok(());
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.review_notes_scroll_up(step);
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
                    app.diff_review_cursor_move(step as isize);
                    return Ok(());
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    app.diff_review_cursor_move(-(step as isize));
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
                // Tab stays drafts-in-this-file; `{`/`}` walk every comment in
                // the review, so the cursor can sweep the whole annotation set.
                KeyCode::Char('}') => {
                    app.diff_review_jump_comment(1);
                    return Ok(());
                }
                KeyCode::Char('{') => {
                    app.diff_review_jump_comment(-1);
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
                KeyCode::Char('E') => {
                    app.diff_review_open_in_editor();
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
            // Reached from the line cursor too: the cursor-active block above
            // doesn't claim `?`, so the overlay is one key away from anywhere in
            // the review that isn't already a text input or a modal.
            KeyCode::Char('?') => {
                app.open_review_help();
                return Ok(());
            }
            KeyCode::Char('Q') => {
                app.open_review_questions();
                return Ok(());
            }
            KeyCode::Char('H') => {
                app.open_review_history();
                return Ok(());
            }
            KeyCode::Char('c') => {
                app.diff_review_toggle_line_cursor();
                return Ok(());
            }
            // Without the cursor active this still opens the file, just at the
            // first hunk rather than a chosen line.
            KeyCode::Char('E') => {
                app.diff_review_open_in_editor();
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
            // Widen / narrow the context around each hunk, so the enclosing
            // function or match arm can be read without leaving the review.
            KeyCode::Char('+') | KeyCode::Char('=') => {
                app.diff_viewer_expand_context();
                return Ok(());
            }
            KeyCode::Char('-') | KeyCode::Char('_') => {
                app.diff_viewer_collapse_context();
                return Ok(());
            }
            KeyCode::Char('*') => {
                app.diff_viewer_toggle_whole_file_context();
                return Ok(());
            }
            KeyCode::Char('W') => {
                app.diff_viewer_toggle_ignore_whitespace();
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
            KeyCode::Char('U') => {
                app.diff_review_undo_verdict();
                return Ok(());
            }
            KeyCode::Char('}') => {
                app.diff_review_jump_comment(1);
                return Ok(());
            }
            KeyCode::Char('{') => {
                app.diff_review_jump_comment(-1);
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
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.diff_viewer_expand_context();
        }
        KeyCode::Char('-') | KeyCode::Char('_') => {
            app.diff_viewer_collapse_context();
        }
        KeyCode::Char('*') => {
            app.diff_viewer_toggle_whole_file_context();
        }
        KeyCode::Char('W') => {
            app.diff_viewer_toggle_ignore_whitespace();
        }
        KeyCode::Char('.') => {
            app.diff_viewer_toggle_amf_files();
        }
        // In the file list, j/k walk *tree rows* (directories included); n/p and
        // the verdict-advance paths still move file-to-file.
        KeyCode::Char('j') | KeyCode::Down => match app.diff_viewer_focus() {
            Some(DiffViewerFocus::FileList) => app.diff_viewer_tree_move(1),
            Some(DiffViewerFocus::Patch) => app.diff_viewer_scroll_patch_down(step),
            None => {}
        },
        KeyCode::Char('k') | KeyCode::Up => match app.diff_viewer_focus() {
            Some(DiffViewerFocus::FileList) => app.diff_viewer_tree_move(-1),
            Some(DiffViewerFocus::Patch) => app.diff_viewer_scroll_patch_up(step),
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

/// Scrolls whatever the diff viewer is showing by `lines`: an open read-only
/// overlay, the feedback editor, the expanded notes panel, or else the patch.
/// The wheel scrolls the view rather than moving the line cursor, and does
/// nothing over prompts and pickers, where a stray scroll would change a
/// choice.
pub fn handle_diff_viewer_wheel(app: &mut App, lines: usize, down: bool) {
    enum Target {
        Help,
        History,
        Overview,
        Interdiff,
        Feedback,
        Notes,
        Patch,
        None,
    }

    if app.review_feature_setup_open() || app.review_destination_pick_open() || app.pr_submit_open()
    {
        return;
    }
    let target = match &app.mode {
        AppMode::DiffViewer(state) => {
            if state.editing_base_ref || state.editing_search || state.summary_open {
                Target::None
            } else if state.help_open {
                Target::Help
            } else if state.review_history.is_some() {
                Target::History
            } else if state.changeset_overview_open {
                Target::Overview
            } else if state.interdiff_open {
                Target::Interdiff
            } else if state.feedback_editing
                || state.editing_general
                || state.editing_line_comment
                || state.editing_file_comment
                || state.editing_suggestion
            {
                Target::Feedback
            } else if state.notes_expanded {
                Target::Notes
            } else {
                Target::Patch
            }
        }
        _ => Target::None,
    };

    match (target, down) {
        (Target::Help, true) => app.review_help_scroll_down(lines),
        (Target::Help, false) => app.review_help_scroll_up(lines),
        (Target::History, true) => app.review_history_scroll_down(lines),
        (Target::History, false) => app.review_history_scroll_up(lines),
        (Target::Overview, true) => app.changeset_overview_scroll_down(lines),
        (Target::Overview, false) => app.changeset_overview_scroll_up(lines),
        (Target::Interdiff, true) => app.interdiff_scroll_down(lines),
        (Target::Interdiff, false) => app.interdiff_scroll_up(lines),
        (Target::Feedback, _) => {
            if let AppMode::DiffViewer(state) = &mut app.mode {
                state.feedback_scroll = if down {
                    state.feedback_scroll.saturating_add(lines)
                } else {
                    state.feedback_scroll.saturating_sub(lines)
                };
                state.feedback_sync_to_cursor = false;
            }
        }
        (Target::Notes, true) => app.review_notes_scroll_down(lines),
        (Target::Notes, false) => app.review_notes_scroll_up(lines),
        (Target::Patch, true) => app.diff_viewer_scroll_patch_down(lines),
        (Target::Patch, false) => app.diff_viewer_scroll_patch_up(lines),
        (Target::None, _) => {}
    }
}

/// Drive the multi-line feedback editor (per-file rejection or general
/// feedback). Tab submits, Esc cancels in plain mode, Ctrl+Q always cancels,
/// Ctrl+J/K plus PgUp/PgDn scroll the editor. The session-wide Ctrl+T Vim
/// toggle is handled by [`handle_diff_viewer_key`] before dispatch reaches
/// this function.
fn handle_feedback_editor_key(app: &mut App, key: KeyEvent, editing_general: bool) -> Result<()> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    if ctrl && key.code == KeyCode::Char('q') {
        app.diff_review_cancel_feedback();
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
mod tests;
