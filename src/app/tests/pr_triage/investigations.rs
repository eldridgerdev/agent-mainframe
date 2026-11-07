use super::*;

#[test]
fn pr_review_load_investigations_reconciles_a_stuck_running_row() {
    use crate::app::pr_review::PrInvestigationStatus;
    use crate::db::pr_investigations::PrInvestigation;

    let store = store_with_feature(ProjectStatus::Active);
    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());

    // A run that never finished (the process died / AMF restarted) plus a
    // completed one, both for PR 42 on this project.
    let stuck = PrInvestigation::new_running(
        "proj-1",
        42,
        100,
        "sha",
        crate::project::AgentKind::Codex,
        "ctx",
    );
    let mut done = PrInvestigation::new_running(
        "proj-1",
        42,
        101,
        "sha",
        crate::project::AgentKind::Codex,
        "ctx",
    );
    done.status = PrInvestigationStatus::Complete;
    done.answer = Some("all good".to_string());
    let db = app.db.as_ref().unwrap();
    db.upsert_pr_investigation(&stuck).unwrap();
    db.upsert_pr_investigation(&done).unwrap();

    let rows = app.pr_review_load_investigations(std::path::Path::new("/tmp/test-workdir"), 42);
    assert_eq!(rows.len(), 2);
    let stuck_row = rows.iter().find(|r| r.comment_id == 100).unwrap();
    assert_eq!(stuck_row.status, PrInvestigationStatus::Failed);
    assert!(stuck_row.error.as_deref().unwrap().contains("interrupted"));
    let done_row = rows.iter().find(|r| r.comment_id == 101).unwrap();
    assert_eq!(done_row.status, PrInvestigationStatus::Complete);
    assert_eq!(done_row.answer.as_deref(), Some("all good"));

    // The reconcile was persisted, not just applied in memory.
    let reloaded = app
        .db
        .as_ref()
        .unwrap()
        .load_pr_investigations("proj-1", 42)
        .unwrap();
    assert_eq!(
        reloaded
            .iter()
            .find(|r| r.comment_id == 100)
            .unwrap()
            .status,
        PrInvestigationStatus::Failed
    );
}

#[test]
fn pr_investigation_run_never_enters_the_writable_fix_path() {
    // The investigate path must not do any of what `pr_review_inject_fix` does:
    // spin up a triage session, stash `pr_review_return`, move the selection
    // into a session, or switch into Viewing/Compose. It only shows a modal
    // read-only loading frame and returns to the pane.
    let mut store = store_with_feature(ProjectStatus::Active);
    store.available_harnesses = vec![crate::project::AgentKind::Codex]; // exactly one → auto-launch, no picker
    let sessions_before = store.projects[0].features[0].sessions.len();

    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));

    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 2);
    assert!(
        !matches!(app.selection, Selection::Session(..)),
        "precondition: not already on a session"
    );

    app.pr_review_start_investigation();

    // Synchronous invariants: modal read-only loading, nothing session-shaped.
    assert!(
        matches!(app.mode, AppMode::PrInvestigationLoading(_)),
        "investigation shows its own modal loading frame, not a session view"
    );
    assert!(app.pr_review_work.investigation_pending());
    assert!(
        app.pr_review_return.is_none(),
        "no leader+P stash — that belongs to the fix hand-off"
    );
    assert!(
        !matches!(app.selection, Selection::Session(..)),
        "selection not moved into a session (that's the fix hand-off)"
    );
    assert_eq!(
        app.store.projects[0].features[0].sessions.len(),
        sessions_before,
        "no triage session spun up for a read-only investigation"
    );

    // Cancelling the wait (the `esc` path) returns to the pane synchronously —
    // no dependency on the real `gh`/harness subprocess the worker shells out
    // to, which makes CI timing irrelevant. Same invariants after.
    app.pr_investigation_cancel();
    assert!(
        matches!(app.mode, AppMode::PrReview(_)),
        "returned to PR Triage"
    );
    assert!(!app.pr_review_work.investigation_pending());
    assert!(app.pr_review_return.is_none());
    assert!(!matches!(app.selection, Selection::Session(..)));
    assert_eq!(
        app.store.projects[0].features[0].sessions.len(),
        sessions_before
    );
}

#[test]
fn pr_investigation_dismiss_action_flips_status_in_memory_and_on_disk() {
    use crate::app::pr_review::PrInvestigationStatus;
    use crate::db::pr_investigations::PrInvestigation;

    let store = store_with_feature(ProjectStatus::Active);
    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 3);

    let mut inv = PrInvestigation::new_running(
        "proj-1",
        7,
        2,
        "sha",
        crate::project::AgentKind::Codex,
        "ctx",
    );
    inv.status = PrInvestigationStatus::Complete;
    inv.answer = Some("Fix the guard.".to_string());
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1; // comment id 2
        state.investigations.push(inv.clone());
    }
    // The completed row is normally already on disk (written by the poll loop).
    app.db
        .as_ref()
        .unwrap()
        .upsert_pr_investigation(&inv)
        .unwrap();

    // Menu rows: [PostReply, AskFollowUp, Dismiss, KeepAsTodo] — Dismiss is row 2.
    app.pr_review_open_investigation_actions();
    assert!(app.pr_review_investigation_action_picking());
    app.pr_review_investigation_action_move(2);
    app.pr_review_investigation_action_confirm().unwrap();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.investigations[0].status,
                PrInvestigationStatus::Dismissed
            );
            assert_eq!(
                state.investigations[0].answer.as_deref(),
                Some("Fix the guard.")
            );
        }
        _ => panic!("still in PR Triage"),
    }
    let persisted = app
        .db
        .as_ref()
        .unwrap()
        .load_pr_investigations("proj-1", 7)
        .unwrap();
    assert_eq!(persisted[0].status, PrInvestigationStatus::Dismissed);
}

#[test]
fn pr_investigation_action_menu_only_opens_with_a_finished_investigation() {
    use crate::app::pr_review::PrInvestigationStatus;
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
    }

    // No investigation at all: the menu refuses to open.
    app.pr_review_open_investigation_actions();
    assert!(!app.pr_review_investigation_action_picking());

    // A still-running investigation: still refused (nothing to route yet).
    if let AppMode::PrReview(state) = &mut app.mode {
        state.investigations.push(PrInvestigation::new_running(
            "proj-1",
            7,
            1,
            "sha",
            crate::project::AgentKind::Codex,
            "ctx",
        ));
    }
    app.pr_review_open_investigation_actions();
    assert!(!app.pr_review_investigation_action_picking());

    // Completed: the menu opens.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.investigations[0].status = PrInvestigationStatus::Complete;
        state.investigations[0].answer = Some("done".to_string());
    }
    app.pr_review_open_investigation_actions();
    assert!(app.pr_review_investigation_action_picking());
}

#[test]
fn pr_investigation_post_reply_opens_an_editable_draft_seeded_from_the_answer() {
    use crate::app::pr_review::{PrInvestigationStatus, ReplyKind};
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
        let mut inv = PrInvestigation::new_running(
            "proj-1",
            7,
            1,
            "sha",
            crate::project::AgentKind::Codex,
            "ctx",
        );
        inv.status = PrInvestigationStatus::Complete;
        inv.answer = Some("The guard is missing the negative case.".to_string());
        state.investigations.push(inv);
    }

    app.pr_review_open_investigation_actions();
    // Menu rows: [PostReply, AskFollowUp, Dismiss, KeepAsTodo] — PostReply is row 0.
    app.pr_review_investigation_action_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => {
            let reply = state.reply.as_ref().expect("reply dialog is open");
            assert_eq!(reply.kind, ReplyKind::Investigation);
            assert!(!reply.editing, "opens in confirm view, editable on demand");
            assert!(
                reply
                    .editor
                    .text()
                    .contains("The guard is missing the negative case.")
            );
            assert!(reply.editor.text().contains("read-only investigation"));
            assert!(!reply.agent_drafted);
            assert!(state.investigation_action_pick.is_none());
        }
        _ => panic!("still in PR Triage"),
    }
}

#[test]
fn pr_investigation_follow_up_editor_opens_and_refuses_an_empty_question() {
    use crate::app::pr_review::PrInvestigationStatus;
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
        let mut inv = PrInvestigation::new_running(
            "proj-1",
            7,
            1,
            "sha",
            crate::project::AgentKind::Codex,
            "ctx",
        );
        inv.status = PrInvestigationStatus::Complete;
        inv.answer = Some("done".to_string());
        state.investigations.push(inv);
    }

    app.pr_review_open_investigation_actions();
    app.pr_review_investigation_action_move(1); // [PostReply, AskFollowUp, Dismiss, KeepAsTodo]
    app.pr_review_investigation_action_confirm().unwrap();
    assert!(app.pr_review_investigation_follow_up_open());

    // Empty question: refused, editor stays open, no parked follow-up.
    app.pr_review_investigation_follow_up_submit();
    assert!(app.pr_review_investigation_follow_up_open());
    match &app.mode {
        AppMode::PrReview(state) => assert!(state.pending_follow_up.is_none()),
        _ => panic!(),
    }

    app.pr_review_investigation_follow_up_cancel();
    assert!(!app.pr_review_investigation_follow_up_open());
}
