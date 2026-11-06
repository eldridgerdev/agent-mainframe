use super::*;

pub(super) fn investigation_scope_todos(
    app: &App,
    state: &PrReviewState,
) -> Vec<crate::db::todos::Todo> {
    use crate::db::todos::TodoScope;
    let Some(db) = app.db.as_ref() else {
        return Vec::new();
    };
    let Some((pi, fi)) = app.pr_review_feature_for_target(state) else {
        return Vec::new();
    };
    let mut scopes = Vec::new();
    if let Some(scope) = app.worktree_todo_scope(pi, fi) {
        scopes.push(scope);
    }
    if let Some(project) = app.store.projects.get(pi) {
        scopes.push(TodoScope::Project {
            project_id: project.id.clone(),
        });
    }
    scopes.push(TodoScope::Global);
    scopes
        .iter()
        .filter_map(|scope| db.todo_list(scope).ok().flatten())
        .flat_map(|list| db.todos(&list.id).unwrap_or_default())
        .collect()
}

/// From `todos`, the one that came from this comment's investigation answer
/// and, separately, one from an older answer to the same comment:
/// `(exact answer, an older answer)`.
pub(super) fn find_investigation_todos(
    todos: &[crate::db::todos::Todo],
    pr_number: u32,
    comment_id: u64,
    answer: &str,
) -> (
    Option<crate::db::todos::Todo>,
    Option<crate::db::todos::Todo>,
) {
    let marker = todo_source_marker(pr_number, comment_id, answer);
    let prefix = todo_source_prefix(pr_number, comment_id);
    let mut older = None;
    for todo in todos {
        let Some(body) = todo.body.as_deref() else {
            continue;
        };
        if body.contains(&marker) {
            return (Some(todo.clone()), older);
        }
        if older.is_none() && body.contains(&prefix) {
            older = Some(todo.clone());
        }
    }
    (None, older)
}

/// Keep a finished investigation answer as a TODO on the PR's feature (its
/// worktree list, or the project's at the repo root), as the TUI does, with a
/// source marker in the notes. Keeping it again does not write a second item,
/// even if the earlier one was moved to another scope.
pub(super) fn keep_investigation_as_todo(
    gui: &mut GuiHandle,
    comment_id: u64,
    head_sha: &str,
) -> GuiResult<()> {
    let app = gui.app_for_workflow();
    if app.db.is_none() {
        return Err(GuiError::conflict(
            "AMF can't reach its database, so there's no TODO list to add to.",
        ));
    }
    let comment = select(app, comment_id)?;
    let state = review_mut(app)?;
    if state.review.pr.head_sha != head_sha {
        return Err(GuiError::conflict(
            "The pull request head changed since you opened this answer; refresh and review it before keeping it",
        ));
    }
    let inv = state
        .investigations
        .iter()
        .find(|r| r.comment_id == comment_id)
        .ok_or_else(|| GuiError::conflict("This comment has no investigation to keep"))?;
    if !matches!(
        inv.status,
        PrInvestigationStatus::Complete | PrInvestigationStatus::Dismissed
    ) {
        return Err(GuiError::conflict(
            "Wait for the investigation to finish before keeping it as a TODO",
        ));
    }
    let answer = inv
        .answer
        .clone()
        .filter(|a| !a.trim().is_empty())
        .ok_or_else(|| GuiError::conflict("This investigation has no answer to keep"))?;
    let inv_head = inv.head_sha.clone();
    let pr_number = state.review.pr.number;
    // The seed carries the source marker, so the TUI and GUI dedupe alike.
    let (title, body) = investigation_todo_seed(pr_number, &comment, Some(&answer));
    let Some((pi, fi)) = app.pr_review_target_feature() else {
        return Err(GuiError::conflict(
            "Can't keep as TODO: no feature for this PR",
        ));
    };
    let Some(scope) = app.default_todo_scope(pi, fi) else {
        return Err(GuiError::conflict("Couldn't resolve a TODO scope"));
    };
    let label = app.todo_scope_label(&scope);
    let (exact, older) = match &app.mode {
        AppMode::PrReview(state) => find_investigation_todos(
            &investigation_scope_todos(app, state),
            pr_number,
            comment_id,
            &answer,
        ),
        _ => (None, None),
    };
    let notice = if let Some(todo) = exact {
        format!("Already kept as a TODO: {}", todo.title)
    } else {
        let has_session = app.store.projects[pi].features[fi].has_todos_session();
        if !has_session {
            if let Err(e) = app.add_todos_session_for_picker(pi, fi, None) {
                return Err(GuiError::conflict(format!(
                    "Couldn't start a TODO list; nothing was written: {e}"
                )));
            }
            // add_todos_session_for_picker leaves its own status message behind.
            app.message = None;
        }
        let feature_id = app.store.projects[pi].features[fi].id.clone();
        let head_note = if inv_head.is_empty() {
            String::new()
        } else {
            format!(
                " Investigated at PR head {}.",
                &inv_head[..inv_head.len().min(12)]
            )
        };
        let body = format!("{body}{head_note}");
        let db = app.db.as_ref().expect("checked above");
        db.load_or_create_todo_list(&scope, Some(&feature_id))
            .and_then(|list| {
                db.add_todo(
                    &list.id,
                    &title,
                    Some(&body),
                    crate::db::todos::TodoPriority::Med,
                )
            })
            .map_err(|e| GuiError::conflict(format!("Couldn't add the TODO: {e}")))?;
        match older {
            Some(old) => format!(
                "Kept as a TODO on {label}. An earlier TODO for an older answer to this comment (\"{}\") was left in place.",
                old.title
            ),
            None => format!("Kept as a TODO on {label}"),
        }
    };
    gui.pr_triage_context.as_mut().unwrap().notice = Some(notice);
    Ok(())
}

pub(super) fn set_feedback(gui: &mut GuiHandle, toasts: usize) {
    let (error, notice) = take_feedback(gui.app_for_workflow(), toasts);
    let context = gui.pr_triage_context.as_mut().unwrap();
    if notice.is_some() {
        context.notice = notice;
    }
    if error.is_some() {
        context.error = error;
    }
}

pub(super) fn request_investigation(
    gui: &mut GuiHandle,
    reads: &mut PrTriagePrefetch,
    comment_id: u64,
    harness: AgentKind,
    note: Option<String>,
    follow_up: Option<String>,
) -> GuiResult<()> {
    let workdir = gui.pr_triage_context.as_ref().unwrap().workdir.clone();
    let app = gui.app_for_workflow();
    if app.pr_review_work.investigation_pending() {
        return Err(GuiError::conflict("An investigation is already running"));
    }
    if !harnesses(app, &workdir).contains(&harness) {
        return Err(GuiError::conflict(format!(
            "{} is not allowed for this project",
            harness.display_name()
        )));
    }
    let comment = select(app, comment_id)?;
    if !comment.is_actionable() {
        return Err(GuiError::conflict(
            "AMF follow-up replies cannot be investigated",
        ));
    }
    let note = note
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty() && follow_up.is_none());
    if note
        .as_ref()
        .is_some_and(|n| n.chars().count() > INVESTIGATION_CONTEXT_MAX_LEN)
    {
        return Err(GuiError::conflict(format!(
            "Keep the investigation note under {INVESTIGATION_CONTEXT_MAX_LEN} characters"
        )));
    }
    let follow_up = follow_up.map(|q| q.trim().to_string());
    if follow_up.as_ref().is_some_and(String::is_empty) {
        return Err(GuiError::conflict("Type a follow-up question first"));
    }
    let prompt = build_prompt(
        app,
        reads,
        &workdir,
        &comment,
        follow_up.as_deref(),
        note.as_deref(),
    )?;
    let head_sha = review_mut(app)?.review.pr.head_sha.clone();
    gui.pr_triage_context.as_mut().unwrap().investigation = Some(PendingInvestigation {
        comment_id,
        harness,
        follow_up,
        note,
        head_sha,
        prompt,
        viewing: false,
    });
    Ok(())
}

/// Build the prompt the run will send, from a fresh read of the PR.
pub(super) fn build_prompt(
    app: &mut App,
    reads: &mut PrTriagePrefetch,
    workdir: &Path,
    comment: &PrComment,
    follow_up: Option<&str>,
    note: Option<&str>,
) -> GuiResult<String> {
    let state = review_mut(app)?;
    let number = state.review.pr.number;
    let prior = match follow_up {
        None => None,
        Some(question) => {
            let row = state
                .investigations
                .iter()
                .find(|r| r.comment_id == comment.id)
                .filter(|r| {
                    r.status == PrInvestigationStatus::Complete
                        || r.status == PrInvestigationStatus::Dismissed
                })
                .filter(|r| r.answer.as_deref().is_some_and(|a| !a.trim().is_empty()))
                .ok_or_else(|| GuiError::conflict("No completed investigation to follow up on"))?;
            Some((
                row.answer.clone().unwrap_or_default(),
                row.follow_ups.clone(),
                question.to_string(),
            ))
        }
    };
    let meta = reads
        .pr_meta(workdir, number)
        .map_err(|e| GuiError::from(e.context(format!("Couldn't load PR #{number}"))))?;
    Ok(investigation_prompt_from_meta(
        comment,
        number,
        &meta,
        prior.as_ref().map(|(answer, turns, question)| {
            (answer.as_str(), turns.as_slice(), question.as_str())
        }),
        note,
    ))
}

pub(super) fn confirm_investigation(
    gui: &mut GuiHandle,
    reads: &mut PrTriagePrefetch,
) -> GuiResult<()> {
    let workdir = gui.pr_triage_context.as_ref().unwrap().workdir.clone();
    let pending = gui
        .pr_triage_context
        .as_mut()
        .unwrap()
        .investigation
        .take()
        .ok_or_else(|| GuiError::conflict("No AI call is waiting"))?;
    let app = gui.app_for_workflow();
    if app.pr_review_work.investigation_pending() {
        gui.pr_triage_context.as_mut().unwrap().investigation = Some(pending);
        return Err(GuiError::conflict("An investigation is already running"));
    }
    let comment = select(app, pending.comment_id)?;
    if review_mut(app)?.review.pr.head_sha != pending.head_sha {
        return Err(GuiError::conflict(
            "The pull request was refreshed after the preview; start the investigation again",
        ));
    }
    // The PR's description or file list may have changed on GitHub since the
    // preview. Never send a prompt the operator has not seen.
    let prompt = match build_prompt(
        app,
        reads,
        &workdir,
        &comment,
        pending.follow_up.as_deref(),
        pending.note.as_deref(),
    ) {
        Ok(prompt) => prompt,
        Err(e) => {
            // A failed re-read changes nothing: keep the notice to retry.
            gui.pr_triage_context.as_mut().unwrap().investigation = Some(pending);
            return Err(e);
        }
    };
    if prompt != pending.prompt {
        gui.pr_triage_context.as_mut().unwrap().investigation = Some(PendingInvestigation {
            prompt,
            viewing: true,
            ..pending
        });
        return Err(GuiError::conflict(
            "The pull request changed after the preview; review the updated prompt, then continue",
        ));
    }
    let follow_up = pending.follow_up.map(|question| PendingFollowUp {
        comment_id: pending.comment_id,
        question,
    });
    let toasts = app.toasts.len();
    app.pr_review_launch_investigation(pending.harness, follow_up, Some(pending.prompt));
    let started = matches!(app.mode, AppMode::PrInvestigationLoading(_));
    let (error, _) = take_feedback(app, toasts);
    if !started {
        return Err(GuiError::conflict(
            error.unwrap_or_else(|| "The investigation could not start".into()),
        ));
    }
    Ok(())
}
