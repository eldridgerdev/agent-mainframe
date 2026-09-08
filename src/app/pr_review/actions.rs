use super::{MarkAction, PrComment, ReplyDraftRequest, TriageState};
use crate::app::{App, AppMode, FixConfirmState, MarkPickState};
use crate::editor::TextEditor;
use crate::github::GhCli;

/// Append the provider-neutral reply-draft handoff to the exact prompt shown
/// in the fix confirmation dialog. Every built-in harness can run the hidden
/// `amf reply-draft` CLI; the body travels over stdin and the command turns it
/// into structured IPC, avoiding brittle terminal-output scraping.
pub(super) fn with_reply_draft_handoff(
    mut prompt: String,
    pr_number: u32,
    requests: &[ReplyDraftRequest],
) -> String {
    prompt.push_str(
        "\n\nAfter implementing and verifying each fix, draft a concise reviewer-facing \
         reply explaining what changed and any relevant validation. Do not post \
         replies to GitHub yourself. Return each draft to AMF by passing only the \
         reply text on stdin to its matching command. Do not include a commit \
         hash; AMF will add the best-matching commit reference:",
    );
    for request in requests {
        prompt.push_str(&format!(
            "\n\nComment ID {comment_id}:\n\
             amf reply-draft --pr-number {pr_number} --comment-id {comment_id} \
             --request-id {request_id} <<'AMF_REPLY'\n\
             <concise reply text>\n\
             AMF_REPLY",
            comment_id = request.comment_id,
            request_id = request.request_id,
        ));
    }
    prompt
}

/// Build a fresh fix-confirm dialog seeded with `prompt`. The editor opens with
/// the vim keymap when `vim` is set (the pane-level remembered preference) so
/// reopening the dialog for another comment keeps the user's chosen keymap.
/// Build a fresh fix-confirm dialog. `batch` is `None` for an ordinary
/// single-comment fix and `Some(ids)` for the combined-batch flow (`B`), where
/// injecting marks every listed comment `Fixing`.
pub(super) fn new_fix_confirm(
    prompt: String,
    vim: bool,
    batch: Option<Vec<u64>>,
    reply_draft_requests: Vec<ReplyDraftRequest>,
) -> FixConfirmState {
    FixConfirmState {
        editor: if vim {
            TextEditor::with_vim(prompt)
        } else {
            TextEditor::new(prompt)
        },
        editing: false,
        scroll: 0,
        // Seed the view scrolled to the cursor (end of the prompt for plain,
        // start for vim) so a tall prompt opens somewhere sensible.
        sync_to_cursor: true,
        batch,
        reply_draft_requests,
    }
}

impl App {
    /// Persist one comment's triage state (with an optional note) to SQLite. A
    /// write failure is non-fatal (logged, not surfaced).
    pub(super) fn persist_triage(
        &mut self,
        pr_number: u32,
        head_sha: &str,
        comment_id: u64,
        state: TriageState,
        note: Option<&str>,
        batch_id: Option<&str>,
    ) {
        let result = match self.db.as_ref() {
            Some(db) => {
                db.save_pr_comment_triage(pr_number, head_sha, comment_id, state, note, batch_id)
            }
            None => return,
        };
        if let Err(e) = result {
            self.log_warn("pr_review", format!("triage persist failed: {e}"));
        }
    }

    /// Persist a resolved combined-batch comment's shared fix cost onto every
    /// sibling triage row that doesn't have one yet (first writer wins). No-op
    /// without a DB, when the comment isn't part of a batch, or when the cost is
    /// already recorded. Non-fatal on failure.
    pub(super) fn persist_batch_fix_cost(&mut self, pr_number: u32, comment_id: u64, cost: &str) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let batch_id = match db.load_pr_comment_triage(pr_number) {
            Ok(map) => map.get(&comment_id).and_then(|row| row.batch_id.clone()),
            Err(e) => {
                self.log_warn("pr_review", format!("batch fix-cost lookup failed: {e}"));
                return;
            }
        };
        let Some(batch_id) = batch_id else {
            return;
        };
        if let Err(e) = db.set_pr_comment_batch_fix_cost(pr_number, &batch_id, cost) {
            self.log_warn("pr_review", format!("batch fix-cost persist failed: {e}"));
        }
    }

    /// Set the selected comment's triage state in-memory and persist it. The
    /// comment keeps its existing `local_note`. No-op outside PR Triage or
    /// with no selection.
    pub(super) fn pr_review_set_triage(&mut self, state: TriageState) {
        let Some((pr_number, head_sha, comment_id, note)) = ({
            let AppMode::PrReview(s) = &mut self.mode else {
                return;
            };
            s.review.comments.get_mut(s.selected).map(|c| {
                c.triage = state;
                (
                    s.review.pr.number,
                    s.review.pr.head_sha.clone(),
                    c.id,
                    c.local_note.clone(),
                )
            })
        }) else {
            return;
        };
        self.persist_triage(
            pr_number,
            &head_sha,
            comment_id,
            state,
            note.as_deref(),
            None,
        );
    }

    /// Open the "Mark" picker (`m`): a three-row choice between local `Done`,
    /// local `Skip`, and toggling the GitHub thread's resolved state.
    /// Replaces the old separate `m`/`s`/`x` top-level keys with one entry
    /// point. No-op (with a hint) if nothing is selected or another dialog
    /// is already open.
    pub fn pr_review_open_mark_pick(&mut self) {
        let selected_actionable = match &self.mode {
            AppMode::PrReview(state)
                if state.reply.is_none()
                    && state.fix_confirm.is_none()
                    && state.reply_kind_pick.is_none()
                    && state.mark_pick.is_none() =>
            {
                state.selected_comment().map(PrComment::is_actionable)
            }
            _ => return,
        };
        match selected_actionable {
            Some(true) => {}
            Some(false) => {
                self.message = Some("AMF follow-up replies are shown for context only".to_string());
                return;
            }
            None => {
                self.message = Some("No comment selected".into());
                return;
            }
        }
        if let AppMode::PrReview(state) = &mut self.mode {
            state.mark_pick = Some(MarkPickState { selected: 0 });
        }
    }

    /// Whether the "Mark" picker is currently open over PR Triage.
    pub fn pr_review_mark_pick_picking(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state) if state.mark_pick.is_some()
        )
    }

    /// Move the "Mark"-picker highlight (`+1`/`-1`, wrapping).
    pub fn pr_review_mark_pick_move(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(pick) = &mut state.mark_pick
        {
            let len = MarkAction::ALL.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Confirm the "Mark" picker: close it and apply the chosen action
    /// immediately (reusing the existing done/skip/resolve flows as-is) —
    /// no further confirm step, matching the original single-key behavior.
    pub fn pr_review_mark_pick_confirm(&mut self) {
        let chosen = match &self.mode {
            AppMode::PrReview(state) => state
                .mark_pick
                .as_ref()
                .map(|pick| MarkAction::ALL[pick.selected]),
            _ => return,
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            state.mark_pick = None;
        }
        match chosen {
            Some(MarkAction::Done) => self.pr_review_mark_done(),
            Some(MarkAction::Skip) => self.pr_review_skip(),
            Some(MarkAction::ResolveOnGitHub) => self.pr_review_toggle_resolve(),
            None => {}
        }
    }

    /// Cancel the "Mark" picker without choosing.
    pub fn pr_review_mark_pick_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.mark_pick = None;
        }
    }

    /// Mark the selected comment done (toggles back to untriaged if it already
    /// is). Manual, with **no auto-advance** — the user stays on the comment so
    /// they can review the agent's work before moving on (plan: Epic B).
    /// Reached via the "Mark" picker (`m`), or called directly by
    /// tests/internal flows.
    pub fn pr_review_mark_done(&mut self) {
        let next = match self.pr_review_selected_triage() {
            Some(TriageState::Done) => TriageState::Untriaged,
            Some(_) => TriageState::Done,
            None => return,
        };
        self.pr_review_set_triage(next);
        let msg = match next {
            TriageState::Done => "Marked done",
            _ => "Cleared — back to untriaged",
        };
        self.push_toast_success(msg.to_string());
    }

    /// Skip the selected comment locally (toggles back to untriaged if already
    /// skipped). Local-only — no GitHub write, no agent tokens.
    pub fn pr_review_skip(&mut self) {
        let next = match self.pr_review_selected_triage() {
            Some(TriageState::Skipped) => TriageState::Untriaged,
            Some(_) => TriageState::Skipped,
            None => return,
        };
        self.pr_review_set_triage(next);
        let msg = match next {
            TriageState::Skipped => "Skipped",
            _ => "Cleared — back to untriaged",
        };
        self.push_toast_success(msg.to_string());
    }

    /// Triage state of the currently-selected comment, if any.
    pub(super) fn pr_review_selected_triage(&self) -> Option<TriageState> {
        match &self.mode {
            AppMode::PrReview(s) => s.selected_comment().map(|c| c.triage),
            _ => None,
        }
    }

    /// Toggle whether the selected comment is marked for a batch fix (`space`).
    /// Marks are kept by comment id, so they survive the hide-resolved filter
    /// shifting the visible rows. No-op with no selection.
    pub fn pr_review_toggle_mark(&mut self) {
        let selected = match &self.mode {
            AppMode::PrReview(state) => state
                .selected_comment()
                .map(|comment| (comment.id, comment.is_actionable())),
            _ => return,
        };
        let Some((id, actionable)) = selected else {
            return;
        };
        if !actionable {
            self.message = Some("AMF follow-up replies cannot be batched as fixes".to_string());
            return;
        }
        let AppMode::PrReview(state) = &mut self.mode else {
            return;
        };
        let now_marked = if state.marked.remove(&id) {
            false
        } else {
            state.marked.insert(id);
            true
        };
        let count = state.marked.len();
        self.message = Some(if now_marked {
            format!("Marked for batch fix ({count} marked)")
        } else if count == 0 {
            "Unmarked — nothing marked".to_string()
        } else {
            format!("Unmarked ({count} still marked)")
        });
    }

    /// Close PR Triage / cancel a pending load and return to the dashboard.
    pub fn close_pr_review(&mut self) {
        self.pr_review_bg = None;
        self.mode = AppMode::Normal;
    }

    pub fn pr_review_select_next(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            let visible = state.visible_indices();
            let next = match visible.iter().position(|&i| i == state.selected) {
                Some(pos) => visible.get(pos + 1).copied(),
                None => visible.first().copied(),
            };
            if let Some(next) = next {
                state.selected = next;
                state.detail_scroll = 0;
            }
        }
    }

    pub fn pr_review_select_prev(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            let visible = state.visible_indices();
            let prev = match visible.iter().position(|&i| i == state.selected) {
                Some(0) => None,
                Some(pos) => visible.get(pos - 1).copied(),
                None => visible.last().copied(),
            };
            if let Some(prev) = prev {
                state.selected = prev;
                state.detail_scroll = 0;
            }
        }
    }

    /// Move the selection to the next (`forward`) or previous visible comment
    /// that shares the selected comment's combined-batch id (`]` / `[`),
    /// cycling within the batch. A hint (no move) when the selection isn't part
    /// of a batch, or when no other sibling is currently in view.
    pub fn pr_review_jump_sibling(&mut self, forward: bool) {
        let mut hint: Option<&'static str> = None;
        if let AppMode::PrReview(state) = &mut self.mode {
            match state.selected_comment().and_then(|c| c.batch_id.clone()) {
                None => hint = Some("This comment isn't part of a combined batch"),
                Some(batch_id) => {
                    let siblings: Vec<usize> = state
                        .visible_indices()
                        .into_iter()
                        .filter(|&i| {
                            state.review.comments[i].batch_id.as_deref() == Some(batch_id.as_str())
                        })
                        .collect();
                    if siblings.len() < 2 {
                        hint = Some("No other comments from this batch are in view");
                    } else {
                        let cur = siblings
                            .iter()
                            .position(|&i| i == state.selected)
                            .unwrap_or(0);
                        let next = if forward {
                            (cur + 1) % siblings.len()
                        } else {
                            (cur + siblings.len() - 1) % siblings.len()
                        };
                        state.selected = siblings[next];
                        state.detail_scroll = 0;
                    }
                }
            }
        }
        if let Some(hint) = hint {
            self.push_toast_warning(hint);
        }
    }

    /// Toggle hiding GitHub-resolved comments. When the current selection
    /// becomes hidden, snap to its nearest remaining visible neighbor in sort
    /// order (falling back to the closest one before it, then the first
    /// visible comment).
    pub fn pr_review_toggle_resolved(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.hide_resolved = !state.hide_resolved;
            state.snap_selection_to_visible();
        }
    }

    /// Cycle the comment list's sort order (`o`): fetch order → by file → by
    /// author → humans-first → back to fetch order. Independent of the
    /// `hide_resolved` filter.
    pub fn pr_review_cycle_sort(&mut self) {
        let label = {
            let AppMode::PrReview(state) = &mut self.mode else {
                return;
            };
            state.sort_mode = state.sort_mode.next();
            state.sort_mode.label()
        };
        self.push_toast_success(format!("Sort: {label}"));
    }

    /// Whether the fix confirm/edit dialog is currently open, and if so whether
    /// it is in edit mode. `None` means no dialog is open.
    pub fn pr_review_fix_editing(&self) -> Option<bool> {
        match &self.mode {
            AppMode::PrReview(state) => state.fix_confirm.as_ref().map(|c| c.editing),
            _ => None,
        }
    }

    /// Close the fix confirm/edit dialog without injecting anything.
    pub fn pr_review_cancel_fix(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.fix_confirm = None;
        }
    }

    /// Switch the open fix dialog into edit mode so keystrokes flow to the
    /// prompt editor. No-op when the dialog is closed or already editing.
    pub fn pr_review_fix_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
        {
            confirm.editing = true;
        }
    }

    /// Leave edit mode, returning to the confirm view (the prompt is kept).
    pub fn pr_review_fix_stop_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
        {
            confirm.editing = false;
        }
    }

    /// Forward a key to the open fix-prompt editor (only meaningful in edit
    /// mode). Returns `true` when a dialog editor consumed the key. Requests a
    /// cursor-follow scroll when the edit moved the cursor or changed the text.
    pub fn pr_review_fix_editor_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
            && confirm.editing
        {
            let outcome = confirm.editor.handle_key(key);
            if outcome.text_changed || outcome.cursor_moved {
                confirm.sync_to_cursor = true;
            }
            return true;
        }
        false
    }

    /// Toggle the vim keymap on the open fix-prompt editor, remembering the
    /// choice on the pane so reopening the dialog keeps it. No-op when closed.
    pub fn pr_review_fix_toggle_vim(&mut self) {
        let AppMode::PrReview(state) = &mut self.mode else {
            return;
        };
        let Some(confirm) = &mut state.fix_confirm else {
            return;
        };
        confirm.editor.toggle_vim();
        confirm.sync_to_cursor = true;
        let on = confirm.editor.vim_mode().is_some();
        state.fix_vim_enabled = on;
        self.message = Some(if on {
            "Vim mode enabled".into()
        } else {
            "Vim mode disabled".into()
        });
    }

    /// Scroll the fix-prompt editor by `delta` visual rows (positive = down).
    /// Clears cursor-follow so the user can scroll away from the cursor; the
    /// final clamp to content happens during rendering.
    pub fn pr_review_fix_scroll(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
        {
            confirm.scroll = confirm.scroll.saturating_add_signed(delta);
            confirm.sync_to_cursor = false;
        }
    }

    /// The vim mode of the open fix-prompt editor, or `None` when the dialog is
    /// closed or the editor is in plain (non-vim) mode. Drives `Esc` handling
    /// (vim consumes `Esc` for Insert→Normal) and the dialog's mode label.
    pub fn pr_review_fix_vim_mode(&self) -> Option<crate::editor::VimMode> {
        match &self.mode {
            AppMode::PrReview(state) => {
                state.fix_confirm.as_ref().and_then(|c| c.editor.vim_mode())
            }
            _ => None,
        }
    }

    /// Toggle GitHub resolution of the selected comment's review thread via the
    /// GraphQL `resolveReviewThread` / `unresolveReviewThread` mutation. Only
    /// inline comments that belong to a thread can be resolved; conversation
    /// comments and review summaries have no thread, so this is a no-op with a
    /// hint. Independent of replying (the user may resolve without commenting).
    ///
    /// On success the new state is applied to every comment in that thread and
    /// the SQLite cache is refreshed so a later cache-hit re-open reflects it.
    /// Zero agent tokens.
    pub fn pr_review_toggle_resolve(&mut self) {
        let info = match &self.mode {
            AppMode::PrReview(state) => state
                .selected_comment()
                .map(|c| (state.workdir.clone(), c.thread_id.clone(), c.is_resolved)),
            _ => return,
        };
        let Some((workdir, thread_id, is_resolved)) = info else {
            self.message = Some("No comment selected".into());
            return;
        };
        let Some(thread_id) = thread_id else {
            self.message = Some("This comment has no resolvable review thread".into());
            return;
        };

        let desired = !is_resolved;
        let now_resolved = match GhCli::set_thread_resolved(&workdir, &thread_id, desired) {
            Ok(state) => state,
            Err(e) => {
                self.show_error(e);
                return;
            }
        };

        if let AppMode::PrReview(state) = &mut self.mode {
            for c in &mut state.review.comments {
                if c.thread_id.as_deref() == Some(thread_id.as_str()) {
                    c.is_resolved = now_resolved;
                }
            }
        }
        self.recache_current_review();
        let msg = if now_resolved {
            "Thread resolved"
        } else {
            "Thread reopened"
        };
        self.push_toast_success(msg.to_string());
    }

    /// The combined-batch context for `comment_id` in `pr_number`, if that
    /// comment was resolved as part of a `B` batch: its `batch_id`'s sibling
    /// count. `None` for a single-comment fix, a pre-`MIGRATION_032` row, or
    /// when there is no DB. Cheap enough to call on the reply path.
    pub(super) fn combined_batch_for(
        &self,
        pr_number: u32,
        comment_id: u64,
    ) -> Option<crate::app::fix_cost::CombinedBatch> {
        let db = self.db.as_ref()?;
        let batch_id = db
            .load_pr_comment_triage(pr_number)
            .ok()?
            .get(&comment_id)?
            .batch_id
            .clone()?;
        let siblings = db
            .pr_comment_triage_batch_siblings(pr_number, &batch_id)
            .ok()?;
        (!siblings.is_empty()).then_some(crate::app::fix_cost::CombinedBatch {
            sibling_count: siblings.len(),
        })
    }

    pub fn pr_review_scroll_detail_up(&mut self, amount: usize) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.detail_scroll = state.detail_scroll.saturating_sub(amount);
        }
    }

    pub fn pr_review_scroll_detail_down(&mut self, amount: usize) {
        if let AppMode::PrReview(state) = &mut self.mode {
            // The renderer records how many lines it last drew; clamp against
            // that so scrolling can't run past the rendered detail content.
            let max_scroll = state.detail_content_lines.saturating_sub(1);
            state.detail_scroll = (state.detail_scroll + amount).min(max_scroll);
        }
    }
}
