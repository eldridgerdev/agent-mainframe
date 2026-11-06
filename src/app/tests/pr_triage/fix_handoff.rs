use super::*;

#[test]
fn pr_review_inject_fix_also_stashes_return_state() {
    // Regression: `f` (inject fix) used to drop the pane's state on the floor
    // when leaving for the fix session, so `leader+P` had nothing to restore
    // even though `f` is the far more common way into that session (`P` is
    // just a peek). `f` must stash exactly like `P` does.
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
        // This regression exercises the post-target confirm/inject path; the
        // target picker (including its new optional name step) is covered
        // separately.
        state.fix_target_picked = true;
    }
    // Go through the real confirm-dialog path (`f` opens it, then the user
    // confirms) rather than the no-dialog fallback — a prior version of this
    // fix left the confirm dialog attached to the stashed state, so returning
    // via leader+P reopened the same "inject fix" dialog instead of the plain
    // comment list.
    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.fix_confirm.is_some()),
        "confirm dialog should be open before injecting"
    );
    let request_id = match &app.mode {
        AppMode::PrReview(state) => state.fix_confirm.as_ref().unwrap().reply_draft_requests[0]
            .request_id
            .clone(),
        _ => unreachable!(),
    };

    app.pr_review_inject_fix().unwrap();

    // Confirming the injection activates this dialog's request id. The agent
    // can now return a draft through IPC; an id from an older dialog could not
    // overwrite it.
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .capture_pr_comment_reply_draft(7, 2, &request_id, "Fixed the selected path.")
            .unwrap()
    );

    // Compose intercept is on by default, so `f` lands in the compose box
    // (seeded with the fix prompt) rather than bare Viewing — the stash must
    // still be keyed to the same session/window either way.
    let compose_view = match &app.mode {
        AppMode::Compose(state) => state.view.clone(),
        other => panic!("expected Compose, got {:?}", std::mem::discriminant(other)),
    };
    let stash = app
        .pr_review_return
        .as_ref()
        .expect("f should stash the pane state just like the P toggle");
    assert_eq!(stash.session, compose_view.session);
    assert_eq!(stash.window, compose_view.window);
    assert_eq!(stash.state.selected, 1);
    let stash_session = stash.session.clone();

    // Ctrl+Space from the compose box cancels it back to the underlying
    // Viewing session (handlers/compose.rs) before handing off to the leader
    // chord — the real path `leader+P` is reached through after `f`.
    app.cancel_compose();
    assert!(matches!(&app.mode, AppMode::Viewing(view) if view.session == stash_session));

    app.pr_review_return_to_pane();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.selected, 1);
            assert!(
                state.fix_confirm.is_none(),
                "the already-actioned confirm dialog must not reappear"
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.pr_review_return.is_none());

    app.pr_review_open_reply_done();
    assert_eq!(reply_editor_text(&app), "Fixed the selected path.");
}

#[test]
fn fix_confirm_prompt_carries_a_completed_investigations_findings() {
    use crate::app::pr_review::{PrInvestigationStatus, PrInvestigationTurn};
    use crate::db::pr_investigations::PrInvestigation;

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 0; // comment id 1
        state.fix_target_picked = true; // skip the target picker
        let mut inv = PrInvestigation::new_running(
            "proj-1",
            7,
            1,
            "sha",
            crate::project::AgentKind::Codex,
            "ctx",
        );
        inv.status = PrInvestigationStatus::Complete;
        inv.answer = Some("The guard already covers the empty case at line 906.".to_string());
        inv.follow_ups.push(PrInvestigationTurn {
            question: "would a debug_assert help?".to_string(),
            answer: "marginally".to_string(),
            harness: crate::project::AgentKind::Codex,
            created_at: "t".to_string(),
        });
        state.investigations.push(inv);
    }

    app.pr_review_open_fix_confirm();
    let text = match &app.mode {
        AppMode::PrReview(state) => state
            .fix_confirm
            .as_ref()
            .expect("fix confirm open")
            .editor
            .text()
            .to_string(),
        _ => panic!("still in PR Triage"),
    };
    assert!(text.starts_with("Address this PR review comment."));
    assert!(text.contains("A read-only investigation of this comment already ran"));
    assert!(text.contains("The guard already covers the empty case at line 906."));
    assert!(text.contains("Follow-up — Q: would a debug_assert help?"));

    // A dismissed / failed investigation contributes nothing.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_confirm = None;
        state.investigations[0].status = PrInvestigationStatus::Dismissed;
    }
    app.pr_review_open_fix_confirm();
    let text = match &app.mode {
        AppMode::PrReview(state) => state
            .fix_confirm
            .as_ref()
            .unwrap()
            .editor
            .text()
            .to_string(),
        _ => panic!(),
    };
    assert!(!text.contains("A read-only investigation of this comment already ran"));
}

#[test]
fn pr_review_inject_fix_rejects_a_named_session_with_the_wrong_harness() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "PR 321 security".to_string());
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().times(2).returning(|_| true);
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        // Simulate a selection that became stale after the picker closed. The
        // injection boundary must still refuse to route Codex work into the
        // same-named Claude session.
        state.fix_target_picked = true;
        state.review_harness = Some(AgentKind::Codex);
        state.dedicated_session_label = "PR 321 security".to_string();
    }
    app.pr_review_open_fix_confirm();

    app.pr_review_inject_fix().unwrap();

    assert!(
        matches!(&app.mode, AppMode::PrReview(state)
            if state.review.comments[0].triage
                == crate::app::pr_review::TriageState::Untriaged),
        "a rejected target must leave the comment and pane untouched"
    );
    assert!(
        app.toasts
            .last()
            .is_some_and(|toast| toast.message.contains("already runs Claude")),
        "expected an actionable harness-conflict toast, got {:?}",
        app.toasts.last().map(|toast| &toast.message)
    );
}

#[test]
fn pr_review_inject_fix_targets_dialogs_original_comment_after_selection_moves() {
    // Regression: a PR Triage refresh (e.g. the automatic one after posting
    // an AI review) can drop the comment a still-open fix-confirm dialog was
    // built for, falling the selection back onto a different comment while
    // the stale dialog stays open. Confirming must still mark — and route
    // the reply-draft handoff to — the dialog's original comment, never
    // whatever the refresh happened to leave selected.
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);
    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1; // comment id 2
        // Keep this regression focused on the stale-dialog target rather than
        // the first-fix target/name picker.
        state.fix_target_picked = true;
    }
    app.pr_review_open_fix_confirm();
    let request_id = match &app.mode {
        AppMode::PrReview(state) => state.fix_confirm.as_ref().unwrap().reply_draft_requests[0]
            .request_id
            .clone(),
        _ => unreachable!(),
    };

    // Simulate the refresh: comment 2's thread resolved upstream and dropped
    // out of the fetched set, so selection fell back to comment 1. The dialog
    // (still targeting comment 2 via its own `reply_draft_requests`) is left
    // open, exactly as `apply_refreshed_pr_review_state` leaves it.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.comments.retain(|c| c.id != 2);
        state.selected = 0;
    }

    app.pr_review_inject_fix().unwrap();

    let triage = app.db.as_ref().unwrap().load_pr_comment_triage(7).unwrap();
    assert!(
        !triage.contains_key(&1),
        "the newly-selected comment must not be marked Fixing"
    );
    assert_eq!(
        triage.get(&2).map(|row| row.state),
        Some(crate::app::pr_review::TriageState::Fixing),
        "the dialog's original comment must still be marked Fixing"
    );
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .capture_pr_comment_reply_draft(7, 2, &request_id, "Fixed the selected path.")
            .unwrap(),
        "the reply-draft handoff must still route to the dialog's original comment"
    );
}

#[test]
fn single_comment_fix_keeps_a_batched_comments_in_memory_batch_id() {
    // Regression: fixing one comment with `f` set `c.batch_id = batch_id` for
    // every targeted comment, and `batch_id` is `None` on the single-comment
    // path — so a comment that was already part of an earlier combined batch
    // had its in-memory `batch_id` cleared, dropping its `⧉` marker and
    // `[`/`]` sibling jump until the pane was re-entered. The single-comment
    // path must leave an existing `batch_id` alone.
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);
    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 2);

    // Comment id 1 was resolved as part of an earlier batch; comment id 2 is
    // the one now being fixed on its own.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1; // comment id 2
        state.fix_target_picked = true;
        if let Some(c) = state.review.comments.iter_mut().find(|c| c.id == 1) {
            c.batch_id = Some("earlier-batch".to_string());
        }
    }

    app.pr_review_open_fix_confirm();
    app.pr_review_inject_fix().unwrap();

    let stashed = &app
        .pr_review_return
        .as_ref()
        .expect("f stashes the pane state")
        .state;
    assert_eq!(
        stashed
            .review
            .comments
            .iter()
            .find(|c| c.id == 1)
            .unwrap()
            .batch_id
            .as_deref(),
        Some("earlier-batch"),
        "a single-comment fix must not clear an unrelated comment's batch_id"
    );
    assert_eq!(
        stashed
            .review
            .comments
            .iter()
            .find(|c| c.id == 2)
            .unwrap()
            .batch_id,
        None,
        "the single-comment fix target itself gets no batch id"
    );
}

#[test]
fn pr_review_i_opens_syntax_picker_for_selected_comment_file() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2); // comments have paths src/file{id}.rs (Rust)

    app.open_syntax_language_picker_for_selected_diff_file();

    match &app.mode {
        AppMode::SyntaxLanguagePicker(state) => {
            assert_eq!(
                state.languages[state.selected].language,
                crate::highlight::HighlightLanguage::Rust
            );
            // The picker returns to the same review pane on close.
            assert!(matches!(
                state.return_to.as_deref(),
                Some(AppMode::PrReview(_))
            ));
        }
        other => panic!(
            "expected syntax picker, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}
