use super::{
    PrComment, ReplyDraftProvenance, ReplyDraftRequest, ReplyGenerationMetadata, ReplyKind,
    ReplyTarget, TriageState, append_reply_attribution, reply_effective_agent_drafted,
};
use crate::app::{App, AppMode, Feature, ReplyKindPickState, ReplyState, SessionKind};
use crate::editor::TextEditor;
use crate::github::GhCli;
use anyhow::Result;
use std::path::Path;

/// Short HEAD commit hash of `workdir`, used as the last-resort seed for a
/// "Done in `<sha>`." reply. `None` when the directory isn't a git repo or has
/// no commits yet.
pub(super) fn latest_commit_short_sha(workdir: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Short hash of the most recent commit that touched `path` at `line`, via
/// `git log -L` (line-history search). `None` when the line has no history
/// (e.g. it predates the repo, or the lookup fails for any reason — an
/// outdated/shifted line number, a rename `git log` didn't follow, etc.); the
/// caller falls back to a file-level or bare-HEAD search.
pub(super) fn commit_touching_line(workdir: &Path, path: &str, line: u32) -> Option<String> {
    let output = std::process::Command::new("git")
        .args([
            "log",
            "-L",
            &format!("{line},{line}:{path}"),
            "-1",
            "--format=%h",
            "--no-patch",
        ])
        .current_dir(workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Short hash of the most recent commit that touched `path` at all — the
/// file-level fallback when a line-anchored search isn't applicable (a
/// file-level comment) or comes up empty.
pub(super) fn commit_touching_file(workdir: &Path, path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%h", "--", path])
        .current_dir(workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Best-effort commit for a "Done in `<sha>`" reply: search history for a
/// commit that plausibly addressed `comment` before falling back to bare
/// `HEAD`. Returns the sha alongside whether it's a confident match (the
/// caller adds a "(latest commit)" caveat when it isn't).
///
/// Order: line history (skipped for an outdated anchor, since the line number
/// no longer corresponds to the comment's original line) → file history →
/// bare HEAD.
pub(super) fn commit_for_done_reply(workdir: &Path, comment: &PrComment) -> (Option<String>, bool) {
    if let Some(path) = &comment.path {
        if !comment.outdated
            && let Some(line) = comment.line
            && let Some(sha) = commit_touching_line(workdir, path, line)
        {
            return (Some(sha), true);
        }
        if let Some(sha) = commit_touching_file(workdir, path) {
            return (Some(sha), true);
        }
    }
    (latest_commit_short_sha(workdir), false)
}

/// Whether `ancestor` is reachable from `descendant` — guards against citing a
/// commit from `{base}..HEAD` when `HEAD` isn't actually a descendant of the
/// recorded PR head (branch switched, force-push, rebase, or a triage session
/// on a different feature), where that range would otherwise silently return
/// unrelated history instead of nothing.
pub(super) fn is_ancestor(workdir: &Path, ancestor: &str, descendant: &str) -> bool {
    std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(workdir)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Most recent commit after the fix injection's recorded PR head that touched
/// the selected comment's file. Unlike line history, this also finds fixes that
/// insert a guard beside an unchanged commented line. If the base is unknown,
/// `HEAD` isn't a descendant of it, no later commit exists, or no later commit
/// touched an inline comment's file, return None rather than citing an older
/// or unrelated commit.
///
/// Conversation-level comments (no `path`) have no file to check "touched-ness"
/// against, so they always return `None` here rather than citing whatever
/// commit happens to be newest.
pub(super) fn commit_after_fix_request(
    workdir: &Path,
    comment: &PrComment,
    base_head_sha: &str,
) -> Option<String> {
    let path = comment.path.as_ref()?;
    let base_head_sha = base_head_sha.trim();
    if base_head_sha.is_empty() || !is_ancestor(workdir, base_head_sha, "HEAD") {
        return None;
    }
    let range = format!("{base_head_sha}..HEAD");
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%h", &range, "--", path])
        .current_dir(workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

impl App {
    /// Make this injection's reply-draft request ids authoritative and clear
    /// any previous draft for the same comments. A write failure is non-fatal:
    /// the fix still runs and `R` falls back to its existing deterministic seed.
    ///
    /// `provenance` pins the session the fix is being delivered to, so the
    /// disclosure posted with the returned draft names that session rather than
    /// whatever the pane's fix target resolves to later. Serializing it is
    /// best-effort for the same reason the write is: a draft with no provenance
    /// reports unavailable metadata, which is worse than the truth but not
    /// wrong.
    pub(super) fn begin_reply_draft_requests(
        &mut self,
        pr_number: u32,
        requests: &[ReplyDraftRequest],
        provenance: Option<&ReplyDraftProvenance>,
    ) {
        let provenance = provenance.and_then(|provenance| {
            serde_json::to_string(provenance)
                .inspect_err(|error| {
                    self.log_warn(
                        "pr_review",
                        format!("reply-draft provenance encode failed: {error}"),
                    );
                })
                .ok()
        });
        for request in requests {
            let result = match self.db.as_ref() {
                Some(db) => db.begin_pr_comment_reply_draft(
                    pr_number,
                    request.comment_id,
                    &request.request_id,
                    &request.base_head_sha,
                    provenance.as_deref(),
                ),
                None => return,
            };
            if let Err(error) = result {
                self.log_warn(
                    "pr_review",
                    format!(
                        "reply-draft request persist failed for comment {}: {error}",
                        request.comment_id
                    ),
                );
            }
        }
    }

    pub(super) fn load_reply_draft(
        &mut self,
        pr_number: u32,
        comment_id: u64,
    ) -> Option<crate::db::pr_comment_triage::ReplyDraftRow> {
        let result = self
            .db
            .as_ref()?
            .load_pr_comment_reply_draft_row(pr_number, comment_id);
        match result {
            Ok(draft) => draft.filter(|draft| !draft.body.trim().is_empty()),
            Err(error) => {
                self.log_warn("pr_review", format!("reply-draft load failed: {error}"));
                None
            }
        }
    }

    /// Decode a draft's stored provenance. A row written before provenance was
    /// recorded, or one whose blob no longer parses, yields `None` — the reply
    /// then discloses AI authorship with the details marked unavailable rather
    /// than borrowing another session's.
    pub(super) fn decode_reply_draft_provenance(
        &mut self,
        raw: Option<&str>,
    ) -> Option<ReplyDraftProvenance> {
        let raw = raw?;
        match serde_json::from_str(raw) {
            Ok(provenance) => Some(provenance),
            Err(error) => {
                self.log_warn(
                    "pr_review",
                    format!("reply-draft provenance decode failed: {error}"),
                );
                None
            }
        }
    }

    pub(super) fn clear_reply_draft(&mut self, pr_number: u32, comment_id: u64) {
        let result = match self.db.as_ref() {
            Some(db) => db.clear_pr_comment_reply_draft(pr_number, comment_id),
            None => return,
        };
        if let Err(error) = result {
            self.log_warn("pr_review", format!("reply-draft clear failed: {error}"));
        }
    }

    /// Open the reply-kind picker (`R`): a two-row choice between a "Done in
    /// `<sha>`" report and a "not needed" explanation, shown before the
    /// actual reply dialog. Replaces the old separate `R`/`n` top-level keys
    /// with one entry point. No-op (with a hint) if nothing is selected or
    /// another dialog is already open.
    pub fn pr_review_open_reply_pick(&mut self) {
        let selected_actionable = match &self.mode {
            AppMode::PrReview(state)
                if state.reply.is_none()
                    && state.fix_confirm.is_none()
                    && state.reply_kind_pick.is_none() =>
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
            state.reply_kind_pick = Some(ReplyKindPickState { selected: 0 });
        }
    }

    /// Whether the reply-kind picker is currently open over PR Triage.
    pub fn pr_review_reply_pick_picking(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state) if state.reply_kind_pick.is_some()
        )
    }

    /// Move the reply-kind-picker highlight (`+1`/`-1`, wrapping).
    pub fn pr_review_reply_pick_move(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(pick) = &mut state.reply_kind_pick
        {
            let len = ReplyKind::ALL.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Confirm the reply-kind picker: close it and open the corresponding
    /// reply dialog (reusing the existing `Done`/`NotNeeded` flows as-is).
    pub fn pr_review_reply_pick_confirm(&mut self) {
        let chosen = match &self.mode {
            AppMode::PrReview(state) => state
                .reply_kind_pick
                .as_ref()
                .map(|pick| ReplyKind::ALL[pick.selected]),
            _ => return,
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            state.reply_kind_pick = None;
        }
        match chosen {
            Some(ReplyKind::Done) => self.pr_review_open_reply_done(),
            Some(ReplyKind::NotNeeded) => self.pr_review_open_reply_not_needed(),
            // The `R` picker only lists `ReplyKind::ALL`; an investigation
            // reply is reached through the `a` action menu instead.
            Some(ReplyKind::Investigation) | None => {}
        }
    }

    /// Cancel the reply-kind picker without choosing.
    pub fn pr_review_reply_pick_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.reply_kind_pick = None;
        }
    }

    /// Open a completed-fix reply for the selected comment. A draft returned by
    /// the fixing agent takes priority; otherwise seed `Done in <sha>` from the
    /// most recent commit that plausibly touched the comment's file/line,
    /// falling back to bare `HEAD` (flagged "latest commit"). Editable before
    /// posting; posting marks the comment `Done`. Reached via the reply-kind
    /// picker (`R`), or called directly by tests/internal flows.
    pub fn pr_review_open_reply_done(&mut self) {
        let (workdir, comment) = match &self.mode {
            AppMode::PrReview(state) if state.reply.is_none() && state.fix_confirm.is_none() => {
                (state.workdir.clone(), state.selected_comment().cloned())
            }
            _ => return,
        };
        let seed = match comment {
            Some(comment) => match commit_for_done_reply(&workdir, &comment) {
                (Some(sha), true) => format!("Done in `{sha}`."),
                (Some(sha), false) => format!("Done in `{sha}` (latest commit)."),
                (None, _) => "Done.".to_string(),
            },
            None => String::new(),
        };
        self.open_reply(ReplyKind::Done, seed);
    }

    /// Open a **"not needed"** reply for the selected comment. A draft returned
    /// by the agent is prefilled when present; otherwise the editor starts empty
    /// for the user to explain *why* a fix isn't needed. Posting marks the
    /// comment `Skipped` and stores the explanation as its local note.
    pub fn pr_review_open_reply_not_needed(&mut self) {
        self.open_reply(ReplyKind::NotNeeded, String::new());
    }

    /// Shared entry: open the reply dialog for the selected comment with a kind
    /// and fallback body. A captured agent draft wins over that fallback. No-op
    /// if a fix/reply dialog is already open or nothing is selected.
    pub(super) fn open_reply(&mut self, kind: ReplyKind, seed: String) {
        let selected = match &self.mode {
            AppMode::PrReview(state) if state.reply.is_none() && state.fix_confirm.is_none() => {
                state.selected_comment().map(|comment| {
                    (
                        state.review.pr.number,
                        comment.id,
                        state.workdir.clone(),
                        comment.clone(),
                    )
                })
            }
            _ => return,
        };
        let Some((pr_number, comment_id, workdir, comment)) = selected else {
            self.message = Some("No comment selected".into());
            return;
        };
        if !comment.is_actionable() {
            self.message = Some("AMF follow-up replies are shown for context only".to_string());
            return;
        }
        // A captured draft is the fixing agent's description of what it
        // changed — meaningful only for a `Done` reply. NotNeeded always
        // starts from the fallback (empty, for the user to fill in) so a fix
        // description can never be posted as a "not needed" rationale.
        let draft = if matches!(kind, ReplyKind::Done) {
            self.load_reply_draft(pr_number, comment_id)
        } else {
            None
        };
        let has_draft = draft.is_some();
        // If this comment was fixed inside a combined batch, the disclosed cost
        // is the whole run's — mark it `combined` and carry the sibling count
        // into the posted reply.
        let combined_batch = self.combined_batch_for(pr_number, comment_id);
        // Read off the draft's own record of the session that wrote it, not the
        // pane's current fix target — which a re-opened triage pane has already
        // reset to the default.
        let generation_metadata = draft.as_ref().map(|draft| {
            let provenance = self.decode_reply_draft_provenance(draft.provenance.as_deref());
            match provenance {
                Some(provenance) => self.reply_generation_metadata(&provenance, combined_batch),
                None => ReplyGenerationMetadata {
                    combined_batch,
                    ..ReplyGenerationMetadata::unattributed()
                },
            }
        });
        let seed = match draft {
            Some(draft) => {
                if let Some(sha) =
                    commit_after_fix_request(&workdir, &comment, &draft.base_head_sha)
                {
                    format!("{}\n\nDone in `{sha}`.", draft.body.trim_end())
                } else {
                    draft.body
                }
            }
            None => seed,
        };
        // A captured draft is post-ready and opens in confirm view. Without one,
        // not-needed still starts in edit mode because the user must type a
        // reason; the deterministic done template remains post-ready.
        let editing = matches!(kind, ReplyKind::NotNeeded) && !has_draft;
        if let AppMode::PrReview(state) = &mut self.mode {
            state.reply = Some(ReplyState {
                comment_id,
                kind,
                editor: TextEditor::new(seed.clone()),
                editing,
                agent_drafted: has_draft,
                generation_metadata,
                original_seed: seed,
            });
        }
    }

    /// Enter edit mode so keystrokes flow to the reply editor.
    pub fn pr_review_reply_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(reply) = &mut state.reply
        {
            reply.editing = true;
        }
    }

    /// Leave edit mode, returning to the confirm view (the body is kept).
    pub fn pr_review_reply_stop_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(reply) = &mut state.reply
        {
            reply.editing = false;
        }
    }

    /// Forward a key to the open reply editor (only meaningful in edit mode).
    pub fn pr_review_reply_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(reply) = &mut state.reply
            && reply.editing
        {
            reply.editor.handle_key(key);
        }
    }

    /// Close the reply dialog without posting.
    pub fn pr_review_cancel_reply(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.reply = None;
        }
    }

    /// Reply-dialog status for the key handler: `None` when closed, else whether
    /// it is currently in edit mode.
    pub fn pr_review_reply_view(&self) -> Option<bool> {
        match &self.mode {
            AppMode::PrReview(state) => state.reply.as_ref().map(|r| r.editing),
            _ => None,
        }
    }

    /// Post the (possibly edited) reply to GitHub and close the dialog. Inline
    /// comments reply into their thread; conversation comments and review
    /// summaries post a new conversation comment. On success the comment is
    /// marked by the reply's kind — `Done` for a "done in `<sha>`" reply,
    /// `Skipped` (with the body kept as the local note) for a "not needed" one.
    /// The GitHub write runs only on the user's explicit confirm.
    pub fn pr_review_post_reply(&mut self) -> Result<()> {
        let prep = match &self.mode {
            AppMode::PrReview(state) => state.reply.as_ref().and_then(|reply| {
                let comment = state
                    .review
                    .comments
                    .iter()
                    .find(|c| c.id == reply.comment_id)?;
                Some((
                    state.workdir.clone(),
                    state.review.pr.clone(),
                    comment.reply_target(),
                    reply.kind,
                    reply.comment_id,
                    reply.editor.text().trim().to_string(),
                    reply_effective_agent_drafted(reply),
                    reply.generation_metadata.clone(),
                ))
            }),
            _ => return Ok(()),
        };
        let Some((workdir, pr, target, kind, comment_id, body, agent_drafted, generation_metadata)) =
            prep
        else {
            return Ok(());
        };

        if body.is_empty() {
            let hint = match kind {
                ReplyKind::NotNeeded => "Explain why a fix isn't needed, or esc to cancel",
                ReplyKind::Done | ReplyKind::Investigation => {
                    "Reply is empty — type something or esc to cancel"
                }
            };
            self.message = Some(hint.into());
            return Ok(());
        }

        // The posted body carries AI authorship attribution for a captured
        // agent draft and channel-only AMF attribution for a deterministic or
        // user-written reply. The local note stays unmarked because it is
        // AMF's own record, not content read back from GitHub.
        let posted_body =
            append_reply_attribution(&body, agent_drafted, generation_metadata.as_ref());
        let result = match target {
            ReplyTarget::InlineThread { root_comment_id } => GhCli::reply_to_review_comment(
                &workdir,
                &pr.owner,
                &pr.repo,
                pr.number,
                root_comment_id,
                &posted_body,
            ),
            ReplyTarget::Conversation => {
                GhCli::post_issue_comment(&workdir, &pr.owner, &pr.repo, pr.number, &posted_body)
            }
        };
        if let Err(e) = result {
            self.show_error(e);
            return Ok(());
        }

        // Apply the triage outcome for this reply kind and close the dialog.
        let (triage, note) = match kind {
            ReplyKind::Done => (TriageState::Done, None),
            ReplyKind::NotNeeded => (TriageState::Skipped, Some(body.clone())),
            // An investigation reply is informational — it does not claim the
            // comment is done or won't-fix, only that a reply went out.
            ReplyKind::Investigation => (TriageState::Replied, None),
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            if let Some(c) = state
                .review
                .comments
                .iter_mut()
                .find(|c| c.id == comment_id)
            {
                c.triage = triage;
                c.local_note = note.clone();
            }
            state.reply = None;
        }
        // `None` batch id keeps any existing combined-batch membership (sticky
        // via COALESCE) — a `Done`/`Skipped` reply must not un-batch the row.
        self.persist_triage(
            pr.number,
            &pr.head_sha,
            comment_id,
            triage,
            note.as_deref(),
            None,
        );
        // A batched comment resolving: capture the run's shared cost durably
        // now, before `clear_reply_draft` deletes the draft the live figure is
        // derived from — so every sibling and the AI Review pane can show the
        // same number afterwards.
        if let Some(meta) = &generation_metadata
            && meta.combined_batch.is_some()
            && let Some(cost) = meta.estimated_cost.clone()
        {
            self.persist_batch_fix_cost(pr.number, comment_id, &cost);
        }
        self.clear_reply_draft(pr.number, comment_id);
        // Posting can flip a thread's resolution (e.g. GitHub auto-resolves, or
        // the reviewer resolved meanwhile), so re-pull thread state to keep the
        // `✓` marker honest. Zero agent tokens — one GraphQL call.
        self.refresh_thread_resolution();
        let toast = match kind {
            ReplyKind::Done => "Posted reply · marked done",
            ReplyKind::NotNeeded => "Posted reply · marked skipped",
            ReplyKind::Investigation => "Posted reply · marked replied",
        };
        self.push_toast_success(toast.to_string());
        Ok(())
    }

    /// Record which session AMF is about to ask for a reply draft, at the one
    /// moment the answer is unambiguous: the fix injection that resolved it.
    /// `None` for a non-agent window, which cannot produce a draft anyway.
    pub(super) fn reply_draft_provenance(
        &self,
        pi: usize,
        fi: usize,
        si: usize,
    ) -> Option<ReplyDraftProvenance> {
        let feature = self.store.projects.get(pi)?.features.get(fi)?;
        let session = feature.sessions.get(si)?;
        let harness = match session.kind {
            SessionKind::Claude => "Claude",
            SessionKind::Opencode => "Opencode",
            SessionKind::Codex => "Codex",
            SessionKind::Pi => "Pi",
            _ => return None,
        };
        Some(ReplyDraftProvenance {
            harness: harness.to_string(),
            session_id: session.id.clone(),
            // Usually `None` here — a session created by this very fix has no
            // transcript to read a model out of yet. The live lookup at reply
            // time is the better source; this is what survives the session.
            model: self.session_disclosure_model(feature, session),
            usage_baseline: session.token_usage.clone(),
        })
    }

    /// Model discovery for the disclosure, following the same transcript/sidebar
    /// sources as the dashboard. Some harnesses (notably Pi) do not expose an
    /// interactive model, so `None` is a normal answer and prints as
    /// `unreported` rather than being guessed at.
    pub(super) fn session_disclosure_model(
        &self,
        feature: &Feature,
        session: &crate::project::FeatureSession,
    ) -> Option<String> {
        let source_id = session
            .token_usage_source
            .as_ref()
            .map(|source| source.id.as_str())
            .or(session.claude_session_id.as_deref());
        match session.kind {
            SessionKind::Claude => source_id.and_then(|id| {
                crate::app::claude_sessions::sidebar_metadata_for_session_id(&feature.workdir, id)
                    .ok()
                    .flatten()
                    .and_then(|metadata| metadata.model)
            }),
            SessionKind::Opencode => {
                crate::app::opencode_storage::read_sidebar_data(&feature.workdir, source_id)
                    .and_then(|sidebar| match (sidebar.provider, sidebar.model) {
                        (Some(provider), Some(model))
                            if !provider.trim().is_empty()
                                && !provider.eq_ignore_ascii_case(model.trim()) =>
                        {
                            Some(format!("{}/{}", provider.trim(), model.trim()))
                        }
                        (_, Some(model)) if !model.trim().is_empty() => {
                            Some(model.trim().to_string())
                        }
                        _ => None,
                    })
            }
            SessionKind::Codex => source_id
                .and_then(|id| self.cached_codex_session_model(&feature.workdir, id))
                .map(ToOwned::to_owned)
                .or_else(crate::codex_config::configured_model),
            SessionKind::Pi => None,
            _ => None,
        }
        .or_else(|| {
            self.sidebar_model_cache
                .get(&feature.tmux_session)
                .map(|model| {
                    model
                        .trim()
                        .strip_prefix("Model:")
                        .unwrap_or(model.trim())
                        .trim()
                        .to_string()
                })
                .filter(|model| !model.is_empty())
        })
    }

    /// Locate a session by its AMF id, anywhere in the store. Deliberately not
    /// scoped to the pane's fix target: a draft's provenance names one specific
    /// session, and it must resolve to that session or to nothing.
    pub(super) fn feature_session_by_id(
        &self,
        session_id: &str,
    ) -> Option<(&Feature, &crate::project::FeatureSession)> {
        self.store.projects.iter().find_map(|project| {
            project.features.iter().find_map(|feature| {
                feature
                    .sessions
                    .iter()
                    .find(|session| session.id == session_id)
                    .map(|session| (feature, session))
            })
        })
    }

    /// Turn a draft's persisted provenance into the disclosure posted with it.
    /// Harness comes straight from the record. Model and usage are read live
    /// off the *named* session when it still exists — the transcript it needs
    /// only appears after the fix runs — and fall back to the snapshot taken at
    /// injection time (model) or to `unavailable` (usage) once it is gone.
    /// Nothing here consults the pane's current fix target.
    pub(super) fn reply_generation_metadata(
        &self,
        provenance: &ReplyDraftProvenance,
        combined_batch: Option<crate::app::fix_cost::CombinedBatch>,
    ) -> ReplyGenerationMetadata {
        let live = self.feature_session_by_id(&provenance.session_id);
        let model = live
            .and_then(|(feature, session)| self.session_disclosure_model(feature, session))
            .or_else(|| provenance.model.clone());
        let usage = live
            .and_then(|(_, session)| session.token_usage.as_ref())
            .map(|current| match &provenance.usage_baseline {
                Some(baseline) => crate::token_tracking::token_usage_delta(current, baseline),
                None => current.clone(),
            })
            .filter(|usage| usage.total_tokens > 0);
        ReplyGenerationMetadata {
            harness: Some(provenance.harness.clone()),
            model,
            estimated_tokens: usage.as_ref().map(|usage| usage.total_tokens),
            estimated_cost: usage.as_ref().map(|usage| {
                crate::token_tracking::format_token_cost(usage, &self.config.token_pricing)
            }),
            combined_batch,
        }
    }
}
