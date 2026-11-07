use super::*;

pub(super) fn start_fix_draft(
    gui: &mut GuiHandle,
    comment_id: u64,
    session_id: String,
    feature_id: Option<String>,
    destination_revision: String,
) -> GuiResult<()> {
    let opened = gui.pr_triage_context.as_ref().unwrap().target.feature()?;
    let target = FeatureTarget {
        project_id: opened.project_id.clone(),
        feature_id: feature_id.unwrap_or_else(|| opened.feature_id.clone()),
    };
    let app = gui.app_for_workflow();
    let comment = select(app, comment_id)?;
    let state = review_mut(app)?;
    if !comment.is_actionable() {
        return Err(GuiError::conflict(
            "AMF follow-up replies are shown for context only",
        ));
    }
    if state.reply.is_some() {
        return Err(GuiError::conflict(
            "Post or discard the open reply before preparing a fix",
        ));
    }
    let prompt = state.fix_draft_prompt(&comment);
    let head_sha = state.review.pr.head_sha.clone();
    let number = state.review.pr.number;
    let destination_kind = crate::gui_pr_workspace::destination_feature_kind(
        app,
        &opened.project_id,
        &opened.feature_id,
        number,
        &head_sha,
        &target.feature_id,
    )?;
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("The feature was deleted"))?;
    let feature = &app.store.projects[pi].features[fi];
    let session = feature
        .sessions
        .iter()
        .find(|s| s.id == session_id && s.kind.is_agent_harness())
        .ok_or_else(|| {
            GuiError::not_found("Choose an existing agent session in this feature or its companion")
        })?;
    if crate::gui_pr_workspace::destination_revision(feature, session) != destination_revision {
        return Err(GuiError::conflict(
            "The agent session changed; choose it again",
        ));
    }
    let pending = PendingFixDraft {
        view: PrFixDraftView {
            comment_id,
            target: SessionTarget {
                project_id: target.project_id,
                feature_id: target.feature_id,
                session_id,
            },
            prompt,
            submission_prompt: None,
        },
        comment,
        head_sha,
        tmux_session: feature.tmux_session.clone(),
        tmux_window: session.tmux_window.clone(),
        session_kind: session.kind.clone(),
        destination_kind,
        submission: None,
    };
    gui.pr_triage_context.as_mut().unwrap().fix = Some(pending);
    Ok(())
}

pub(super) fn validate_fix_target(
    gui: &mut GuiHandle,
    reads: &mut PrTriagePrefetch,
    prompt: String,
) -> GuiResult<SessionTarget> {
    if prompt.trim().is_empty() {
        return Err(GuiError::conflict("The fix prompt is empty"));
    }
    let context = gui.pr_triage_context.as_ref().unwrap();
    let pending = context
        .fix
        .as_ref()
        .ok_or_else(|| GuiError::conflict("No fix draft is open"))?;
    let workdir = context.workdir.clone();
    let opened = context.target.feature()?;
    let target = pending.view.target.clone();
    let (head, comment, tmux_session, tmux_window, session_kind, destination_kind) = (
        pending.head_sha.clone(),
        pending.comment.clone(),
        pending.tmux_session.clone(),
        pending.tmux_window.clone(),
        pending.session_kind.clone(),
        pending.destination_kind,
    );
    let app = gui.app_for_workflow();
    let state = review_mut(app)?;
    if state.review.pr.head_sha != head
        || !state.review.comments.iter().any(|c| {
            c.id == comment.id
                && c.body == comment.body
                && c.path == comment.path
                && c.line == comment.line
                && c.diff_hunk == comment.diff_hunk
        })
    {
        return Err(GuiError::conflict(
            "The comment changed; cancel and prepare the fix again",
        ));
    }
    let number = state.review.pr.number;
    let current = reads
        .fetch_pr_by_number(&workdir, state.review.pr.number)
        .map_err(GuiError::from)?;
    if current.head_sha != head {
        return Err(GuiError::conflict(
            "The PR has new commits; cancel the fix draft and refresh",
        ));
    }
    if crate::gui_pr_workspace::destination_feature_kind(
        app,
        &opened.project_id,
        &opened.feature_id,
        number,
        &head,
        &target.feature_id,
    )? != destination_kind
    {
        return Err(GuiError::conflict(
            "The destination changed kind; cancel and choose a target again",
        ));
    }
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("The feature was deleted"))?;
    let feature = &app.store.projects[pi].features[fi];
    if feature.tmux_session != tmux_session
        || !feature.sessions.iter().any(|s| {
            s.id == target.session_id && s.tmux_window == tmux_window && s.kind == session_kind
        })
    {
        return Err(GuiError::conflict(
            "The agent session changed; cancel and choose a target again",
        ));
    }
    Ok(target)
}

pub(super) fn confirm_fix_draft(
    gui: &mut GuiHandle,
    reads: &mut PrTriagePrefetch,
    prompt: String,
) -> GuiResult<()> {
    let target = validate_fix_target(gui, reads, prompt.clone())?;
    let context = gui.pr_triage_context.as_mut().unwrap();
    context.fix = None;
    context.handoff = Some(PrFixHandoff {
        target,
        draft_prompt: prompt,
    });
    context.notice = Some("Fix prompt prepared as an unsent composer draft".into());
    Ok(())
}

pub(super) fn prepare_fix_submission(gui: &mut GuiHandle, prompt: String) -> GuiResult<()> {
    if prompt.trim().is_empty() {
        return Err(GuiError::conflict("The fix prompt is empty"));
    }
    let pending = gui
        .pr_triage_context
        .as_ref()
        .unwrap()
        .fix
        .as_ref()
        .ok_or_else(|| GuiError::conflict("No fix draft is open"))?;
    let request = ReplyDraftRequest::new(pending.comment.id, &pending.head_sha);
    let number = review_mut(gui.app_for_workflow())?.review.pr.number;
    let exact = with_reply_draft_handoff(prompt, number, std::slice::from_ref(&request));
    let pending = gui
        .pr_triage_context
        .as_mut()
        .unwrap()
        .fix
        .as_mut()
        .unwrap();
    pending.view.submission_prompt = Some(exact);
    pending.submission = Some(request);
    Ok(())
}

pub(super) fn confirm_fix_submission(
    gui: &mut GuiHandle,
    reads: &mut PrTriagePrefetch,
) -> GuiResult<()> {
    let pending = gui
        .pr_triage_context
        .as_ref()
        .unwrap()
        .fix
        .as_ref()
        .ok_or_else(|| GuiError::conflict("No fix draft is open"))?;
    let prompt = pending
        .view
        .submission_prompt
        .clone()
        .ok_or_else(|| GuiError::conflict("Preview the fix submission first"))?;
    let request = pending.submission.clone().unwrap();
    let target = validate_fix_target(gui, reads, prompt.clone())?;
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .unwrap();
    let feature = &app.store.projects[pi].features[fi];
    let si = feature
        .sessions
        .iter()
        .position(|s| s.id == target.session_id)
        .unwrap();
    let session = &feature.sessions[si];
    if session.stopped
        || !app
            .tmux
            .window_exists(&feature.tmux_session, &session.tmux_window)
    {
        return Err(GuiError::conflict(
            "The agent is stopped; start it from its session tab, then retry",
        ));
    }
    let (tmux_session, tmux_window) = (feature.tmux_session.clone(), session.tmux_window.clone());
    let number = review_mut(app)?.review.pr.number;
    let provenance = app.reply_draft_provenance(pi, fi, si);
    // Register before sending: a fast agent must be able to return its receipt.
    // Refuse a missing DB or failed write before touching the terminal.
    let encoded = provenance
        .map(|p| serde_json::to_string(&p))
        .transpose()
        .map_err(|e| GuiError::from(anyhow::Error::from(e)))?;
    let db = app
        .db
        .as_ref()
        .ok_or_else(|| GuiError::conflict("Reply-draft storage is unavailable"))?;
    let prior = db
        .snapshot_pr_comment_reply_draft(number, request.comment_id)
        .map_err(GuiError::from)?;
    db.begin_pr_comment_reply_draft(
        number,
        request.comment_id,
        &request.request_id,
        &request.base_head_sha,
        encoded.as_deref(),
    )
    .map_err(GuiError::from)?;
    let delivered = app
        .tmux
        .send_key_name(&tmux_session, &tmux_window, "C-u")
        .and_then(|()| app.tmux.paste_text(&tmux_session, &tmux_window, &prompt));
    if let Err(e) = delivered {
        // Nothing was submitted, so the reply draft an earlier fix returned
        // for this comment must survive the failed attempt.
        let restored = app.db.as_ref().unwrap().restore_pr_comment_reply_draft(
            number,
            request.comment_id,
            &request.request_id,
            prior.as_ref(),
        );
        return Err(GuiError::from(match restored {
            Ok(()) => e,
            Err(r) => e.context(format!(
                "Could not restore this comment's earlier reply draft ({r:#}); it was replaced"
            )),
        }));
    }
    // Retire confirmation before Enter: a transport failure can be ambiguous.
    // Retrying must require preparing a new explicit submission.
    let pending = gui
        .pr_triage_context
        .as_mut()
        .unwrap()
        .fix
        .as_mut()
        .unwrap();
    pending.submission = None;
    pending.view.submission_prompt = None;
    let app = gui.app_for_workflow();
    app.tmux
        .send_key_name(&tmux_session, &tmux_window, "Enter")
        .map_err(|e| GuiError::from(e.context("Could not confirm terminal delivery. Inspect the agent before preparing another fix; it may have received this prompt")))?;
    let state = review_mut(app)?;
    state
        .review
        .comments
        .iter_mut()
        .find(|c| c.id == request.comment_id)
        .unwrap()
        .triage = TriageState::Fixing;
    app.persist_triage(
        number,
        &request.base_head_sha,
        request.comment_id,
        TriageState::Fixing,
        None,
        None,
    );
    gui.pr_triage_context.as_mut().unwrap().fix = None;
    gui.pr_triage_context.as_mut().unwrap().notice = Some("Fix sent to the agent. Its reply draft will be available under Reply: fixed; posting remains explicit.".into());
    Ok(())
}

/// Hard ceiling on one batch. The TUI only warns past
/// `BATCH_COMBINED_COMMENT_WARN`; the GUI also refuses a runaway selection.
const MAX_BATCH_COMMENTS: usize = 50;

/// Digest of everything a batch fix was shown: the PR and head, each selected
/// comment as fetched (body, location, hunk, resolution and triage state),
/// the destination session and its revision. Any change makes it differ.
pub(super) fn batch_digest(
    number: u32,
    head_sha: &str,
    comments: &[&PrComment],
    target: &SessionTarget,
    destination_kind: crate::gui_pr_workspace::DestinationKind,
    destination_revision: &str,
) -> String {
    use sha2::{Digest, Sha256};
    let bytes = serde_json::to_vec(&(
        number,
        head_sha,
        comments,
        target,
        destination_kind,
        destination_revision,
    ))
    .expect("serializable batch");
    format!("{:x}", Sha256::digest(bytes))
}

/// The selected comments in review order, refusing (never silently dropping)
/// any id that is missing, resolved, local-only or an AMF follow-up reply.
pub(super) fn batch_comments<'a>(
    state: &'a PrReviewState,
    ids: &[u64],
) -> GuiResult<Vec<&'a PrComment>> {
    let mut seen = std::collections::HashSet::new();
    if ids.is_empty() || ids.iter().any(|id| !seen.insert(*id)) {
        return Err(GuiError::conflict("Choose one or more distinct comments"));
    }
    if ids.len() > MAX_BATCH_COMMENTS {
        return Err(GuiError::conflict(format!(
            "Choose at most {MAX_BATCH_COMMENTS} comments for one batch"
        )));
    }
    for id in ids {
        let comment = state
            .review
            .comments
            .iter()
            .find(|c| c.id == *id)
            .ok_or_else(|| GuiError::conflict("A selected comment is no longer in this review"))?;
        if comment.is_resolved {
            return Err(GuiError::conflict(
                "A selected comment was resolved; choose the comments again",
            ));
        }
        if !comment.is_actionable() || comment.is_local_finding() {
            return Err(GuiError::conflict(
                "Only incoming GitHub review comments can be batched",
            ));
        }
    }
    Ok(state
        .review
        .comments
        .iter()
        .filter(|c| ids.contains(&c.id))
        .collect())
}

pub(super) fn start_batch_fix(
    gui: &mut GuiHandle,
    comment_ids: Vec<u64>,
    session_id: String,
    destination_revision: String,
    feature_id: Option<String>,
) -> GuiResult<()> {
    let opened = gui.pr_triage_context.as_ref().unwrap().target.feature()?;
    let target = FeatureTarget {
        project_id: opened.project_id.clone(),
        feature_id: feature_id.unwrap_or_else(|| opened.feature_id.clone()),
    };
    let app = gui.app_for_workflow();
    let state = review_mut(app)?;
    if state.reply.is_some() {
        return Err(GuiError::conflict(
            "Post or discard the open reply before preparing a fix",
        ));
    }
    let selected = batch_comments(state, &comment_ids)?;
    let mut prompt = crate::app::pr_review::combined_fix_prompt(&selected, &state.review.comments);
    let appendix: String = selected
        .iter()
        .enumerate()
        .filter_map(|(i, c)| {
            state
                .investigations
                .iter()
                .find(|r| r.comment_id == c.id)
                .and_then(investigation_findings_for_prompt)
                .map(|f| format!("\n\nInvestigation already run for comment {}:\n{f}", i + 1))
        })
        .collect();
    if !appendix.is_empty() {
        prompt.push_str(
            "\n\n--- Read-only investigations already ran for some of these \
             comments. Use them as starting points, but verify: ---",
        );
        prompt.push_str(&appendix);
    }
    let ordered: Vec<u64> = selected.iter().map(|c| c.id).collect();
    let count = ordered.len();
    let tokens = crate::app::pr_review::estimate_tokens(&prompt);
    let warning = (count > crate::app::pr_review::BATCH_COMBINED_COMMENT_WARN
        || tokens > crate::app::pr_review::BATCH_COMBINED_TOKEN_WARN)
        .then(|| {
            format!(
                "Large batch: {count} comments (~{tokens} tokens) in one prompt. It may exceed the agent's context window."
            )
        });
    let head_sha = state.review.pr.head_sha.clone();
    let number = state.review.pr.number;
    let destination_kind = crate::gui_pr_workspace::destination_feature_kind(
        app,
        &opened.project_id,
        &opened.feature_id,
        number,
        &head_sha,
        &target.feature_id,
    )?;
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("The feature was deleted"))?;
    let feature = &app.store.projects[pi].features[fi];
    // The destination is exactly the session the user chose, at the revision
    // they saw. It is never replaced by another session.
    let session = feature
        .sessions
        .iter()
        .find(|s| s.id == session_id && s.kind.is_agent_harness())
        .ok_or_else(|| {
            GuiError::not_found("Choose an existing agent session in this feature or its companion")
        })?;
    if crate::gui_pr_workspace::destination_revision(feature, session) != destination_revision {
        return Err(GuiError::conflict(
            "The agent session changed; choose it again",
        ));
    }
    let target = SessionTarget {
        project_id: target.project_id,
        feature_id: target.feature_id,
        session_id,
    };
    let state = review_state(&app.mode).unwrap();
    let selected = batch_comments(state, &ordered)?;
    let digest = batch_digest(
        number,
        &head_sha,
        &selected,
        &target,
        destination_kind,
        &destination_revision,
    );
    gui.pr_triage_context.as_mut().unwrap().batch = Some(PendingBatchFix {
        view: PrBatchFixView {
            comment_ids: ordered,
            target,
            destination_revision,
            destination_kind,
            digest,
            prompt,
            submission_prompt: None,
            warning,
        },
        head_sha,
        submission: None,
    });
    Ok(())
}

/// Everything that can be checked without GitHub: the digest of the live
/// comments, head and destination session still equals the one previewed.
pub(super) fn check_batch_local(
    gui: &mut GuiHandle,
    digest: &str,
) -> GuiResult<(SessionTarget, Vec<u64>)> {
    let pending = gui
        .pr_triage_context
        .as_ref()
        .unwrap()
        .batch
        .as_ref()
        .ok_or_else(|| GuiError::conflict("No batch fix is open"))?;
    if pending.view.digest != digest {
        return Err(GuiError::conflict(
            "This is not the batch you reviewed; cancel and prepare it again",
        ));
    }
    let (view, head) = (pending.view.clone(), pending.head_sha.clone());
    let opened = gui.pr_triage_context.as_ref().unwrap().target.feature()?;
    let app = gui.app_for_workflow();
    let number = review_mut(app)?.review.pr.number;
    let live_kind = crate::gui_pr_workspace::destination_feature_kind(
        app,
        &opened.project_id,
        &opened.feature_id,
        number,
        &head,
        &view.target.feature_id,
    )?;
    if live_kind != view.destination_kind {
        return Err(GuiError::conflict(
            "The destination changed kind; cancel and choose a target again",
        ));
    }
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&view.target.project_id), &view.target.feature_id)
        .ok_or_else(|| GuiError::not_found("The feature was deleted"))?;
    let feature = &app.store.projects[pi].features[fi];
    let session = feature
        .sessions
        .iter()
        .find(|s| s.id == view.target.session_id && s.kind.is_agent_harness())
        .ok_or_else(|| {
            GuiError::conflict("The agent session is gone; cancel and choose a target again")
        })?;
    let revision = crate::gui_pr_workspace::destination_revision(feature, session);
    if revision != view.destination_revision {
        return Err(GuiError::conflict(
            "The agent session changed; cancel and choose a target again",
        ));
    }
    let state = review_mut(app)?;
    if state.review.pr.head_sha != head {
        return Err(GuiError::conflict(
            "The PR has new commits; cancel the batch fix and refresh",
        ));
    }
    let selected = batch_comments(state, &view.comment_ids)?;
    if batch_digest(number, &head, &selected, &view.target, live_kind, &revision) != view.digest {
        return Err(GuiError::conflict(
            "The selected comments changed; cancel and prepare the batch again",
        ));
    }
    Ok((view.target, view.comment_ids))
}

pub(super) fn prepare_batch_submission(
    gui: &mut GuiHandle,
    prompt: String,
    digest: String,
) -> GuiResult<()> {
    if prompt.trim().is_empty() {
        return Err(GuiError::conflict("The fix prompt is empty"));
    }
    let (_, ids) = check_batch_local(gui, &digest)?;
    let head = gui
        .pr_triage_context
        .as_ref()
        .unwrap()
        .batch
        .as_ref()
        .unwrap()
        .head_sha
        .clone();
    let requests: Vec<ReplyDraftRequest> = ids
        .iter()
        .map(|id| ReplyDraftRequest::new(*id, &head))
        .collect();
    let number = review_mut(gui.app_for_workflow())?.review.pr.number;
    let exact = with_reply_draft_handoff(prompt, number, &requests);
    let pending = gui
        .pr_triage_context
        .as_mut()
        .unwrap()
        .batch
        .as_mut()
        .unwrap();
    pending.view.submission_prompt = Some(exact);
    pending.submission = Some(requests);
    Ok(())
}

/// Put back every reply-draft row a failed batch replaced, newest first.
/// Returns a note about any row that could not be restored.
pub(super) fn restore_reply_drafts(
    app: &App,
    number: u32,
    registered: &[(
        ReplyDraftRequest,
        Option<crate::db::pr_comment_triage::ReplyDraftSnapshot>,
    )],
) -> Option<String> {
    let db = app.db.as_ref()?;
    let failures: Vec<String> = registered
        .iter()
        .rev()
        .filter_map(|(request, prior)| {
            db.restore_pr_comment_reply_draft(
                number,
                request.comment_id,
                &request.request_id,
                prior.as_ref(),
            )
            .err()
            .map(|e| format!("comment {}: {e:#}", request.comment_id))
        })
        .collect();
    (!failures.is_empty()).then(|| {
        format!(
            "Could not restore earlier reply drafts ({}); they were replaced",
            failures.join("; ")
        )
    })
}

pub(super) fn confirm_batch_submission(
    gui: &mut GuiHandle,
    reads: &mut PrTriagePrefetch,
    digest: String,
) -> GuiResult<()> {
    let pending = gui
        .pr_triage_context
        .as_ref()
        .unwrap()
        .batch
        .as_ref()
        .ok_or_else(|| GuiError::conflict("No batch fix is open"))?;
    let prompt = pending
        .view
        .submission_prompt
        .clone()
        .ok_or_else(|| GuiError::conflict("Preview the batch submission first"))?;
    let requests = pending.submission.clone().unwrap();
    let (head, workdir) = (
        pending.head_sha.clone(),
        gui.pr_triage_context.as_ref().unwrap().workdir.clone(),
    );
    let (target, ids) = check_batch_local(gui, &digest)?;
    let app = gui.app_for_workflow();
    let number = review_mut(app)?.review.pr.number;
    let current = reads
        .fetch_pr_by_number(&workdir, number)
        .map_err(GuiError::from)?;
    if current.head_sha != head {
        return Err(GuiError::conflict(
            "The PR has new commits; cancel the batch fix and refresh",
        ));
    }
    let app = gui.app_for_workflow();
    let (pi, fi) = app
        .store
        .locate_feature_by_id(Some(&target.project_id), &target.feature_id)
        .ok_or_else(|| GuiError::not_found("The feature was deleted"))?;
    let feature = &app.store.projects[pi].features[fi];
    let si = feature
        .sessions
        .iter()
        .position(|s| s.id == target.session_id)
        .ok_or_else(|| GuiError::conflict("The agent session is gone"))?;
    let session = &feature.sessions[si];
    if session.stopped
        || !app
            .tmux
            .window_exists(&feature.tmux_session, &session.tmux_window)
    {
        return Err(GuiError::conflict(
            "The agent is stopped; start it from its session tab, then retry",
        ));
    }
    let (tmux_session, tmux_window) = (feature.tmux_session.clone(), session.tmux_window.clone());
    let provenance = app.reply_draft_provenance(pi, fi, si);
    let encoded = provenance
        .map(|p| serde_json::to_string(&p))
        .transpose()
        .map_err(|e| GuiError::from(anyhow::Error::from(e)))?;
    if app.db.is_none() {
        return Err(GuiError::conflict("Reply-draft storage is unavailable"));
    }
    // Register every comment before sending: a fast agent must be able to
    // return its receipts. A failure on any one restores all earlier ones and
    // nothing reaches the terminal.
    let mut registered = Vec::new();
    for request in &requests {
        let db = app.db.as_ref().unwrap();
        let step = db
            .snapshot_pr_comment_reply_draft(number, request.comment_id)
            .and_then(|prior| {
                db.begin_pr_comment_reply_draft(
                    number,
                    request.comment_id,
                    &request.request_id,
                    &request.base_head_sha,
                    encoded.as_deref(),
                )
                .map(|()| prior)
            });
        match step {
            Ok(prior) => registered.push((request.clone(), prior)),
            Err(e) => {
                let note = restore_reply_drafts(app, number, &registered);
                return Err(GuiError::from(match note {
                    None => e.context(format!(
                        "Could not register the reply draft for comment {}; nothing was sent",
                        request.comment_id
                    )),
                    Some(n) => e.context(n),
                }));
            }
        }
    }
    let delivered = app
        .tmux
        .send_key_name(&tmux_session, &tmux_window, "C-u")
        .and_then(|()| app.tmux.paste_text(&tmux_session, &tmux_window, &prompt));
    if let Err(e) = delivered {
        let note = restore_reply_drafts(app, number, &registered);
        return Err(GuiError::from(match note {
            None => e,
            Some(n) => e.context(n),
        }));
    }
    // Retire confirmation before Enter: a transport failure can be ambiguous,
    // and retrying must need a fresh explicit preview.
    let pending = gui
        .pr_triage_context
        .as_mut()
        .unwrap()
        .batch
        .as_mut()
        .unwrap();
    pending.submission = None;
    pending.view.submission_prompt = None;
    let app = gui.app_for_workflow();
    app.tmux
        .send_key_name(&tmux_session, &tmux_window, "Enter")
        .map_err(|e| GuiError::from(e.context("Could not confirm terminal delivery. Inspect the agent before preparing another fix; it may have received this prompt")))?;
    let batch_id = uuid::Uuid::new_v4().to_string();
    for id in &ids {
        // The review state was resolved above (`number`) and nothing since
        // can remove it; never fail after the prompt has been delivered.
        if let Some(c) = review_mut(app)
            .ok()
            .and_then(|state| state.review.comments.iter_mut().find(|c| c.id == *id))
        {
            c.triage = TriageState::Fixing;
            c.batch_id = Some(batch_id.clone());
        }
        app.persist_triage(
            number,
            &head,
            *id,
            TriageState::Fixing,
            None,
            Some(&batch_id),
        );
    }
    let context = gui.pr_triage_context.as_mut().unwrap();
    context.batch = None;
    context.notice = Some(format!(
        "Fix for {} comments sent to the agent. Reply drafts will be available under Reply: fixed; posting remains explicit.",
        ids.len()
    ));
    Ok(())
}
