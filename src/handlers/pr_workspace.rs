//! Workspace navigation is handled here; real review actions stay in their
//! existing handlers, including all editor/confirmation key precedence.

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::app::{
    App, AppMode, DiffViewerFocus,
    pr_workspace::{WorkspacePane, WorkspaceTab, engine_modal},
};

pub(crate) fn handle(app: &mut App, key: KeyEvent, visible_rows: u16) -> Result<()> {
    if app
        .pr_workspace
        .as_ref()
        .is_none_or(|workspace| workspace.pr.is_none())
    {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                app.close_pr_workspace();
            }
            KeyCode::Tab | KeyCode::BackTab => {}
            _ => {
                super::handle_key_inner(app, key, visible_rows)?;
            }
        }
        return Ok(());
    }
    if matches!(app.mode, AppMode::Viewing(_))
        && app.leader_active
        && matches!(key.code, KeyCode::Char('U') | KeyCode::Char('W'))
        && !app.leave_pr_workspace_preserving_mode()
    {
        return Ok(());
    }
    if WorkspaceTab::for_mode(&app.mode).is_none() || engine_modal(&app.mode) {
        return super::handle_key_inner(app, key, visible_rows);
    }
    let workspace = app.pr_workspace.as_mut().expect("workspace checked");
    if workspace.help {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => workspace.help = false,
            KeyCode::Char('j') | KeyCode::Down => {
                workspace.help_scroll = workspace.help_scroll.saturating_add(1)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                workspace.help_scroll = workspace.help_scroll.saturating_sub(1)
            }
            KeyCode::PageDown => workspace.help_scroll = workspace.help_scroll.saturating_add(10),
            KeyCode::PageUp => workspace.help_scroll = workspace.help_scroll.saturating_sub(10),
            _ => {}
        }
        return Ok(());
    }
    // Only unmodified keys belong to the shell. Ctrl+digit etc. retain the
    // backend's semantics; a text editor was already given full precedence.
    if !key
        .modifiers
        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
    {
        match key.code {
            KeyCode::Char('1') => {
                app.browse_pr_workspace(WorkspaceTab::Files);
                return Ok(());
            }
            KeyCode::Char('2') => {
                app.browse_pr_workspace(WorkspaceTab::Threads);
                return Ok(());
            }
            KeyCode::Char('3') => {
                app.browse_pr_workspace(WorkspaceTab::Ai);
                return Ok(());
            }
            KeyCode::Tab | KeyCode::BackTab => {
                workspace.focus = workspace.focus.next(key.code == KeyCode::BackTab);
                return Ok(());
            }
            KeyCode::Char('z') => {
                workspace.expanded = !workspace.expanded;
                return Ok(());
            }
            KeyCode::Char('?') => {
                workspace.help = true;
                workspace.help_scroll = 0;
                return Ok(());
            }
            KeyCode::Esc if workspace.expanded => {
                workspace.expanded = false;
                return Ok(());
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                app.close_pr_workspace();
                return Ok(());
            }
            KeyCode::Char('A') => {
                if app.switch_pr_workspace(WorkspaceTab::Ai) {
                    app.start_ai_pr_review();
                    // Progress stays in the header, so files remain one key away.
                    app.cancel_ai_pr_review();
                }
                return Ok(());
            }
            // Human review and AI posting remain separately attributed and
            // separately confirmed, using the existing submit dialogs.
            KeyCode::Char('p') => {
                if app.switch_pr_workspace(WorkspaceTab::Files) {
                    app.open_pr_submit();
                }
                return Ok(());
            }
            KeyCode::F(5) => {
                app.reload_pr_workspace();
                return Ok(());
            }
            KeyCode::Enter if workspace.focus == WorkspacePane::Navigator => {
                workspace.focus = WorkspacePane::Diff;
                return Ok(());
            }
            _ => {}
        }
    }
    let (tab, focus) = (workspace.tab, workspace.focus);
    if tab != WorkspaceTab::Files
        && matches!(key.code, KeyCode::Char('f') | KeyCode::Char('B'))
        && workspace.observed_head_moved(&app.mode)
    {
        app.push_toast_warning(
            "The PR has new commits · press F5 to reopen it before sending a fix",
        );
        return Ok(());
    }
    if focus == WorkspacePane::Diff && workspace.anchor_message.is_some() {
        app.message =
            Some("Press 1 to browse code, or F5 to reopen this PR at its latest head".into());
        return Ok(());
    }
    if tab == WorkspaceTab::Threads && matches!(app.mode, AppMode::PrReviewLoading(_)) {
        if key.code == KeyCode::Char('r') {
            app.retry_workspace_comments();
        }
        return Ok(());
    }
    let target = if focus == WorkspacePane::Diff || tab == WorkspaceTab::Files {
        WorkspaceTab::Files
    } else {
        tab
    };
    if !app.switch_pr_workspace(target) {
        return Ok(());
    }
    if let AppMode::DiffViewer(state) = &mut app.mode {
        state.focus = if focus == WorkspacePane::Navigator {
            DiffViewerFocus::FileList
        } else {
            DiffViewerFocus::Patch
        };
        // The workspace has its own context pane; don't let an old expanded
        // notes flag swallow code navigation after returning from an editor.
        state.notes_expanded = focus == WorkspacePane::Context;
    }
    // Preserve the AI view before the existing f/B handoff consumes it, and
    // supply the exact parked triage state instead of opening a second copy.
    if target == WorkspaceTab::Ai
        && matches!(key.code, KeyCode::Char('f') | KeyCode::Char('B'))
        && let AppMode::AiReview(state) = &app.mode
    {
        let snapshot = AppMode::AiReview(state.clone());
        let workspace = app.pr_workspace.as_mut().expect("workspace exists");
        workspace.parked[WorkspaceTab::Ai.index()] = Some(Box::new(snapshot));
        app.ai_review_return_to = workspace.parked[WorkspaceTab::Threads.index()].take();
    }
    let context_navigation = focus == WorkspacePane::Context && target != WorkspaceTab::Files;
    if context_navigation
        && matches!(
            key.code,
            KeyCode::Char('j') | KeyCode::Down | KeyCode::Char('k') | KeyCode::Up
        )
    {
        let down = matches!(key.code, KeyCode::Char('j') | KeyCode::Down);
        match &mut app.mode {
            AppMode::PrReview(state) => {
                state.detail_scroll = if down {
                    state
                        .detail_scroll
                        .saturating_add(1)
                        .min(state.detail_content_lines.saturating_sub(1))
                } else {
                    state.detail_scroll.saturating_sub(1)
                }
            }
            AppMode::AiReview(state) => {
                state.detail_scroll = if down {
                    state
                        .detail_scroll
                        .saturating_add(1)
                        .min(state.detail_content_lines.saturating_sub(1))
                } else {
                    state.detail_scroll.saturating_sub(1)
                }
            }
            _ => {}
        }
    } else if target == WorkspaceTab::Threads && key.code == KeyCode::Char('g') {
        // Changing PR goes through the one shared picker, after saving drafts.
        let workdir = app
            .pr_workspace
            .as_ref()
            .expect("workspace exists")
            .workdir
            .clone();
        if app.close_pr_workspace() {
            app.open_pr_workspace_for_workdir(workdir);
        }
    } else {
        super::handle_key_inner(app, key, visible_rows)?;
    }
    app.reconcile_pr_workspace();
    if target == WorkspaceTab::Ai
        && WorkspaceTab::for_mode(&app.mode) == Some(WorkspaceTab::Ai)
        && let Some(triage) = app.ai_review_return_to.take()
        && let Some(workspace) = &mut app.pr_workspace
    {
        workspace.parked[WorkspaceTab::Threads.index()] = Some(triage);
    }
    // A code-only action shouldn't switch away from the selected feedback.
    // Editors stay active until they close, so their input is never intercepted.
    if target != tab && !engine_modal(&app.mode) {
        app.switch_pr_workspace(tab);
    }
    if let Some(workspace) = &mut app.pr_workspace {
        workspace.focus = focus;
    }
    app.sync_pr_workspace_anchor();
    Ok(())
}

pub(crate) fn mouse(app: &mut App, mouse: MouseEvent, visible_rows: u16) -> Result<bool> {
    if !app.pr_workspace_owns_engines()
        || WorkspaceTab::for_mode(&app.mode).is_none()
        || engine_modal(&app.mode)
    {
        return Ok(false);
    }
    let Some(workspace) = &mut app.pr_workspace else {
        return Ok(false);
    };
    if workspace.help {
        return Ok(true);
    }
    let layout = crate::ui::pr_workspace_pane_areas(
        ratatui::layout::Rect::new(0, 0, app.viewport_cols, app.viewport_total_rows),
        workspace.focus,
        workspace.expanded,
    );
    if let Some((pane, _)) = layout
        .iter()
        .rev()
        .find(|(_, area)| area.contains((mouse.column, mouse.row).into()))
    {
        workspace.focus = *pane;
    }
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            handle(
                app,
                KeyEvent::new(KeyCode::Down, KeyModifiers::NONE),
                visible_rows,
            )?;
        }
        MouseEventKind::ScrollUp => {
            handle(
                app,
                KeyEvent::new(KeyCode::Up, KeyModifiers::NONE),
                visible_rows,
            )?;
        }
        MouseEventKind::Down(MouseButton::Left) => {}
        _ => {}
    }
    Ok(true)
}

/// Bracketed paste inserts text into the active editor without interpreting
/// digits, newlines, or action letters as workspace commands.
pub(crate) fn paste(app: &mut App, text: &str) -> bool {
    if !app.pr_workspace_owns_engines() {
        return false;
    }
    let editor = match &mut app.mode {
        AppMode::DiffViewer(state)
            if state.feedback_editing
                || state.editing_general
                || state.editing_line_comment
                || state.editing_file_comment
                || state.editing_suggestion =>
        {
            state.feedback_sync_to_cursor = true;
            Some(&mut state.feedback_editor)
        }
        AppMode::AiReview(state) if state.harness_pick.is_none() && state.model_pick.is_none() => {
            if state.finding_editor.is_some() {
                state.finding_editor.as_mut()
            } else {
                state
                    .post_confirm
                    .as_mut()
                    .filter(|post| post.editing)
                    .map(|post| &mut post.editor)
            }
        }
        AppMode::PrReview(state)
            if state.harness_pick.is_none()
                && state.new_feature_setup.is_none()
                && state.investigation_harness_pick.is_none()
                && state.investigation_action_pick.is_none()
                && state.integrate.is_none()
                && state.reply_kind_pick.is_none()
                && state.mark_pick.is_none() =>
        {
            if let Some(draft) = &mut state.investigation_follow_up {
                Some(&mut draft.editor)
            } else if state.investigation_context.editor.is_some() {
                state.investigation_context.editor.as_mut()
            } else if let Some(confirm) = &mut state.fix_confirm {
                confirm.sync_to_cursor = true;
                confirm.editing.then_some(&mut confirm.editor)
            } else if let Some(reply) = &mut state.reply {
                reply.editing.then_some(&mut reply.editor)
            } else {
                state
                    .memory_add
                    .as_mut()
                    .filter(|memory| memory.editing && memory.ai_summary.is_none())
                    .map(|memory| &mut memory.editor)
            }
        }
        _ => None,
    };
    if let Some(editor) = editor {
        editor.insert_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
        true
    } else {
        false
    }
}
