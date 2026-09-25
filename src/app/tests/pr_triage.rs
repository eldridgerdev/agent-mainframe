use super::support::*;
use crate::app::*;
use crate::automation::{SeedAiReviewFinding, SeedAiReviewRequest};
use crate::project::{AgentKind, SessionKind};
use crate::token_tracking::{SessionTokenUsage, TokenUsageProvider, TokenUsageSource};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent};
use std::collections::HashMap;
use tempfile::NamedTempFile;
use tempfile::TempDir;

fn pr_review_selected(app: &App) -> usize {
    match &app.mode {
        AppMode::PrReview(state) => state.selected,
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn github_repository_resolution_uses_the_selected_project_repo() {
    let repo = TempDir::new().unwrap();
    assert!(
        std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        std::process::Command::new("git")
            .args(["remote", "add", "origin", "git@github.com:Acme/Widget.git"])
            .current_dir(repo.path())
            .status()
            .unwrap()
            .success()
    );

    let app = App::new_for_test(
        store_with_repo(repo.path().to_path_buf(), ProjectStatus::Idle),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let repository = app.github_repository_for_project(0).unwrap();
    assert_eq!(repository.canonical(), "github.com/acme/widget");
}

#[test]
fn pr_review_navigation_clamps_at_both_ends() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);

    // Already at top: prev is a no-op.
    app.pr_review_select_prev();
    assert_eq!(pr_review_selected(&app), 0);

    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 1);
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 2);

    // At the bottom: next is a no-op (no wrap).
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 2);
}

#[test]
fn pr_review_close_returns_to_dashboard() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.close_pr_review();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(!app.pr_review_work.fetch_pending());
}

#[test]
fn pr_review_toggle_to_session_requires_existing_session() {
    // `store_with_feature` has no sessions, so there's no dedicated triage
    // session to jump to yet — the toggle must hint rather than create one
    // as a side effect of a quick peek.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_toggle_to_session().unwrap();

    assert!(matches!(app.mode, AppMode::PrReview(_)));
    assert!(app.pr_review_return.is_none());
}

#[test]
fn pr_review_toggle_to_session_jumps_and_stashes_state() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
        state.detail_scroll = 3;
    }

    app.pr_review_toggle_to_session().unwrap();

    match &app.mode {
        AppMode::Viewing(view) => {
            assert_eq!(view.session, "amf-my-feat");
            assert_eq!(view.session_label, "PR Triage");
        }
        other => panic!("expected Viewing, got {:?}", std::mem::discriminant(other)),
    }
    let stash = app
        .pr_review_return
        .as_ref()
        .expect("pane state should be stashed");
    assert_eq!(stash.session, "amf-my-feat");
    assert_eq!(stash.state.selected, 1);
    assert_eq!(stash.state.detail_scroll, 3);
}

#[test]
fn pr_review_toggle_to_session_uses_the_selected_custom_name() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());
    store.projects[0].features[0]
        .add_session_named(SessionKind::Codex, "PR 321 security".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.dedicated_session_label = "PR 321 security".to_string();
        state.fix_target_picked = true;
    }

    app.pr_review_toggle_to_session().unwrap();

    match &app.mode {
        AppMode::Viewing(view) => {
            assert_eq!(view.session_label, "PR 321 security");
            assert_eq!(view.session_kind, SessionKind::Codex);
        }
        other => panic!("expected Viewing, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_return_to_pane_restores_stashed_state() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
    }
    app.pr_review_toggle_to_session().unwrap();

    app.pr_review_return_to_pane();

    match &app.mode {
        AppMode::PrReview(state) => assert_eq!(state.selected, 1),
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(
        app.pr_review_return.is_none(),
        "stash should be consumed on restore"
    );
}

#[test]
fn pr_review_return_to_pane_ignores_mismatched_session() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| true);

    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_toggle_to_session().unwrap();

    // Simulate having navigated away to an unrelated session's view.
    if let AppMode::Viewing(view) = &mut app.mode {
        view.session = "amf-other-feat".to_string();
        view.window = "claude".to_string();
    }

    app.pr_review_return_to_pane();

    assert!(
        matches!(&app.mode, AppMode::Viewing(view) if view.session == "amf-other-feat"),
        "should not have jumped into the stashed pane from an unrelated session"
    );
    assert!(
        app.pr_review_return.is_some(),
        "the stash should be left alone, not dropped, in case the user navigates back"
    );
}

#[test]
fn pr_review_return_to_pane_without_stash_shows_message() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Normal;

    app.pr_review_return_to_pane();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.pr_review_return.is_none());
}

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

fn sample_ai_review_finding(body: &str) -> crate::app::ai_review::AiReviewFinding {
    crate::app::ai_review::AiReviewFinding {
        path: None,
        line: None,
        side: None,
        body: body.to_string(),
        diff_hunk: None,
        skipped: false,
        published: false,
    }
}

#[test]
fn ai_review_finding_fix_costs_correlate_to_resolved_batched_comments() {
    let db_file = tempfile::NamedTempFile::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());

    // A cached review with two code comments (ids 1 and 2, on src/file1.rs:1
    // and src/file2.rs:2) to correlate findings against.
    let review = pr_review_with_comments(2);
    app.db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&review)
        .unwrap();

    // Comment 1 was fixed in a combined batch and resolved; comment 2 wasn't
    // touched.
    let db = app.db.as_ref().unwrap();
    db.save_pr_comment_triage(
        7,
        "sha",
        1,
        crate::app::pr_review::TriageState::Fixing,
        None,
        Some("batch-z"),
    )
    .unwrap();
    db.save_pr_comment_triage(
        7,
        "sha",
        1,
        crate::app::pr_review::TriageState::Done,
        None,
        None,
    )
    .unwrap();
    // Comment 2 is in the same batch but was never resolved — the partial-batch
    // rule means it must contribute no cost line.
    db.save_pr_comment_triage(
        7,
        "sha",
        2,
        crate::app::pr_review::TriageState::Fixing,
        None,
        Some("batch-z"),
    )
    .unwrap();
    db.set_pr_comment_batch_fix_cost(7, "batch-z", "$0.09")
        .unwrap();

    let pr = review.pr.clone();
    let mut state = sample_ai_review_state(std::path::PathBuf::from("/tmp/wd"), pr);
    // Finding 0 matches comment 1 (resolved, batched); finding 1 matches
    // comment 2 (batched but unresolved); finding 2 matches nothing.
    let mut f0 = sample_ai_review_finding("race");
    f0.path = Some("src/file1.rs".to_string());
    f0.line = Some(1);
    let mut f1 = sample_ai_review_finding("style");
    f1.path = Some("src/file2.rs".to_string());
    f1.line = Some(2);
    let mut f2 = sample_ai_review_finding("orphan");
    f2.path = Some("src/other.rs".to_string());
    f2.line = Some(9);
    state.findings = vec![f0, f1, f2];
    app.mode = AppMode::AiReview(state);

    let costs = app.ai_review_finding_fix_costs();
    assert_eq!(
        costs,
        vec![
            Some("Fix cost (est.): $0.09 · combined (2)".to_string()),
            None,
            None,
        ]
    );

    // The result is memoized: a second call with the pane's findings unchanged
    // answers from `ai_review_fix_cost_cache` without re-hitting the DB. Prove
    // it by dropping the DB and confirming the same answer still comes back.
    assert!(app.ai_review_fix_cost_cache.is_some());
    app.db = None;
    assert_eq!(app.ai_review_finding_fix_costs(), costs);

    // Clearing the memo (what pane (re)open and a landed `A` run do) forces a
    // recompute — now with no DB, so every finding falls back to `None`.
    app.ai_review_fix_cost_cache = None;
    assert_eq!(app.ai_review_finding_fix_costs(), vec![None, None, None]);
}

#[test]
fn ai_review_model_picker_opens_for_pi() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Pi);
    }

    app.start_ai_pr_review();

    match &app.mode {
        AppMode::AiReview(state) => {
            // Pi's headless CLI takes `--model`, so it must get the picker
            // rather than being force-skipped to the harness default.
            assert!(!state.model_picked);
            let pick = state
                .model_pick
                .as_ref()
                .expect("Pi should open the model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
        }
        _ => panic!("expected AI Review pane"),
    }
    assert!(
        !app.ai_review_run.is_pending(),
        "review should not have started"
    );
}

#[test]
fn ai_review_model_picker_backs_through_custom_editor_and_rebuilds_for_new_harness() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .times(1)
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.config.review_model = Some("sonnet".to_string());
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Claude);
        state.model = Some("sonnet".to_string());
        state.model_pick = Some(AiModelPickState {
            rows: vec![
                ModelPickRow::Default,
                ModelPickRow::Preset("sonnet".to_string()),
                ModelPickRow::Custom,
            ],
            selected: 2,
            custom_input: "claude-custom".to_string(),
            editing_custom: true,
        });
    }

    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::AiReview(state) => {
            let pick = state.model_pick.as_ref().expect("model list should remain");
            assert!(!pick.editing_custom);
            assert_eq!(pick.custom_input, "claude-custom");
            assert!(state.harness_pick.is_none());
        }
        _ => panic!("expected AI Review pane"),
    }

    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::AiReview(state) => {
            let pick = state
                .harness_pick
                .as_ref()
                .expect("model list should return to harness picker");
            assert_eq!(pick.agents[pick.selected], AgentKind::Claude);
            assert!(state.harness.is_none());
            assert!(state.model.is_none());
            assert!(!state.model_picked);
            assert!(state.model_pick.is_none());
        }
        _ => panic!("expected AI Review pane"),
    }

    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();

    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            assert!(state.harness_pick.is_none());
            assert!(state.model.is_none());
            assert!(!state.model_picked);
            let pick = state
                .model_pick
                .as_ref()
                .expect("new harness should open a rebuilt model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
            assert_eq!(pick.selected, 0);
            assert!(pick.custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }
}

/// Regression for the gap `ai_review_model_picker_backs_through_custom_editor_and_rebuilds_for_new_harness`
/// doesn't cover: backing out *again* after switching harness once, and
/// reconfirming the *same* (already-switched-to) harness a second time.
/// `harness_changed` used to compare only against the immediately preceding
/// screen, so this second confirm looked unchanged and fell through to
/// `start_ai_pr_review`'s `AppConfig::review_model` fallback — reseeding the
/// Claude-only "sonnet" preset as a custom Opencode model, exactly what the
/// first switch had correctly cleared.
#[test]
fn ai_review_harness_pick_reconfirm_after_backing_out_does_not_reseed_stale_model() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .times(2)
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.config.review_model = Some("sonnet".to_string());
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Claude);
        state.model_pick = Some(AiModelPickState {
            rows: vec![
                ModelPickRow::Default,
                ModelPickRow::Preset("sonnet".to_string()),
                ModelPickRow::Custom,
            ],
            selected: 0,
            custom_input: String::new(),
            editing_custom: false,
        });
    }

    // Back out of the model picker (Claude -> harness picker) and switch to
    // Opencode: this is the already-covered case, so the model resets to
    // fresh defaults instead of "sonnet".
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            let pick = state.model_pick.as_ref().expect("rebuilt model picker");
            assert!(pick.custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Back out a second time without picking a model, and reconfirm the same
    // (already-switched-to) Opencode harness.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::AiReview(state) => {
            let pick = state
                .harness_pick
                .as_ref()
                .expect("should return to harness picker");
            assert_eq!(pick.agents[pick.selected], AgentKind::Opencode);
        }
        _ => panic!("expected AI Review pane"),
    }
    app.accept_selected_ai_review_harness_for_test();

    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            let pick = state
                .model_pick
                .as_ref()
                .expect("reconfirming should still open a fresh model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
            assert_eq!(pick.selected, 0);
            assert!(
                pick.custom_input.is_empty(),
                "must not reseed the Claude-only \"sonnet\" default as an Opencode custom model"
            );
        }
        _ => panic!("expected AI Review pane"),
    }
}

/// Broader regression covering the reviewer's general "repeated navigation"
/// concern rather than just a single back-and-forth: hops through three
/// harnesses (Claude -> Opencode -> Codex -> Opencode -> Claude), backing out
/// to the harness picker between every hop. Every switch away from the
/// original Claude harness must land on fresh `Default`/empty model defaults
/// (never a stale non-Claude value), and switching *back* to the original
/// Claude harness must still restore the meaningful `AppConfig::review_model`
/// default ("sonnet") rather than being needlessly cleared just because a
/// picker round trip happened.
#[test]
fn ai_review_harness_pick_survives_multi_hop_navigation() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .times(4)
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.config.review_model = Some("sonnet".to_string());
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Claude);
        state.model_pick = Some(AiModelPickState {
            rows: vec![
                ModelPickRow::Default,
                ModelPickRow::Preset("sonnet".to_string()),
                ModelPickRow::Custom,
            ],
            selected: 1,
            custom_input: String::new(),
            editing_custom: false,
        });
    }

    // Claude -> Opencode: first switch away from the original harness.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            assert!(state.model_pick.as_ref().unwrap().custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Opencode -> Codex: still away from the original, still fresh defaults.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Codex));
            let pick = state.model_pick.as_ref().expect("rebuilt model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
            assert!(pick.custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Codex -> Opencode: back to a previously-visited harness, still not the
    // original — must not resurrect anything stale.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Up)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            assert!(state.model_pick.as_ref().unwrap().custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Opencode -> Claude: switching back to the *original* harness should
    // restore the configured "sonnet" default, not force an empty Custom.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Up)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Claude));
            let pick = state.model_pick.as_ref().expect("rebuilt model picker");
            assert_eq!(
                pick.rows.get(pick.selected),
                Some(&ModelPickRow::Preset("sonnet".to_string())),
                "returning to the original harness should still honor the configured default"
            );
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn review_memory_bootstrap_pick_opens_defaulting_to_fifty() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);

    assert!(!app.review_memory_bootstrap_picking());
    app.open_review_memory_bootstrap_pick();
    assert!(app.review_memory_bootstrap_picking());

    match &app.mode {
        AppMode::PrPicker(state) => {
            let pick = state.bootstrap_pick.as_ref().unwrap();
            assert_eq!(
                crate::app::pr_review::BootstrapDepth::ALL[pick.selected],
                crate::app::pr_review::BootstrapDepth::default()
            );
        }
        other => panic!("expected PrPicker, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn review_memory_bootstrap_pick_defaults_to_project_scope_and_toggles() {
    use crate::app::review_memory::MemoryScope;

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    app.open_review_memory_bootstrap_pick();

    let scope = |app: &App| match &app.mode {
        AppMode::PrPicker(state) => state.bootstrap_pick.as_ref().unwrap().scope,
        _ => panic!("expected PrPicker"),
    };
    assert_eq!(
        scope(&app),
        MemoryScope::Project,
        "a bootstrap learns from this repo's history, so it defaults to this repo's doc"
    );

    app.review_memory_bootstrap_toggle_scope();
    assert_eq!(scope(&app), MemoryScope::Global);
    app.review_memory_bootstrap_toggle_scope();
    assert_eq!(scope(&app), MemoryScope::Project);
}

#[test]
fn review_memory_bootstrap_pick_move_wraps() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    app.open_review_memory_bootstrap_pick();

    let selected = |app: &App| match &app.mode {
        AppMode::PrPicker(state) => state.bootstrap_pick.as_ref().unwrap().selected,
        _ => panic!("expected PrPicker"),
    };

    // Starts on the default (Fifty, index 1).
    assert_eq!(selected(&app), 1);
    app.review_memory_bootstrap_pick_move(1);
    assert_eq!(selected(&app), 2);
    app.review_memory_bootstrap_pick_move(1);
    assert_eq!(selected(&app), 3);
    // Wraps forward past the last entry.
    app.review_memory_bootstrap_pick_move(1);
    assert_eq!(selected(&app), 0);
    // Wraps backward past the first entry.
    app.review_memory_bootstrap_pick_move(-1);
    assert_eq!(selected(&app), 3);
}

#[test]
fn review_memory_bootstrap_pick_cancel_closes_the_overlay() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    app.open_review_memory_bootstrap_pick();
    assert!(app.review_memory_bootstrap_picking());

    app.review_memory_bootstrap_pick_cancel();

    assert!(!app.review_memory_bootstrap_picking());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
}

#[test]
fn poll_review_memory_bootstrap_bg_surfaces_result_and_returns_to_picker() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_bootstrap_bg = Some(rx);
    app.mode = AppMode::ReviewMemoryBootstrapRunning(crate::app::BootstrapRunState {
        scope: crate::app::review_memory::MemoryScope::Project,
        origin,
        depth: crate::app::pr_review::BootstrapDepth::default(),
        stage: crate::app::pr_review::BootstrapStage::FetchingComments,
    });

    tx.send(crate::app::pr_review::BootstrapProgress::Distilling {
        pr_count: 3,
        token_estimate: 42,
    })
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    match &app.mode {
        AppMode::ReviewMemoryBootstrapRunning(state) => assert_eq!(
            state.stage,
            crate::app::pr_review::BootstrapStage::Distilling {
                pr_count: 3,
                token_estimate: 42,
            }
        ),
        other => panic!(
            "expected ReviewMemoryBootstrapRunning, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    tx.send(crate::app::pr_review::BootstrapProgress::Done(Ok(
        crate::app::pr_review::BootstrapOutcome {
            pr_count: 3,
            appended: 2,
        },
    )))
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert!(app.review_memory_bootstrap_bg.is_none());
}

#[test]
fn poll_review_memory_bootstrap_bg_error_still_returns_to_picker() {
    // Regression: `show_error` unconditionally resets `self.mode` to `Normal`
    // for any non-Normal/Help/Viewing mode. The `Done(Err(_))` branch used to
    // call it *before* restoring the origin picker, so a failed distill (e.g.
    // the headless `claude` call erroring) dumped the user onto the bare
    // dashboard instead of back to the PR picker like the success path does.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_bootstrap_bg = Some(rx);
    app.mode = AppMode::ReviewMemoryBootstrapRunning(crate::app::BootstrapRunState {
        scope: crate::app::review_memory::MemoryScope::Project,
        origin,
        depth: crate::app::pr_review::BootstrapDepth::default(),
        stage: crate::app::pr_review::BootstrapStage::FetchingComments,
    });

    tx.send(crate::app::pr_review::BootstrapProgress::Done(Err(
        anyhow::anyhow!("claude headless command failed"),
    )))
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    match &app.mode {
        AppMode::PrPicker(state) => {
            assert!(
                state
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("claude headless command failed")),
                "expected the picker's inline error to surface the failure, got {:?}",
                state.error
            );
        }
        other => panic!("expected PrPicker, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.review_memory_bootstrap_bg.is_none());
}

#[test]
fn cancel_review_memory_bootstrap_returns_to_picker_without_dropping_the_bg_result() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = match &app.mode {
        AppMode::PrPicker(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_bootstrap_bg = Some(rx);
    app.mode = AppMode::ReviewMemoryBootstrapRunning(crate::app::BootstrapRunState {
        scope: crate::app::review_memory::MemoryScope::Project,
        origin,
        depth: crate::app::pr_review::BootstrapDepth::default(),
        stage: crate::app::pr_review::BootstrapStage::FetchingComments,
    });

    // The user gives up watching...
    app.cancel_review_memory_bootstrap();
    assert!(matches!(app.mode, AppMode::PrPicker(_)));

    // ...but the background run still finishes and reports its result (a real
    // side effect: it wrote findings and spent tokens), even though the user
    // isn't looking at the running screen anymore.
    tx.send(crate::app::pr_review::BootstrapProgress::Done(Ok(
        crate::app::pr_review::BootstrapOutcome {
            pr_count: 5,
            appended: 1,
        },
    )))
    .unwrap();
    assert!(app.poll_review_memory_bootstrap_bg());
    assert!(app.review_memory_bootstrap_bg.is_none());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
}

#[test]
fn open_review_memory_compact_confirm_reads_doc_and_opens() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- One\n- Two\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    });

    assert!(!app.review_memory_compact_confirming());
    app.open_review_memory_compact_confirm();
    assert!(app.review_memory_compact_confirming());

    match &app.mode {
        AppMode::ReviewMemoryCompactConfirm(state) => {
            assert_eq!(state.confirm.existing_findings, 2);
            assert_eq!(
                state.confirm.scope,
                crate::app::review_memory::MemoryScope::Project
            );
            assert!(matches!(state.prior_mode.as_ref(), AppMode::PrPicker(_)));
        }
        other => panic!(
            "expected ReviewMemoryCompactConfirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn open_review_memory_compact_confirm_reachable_from_pr_triage() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- One\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo;
    }

    app.open_review_memory_compact_confirm();

    match &app.mode {
        AppMode::ReviewMemoryCompactConfirm(state) => {
            assert_eq!(state.confirm.existing_findings, 1);
            assert!(matches!(state.prior_mode.as_ref(), AppMode::PrReview(_)));
        }
        other => panic!(
            "expected ReviewMemoryCompactConfirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    // Cancelling restores PR Triage, not the picker.
    app.review_memory_compact_confirm_cancel();
    assert!(matches!(app.mode, AppMode::PrReview(_)));
}

#[test]
fn open_review_memory_compact_confirm_reachable_from_the_dashboard() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- One\n- Two\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].workdir = repo;
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.selection = Selection::Feature(0, 0);
    assert!(matches!(app.mode, AppMode::Normal));

    app.open_review_memory_compact_confirm();

    match &app.mode {
        AppMode::ReviewMemoryCompactConfirm(state) => {
            assert_eq!(state.confirm.existing_findings, 2);
            assert!(matches!(state.prior_mode.as_ref(), AppMode::Normal));
        }
        other => panic!(
            "expected ReviewMemoryCompactConfirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    // Cancelling restores the dashboard.
    app.review_memory_compact_confirm_cancel();
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn open_review_memory_compact_confirm_uses_global_when_only_global_has_findings() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let global_doc = tmp.path().join("global-review-memory.md");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        &global_doc,
        "# Review memory (cross-project)\n\n## Tests\n- One\n- Two\n- Three\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    });

    app.open_review_memory_compact_confirm();

    match &app.mode {
        AppMode::ReviewMemoryCompactConfirm(state) => {
            assert_eq!(state.confirm.existing_findings, 3);
            assert_eq!(
                state.confirm.scope,
                crate::app::review_memory::MemoryScope::Global
            );
        }
        other => panic!(
            "expected ReviewMemoryCompactConfirm, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn review_memory_compact_toggle_scope_rereads_the_selected_doc() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let global_doc = tmp.path().join("global-review-memory.md");
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("review-memory.md"),
        "# Review memory\n\n## Tests\n- Project one\n- Project two\n",
    )
    .unwrap();
    std::fs::write(
        &global_doc,
        "# Review memory (cross-project)\n\n## Tests\n- Global one\n",
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    });

    let compact_selection = |app: &App| match &app.mode {
        AppMode::ReviewMemoryCompactConfirm(state) => {
            (state.confirm.scope, state.confirm.existing_findings)
        }
        _ => panic!("expected ReviewMemoryCompactConfirm"),
    };

    app.open_review_memory_compact_confirm();
    assert_eq!(
        compact_selection(&app),
        (crate::app::review_memory::MemoryScope::Project, 2)
    );

    app.review_memory_compact_toggle_scope();
    assert_eq!(
        compact_selection(&app),
        (crate::app::review_memory::MemoryScope::Global, 1)
    );

    app.review_memory_compact_toggle_scope();
    assert_eq!(
        compact_selection(&app),
        (crate::app::review_memory::MemoryScope::Project, 2)
    );
}

#[test]
fn open_review_memory_compact_confirm_bails_when_doc_missing() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.mode = AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    });

    app.open_review_memory_compact_confirm();

    assert!(!app.review_memory_compact_confirming());
    assert!(app.message.is_some());
}

#[test]
fn review_memory_compact_confirm_cancel_closes_the_overlay() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let prior_mode = Box::new(std::mem::replace(&mut app.mode, AppMode::Normal));
    app.mode = AppMode::ReviewMemoryCompactConfirm(Box::new(
        crate::app::ReviewMemoryCompactConfirmState {
            workdir: std::path::PathBuf::from("/tmp/test-workdir"),
            confirm: crate::app::CompactConfirmState {
                existing_findings: 3,
                scope: crate::app::review_memory::MemoryScope::Project,
            },
            prior_mode,
        },
    ));
    assert!(app.review_memory_compact_confirming());

    app.review_memory_compact_confirm_cancel();

    assert!(!app.review_memory_compact_confirming());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
}

#[test]
fn review_memory_compact_confirm_run_targets_the_selected_global_doc() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    let global_doc = tmp.path().join("global-review-memory.md");
    std::fs::create_dir_all(&repo).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        store_with_feature(ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    let prior_mode = Box::new(AppMode::PrPicker(crate::app::PrPickerState {
        workdir: repo,
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    }));
    app.mode = AppMode::ReviewMemoryCompactConfirm(Box::new(
        crate::app::ReviewMemoryCompactConfirmState {
            workdir: tmp.path().join("repo"),
            // The missing file makes the spawned background read return
            // immediately, while this cached count still lets us exercise the
            // selected-path handoff without invoking an agent in the test.
            confirm: crate::app::CompactConfirmState {
                existing_findings: 1,
                scope: crate::app::review_memory::MemoryScope::Global,
            },
            prior_mode,
        },
    ));

    app.review_memory_compact_confirm_run();
    // The pre-call notice interposes; continuing dispatches the run.
    assert!(matches!(&app.mode, AppMode::PromptPrecall(p)
        if p.prompt_id == crate::prompts::PromptId::ReviewMemoryCompact));
    app.precall_confirm().unwrap();

    match &app.mode {
        AppMode::ReviewMemoryCompactRunning(view) => {
            assert_eq!(view.scope, crate::app::review_memory::MemoryScope::Global);
        }
        other => panic!(
            "expected ReviewMemoryCompactRunning, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    assert_eq!(
        app.review_memory_compact_pending
            .as_ref()
            .map(|state| (&state.path, state.scope)),
        Some((&global_doc, crate::app::review_memory::MemoryScope::Global))
    );
    assert!(matches!(
        app.review_memory_compact_pending
            .as_ref()
            .map(|state| state.origin.as_ref()),
        Some(AppMode::PrPicker(_))
    ));
}

#[test]
fn poll_review_memory_compact_bg_success_opens_review_dialog() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = Box::new(std::mem::replace(&mut app.mode, AppMode::Normal));

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    app.review_memory_compact_pending = Some(crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Global,
    });
    app.mode = AppMode::ReviewMemoryCompactRunning(crate::app::CompactRunView {
        scope: crate::app::review_memory::MemoryScope::Global,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    });

    tx.send(crate::app::pr_review::CompactProgress::Compacting { token_estimate: 99 })
        .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    match &app.mode {
        AppMode::ReviewMemoryCompactRunning(view) => assert_eq!(
            view.stage,
            crate::app::pr_review::CompactStage::Compacting { token_estimate: 99 }
        ),
        other => panic!(
            "expected ReviewMemoryCompactRunning, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    tx.send(crate::app::pr_review::CompactProgress::Done(Ok(Some(
        crate::app::pr_review::CompactOutcome {
            original_findings: 5,
            proposed_findings: 3,
            proposed_content: "# Review memory\n\n## Tests\n- Merged finding\n".to_string(),
            original_content: "# Review memory\n\n## Tests\n- One\n- Two\n".to_string(),
        },
    ))))
    .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    assert!(app.review_memory_compact_bg.is_none());
    match &app.mode {
        AppMode::ReviewMemoryCompactReview(state) => {
            assert_eq!(state.scope, crate::app::review_memory::MemoryScope::Global);
            assert_eq!(state.original_findings, 5);
            assert_eq!(state.proposed_findings, 3);
            assert_eq!(
                state.editor.text(),
                "# Review memory\n\n## Tests\n- Merged finding\n"
            );
        }
        other => panic!(
            "expected ReviewMemoryCompactReview, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn poll_review_memory_compact_bg_nothing_to_compact_returns_to_picker_with_message() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = Box::new(std::mem::replace(&mut app.mode, AppMode::Normal));

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    app.review_memory_compact_pending = Some(crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Project,
    });
    app.mode = AppMode::ReviewMemoryCompactRunning(crate::app::CompactRunView {
        scope: crate::app::review_memory::MemoryScope::Project,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    });

    tx.send(crate::app::pr_review::CompactProgress::Done(Ok(None)))
        .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert!(app.message.is_some());
}

#[test]
fn poll_review_memory_compact_bg_error_still_returns_to_picker() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = Box::new(std::mem::replace(&mut app.mode, AppMode::Normal));

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    app.review_memory_compact_pending = Some(crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Project,
    });
    app.mode = AppMode::ReviewMemoryCompactRunning(crate::app::CompactRunView {
        scope: crate::app::review_memory::MemoryScope::Project,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    });

    tx.send(crate::app::pr_review::CompactProgress::Done(Err(
        anyhow::anyhow!("claude headless command failed"),
    )))
    .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    match &app.mode {
        AppMode::PrPicker(state) => {
            assert!(
                state
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("claude headless command failed"))
            );
        }
        other => panic!("expected PrPicker, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.review_memory_compact_bg.is_none());
}

#[test]
fn cancel_review_memory_compact_does_not_reopen_review_dialog_over_the_user() {
    // Regression guard for the analogous AI-review bug (`ai_review_pending`):
    // a late `Done` after the user already backed out (`esc`) must not force
    // them into a full-screen dialog they didn't ask to see, and must not
    // panic when `self.mode` is no longer `ReviewMemoryCompactRunning`.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_picker_for_test(&mut app);
    let origin = Box::new(std::mem::replace(&mut app.mode, AppMode::Normal));

    let (tx, rx) = std::sync::mpsc::channel();
    app.review_memory_compact_bg = Some(rx);
    app.review_memory_compact_pending = Some(crate::app::CompactRunState {
        origin,
        path: std::path::PathBuf::from("/tmp/test-workdir/.amf/review-memory.md"),
        scope: crate::app::review_memory::MemoryScope::Project,
    });
    app.mode = AppMode::ReviewMemoryCompactRunning(crate::app::CompactRunView {
        scope: crate::app::review_memory::MemoryScope::Project,
        stage: crate::app::pr_review::CompactStage::ReadingDoc,
    });

    app.cancel_review_memory_compact();
    assert!(matches!(app.mode, AppMode::PrPicker(_)));

    tx.send(crate::app::pr_review::CompactProgress::Done(Ok(Some(
        crate::app::pr_review::CompactOutcome {
            original_findings: 2,
            proposed_findings: 1,
            proposed_content: "# Review memory\n".to_string(),
            original_content: "# Review memory\n\n## Tests\n- One\n- Two\n".to_string(),
        },
    ))))
    .unwrap();
    assert!(app.poll_review_memory_compact_bg());
    assert!(app.review_memory_compact_bg.is_none());
    assert!(app.review_memory_compact_pending.is_none());
    // Stays on the picker the user cancelled back to — not yanked into the
    // review dialog, and not silently dropped either (a toast explains it).
    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert!(
        app.toasts
            .last()
            .is_some_and(|t| t.message.contains("navigated away"))
    );
}

/// Enter the compact review dialog with `content` proposed for `path`, taking
/// the conflict baseline from whatever is on disk at `path` right now — the
/// same snapshot the background pass would have read.
fn enter_compact_review_for_test(app: &mut App, path: std::path::PathBuf, content: &str) {
    let original_content = std::fs::read_to_string(&path).unwrap_or_default();
    let origin = Box::new(AppMode::PrPicker(crate::app::PrPickerState {
        workdir: std::path::PathBuf::from("/tmp/test-workdir"),
        entries: vec![],
        selected: 0,
        include_closed: false,
        error: None,
        bootstrap_pick: None,
        current_user: None,
    }));
    app.mode = AppMode::ReviewMemoryCompactReview(crate::app::CompactReviewState {
        origin,
        path,
        scope: crate::app::review_memory::MemoryScope::Project,
        original_findings: 3,
        proposed_findings: 2,
        editor: crate::editor::TextEditor::new(content.to_string()),
        original_content,
        overwrite_confirmed: false,
        editing: false,
        scroll: 0,
        sync_to_cursor: false,
        error: None,
    });
}

#[test]
fn pr_review_compact_write_overwrites_file_and_returns_to_picker() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    std::fs::write(
        &path,
        "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n",
    )
    .unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(
        &mut app,
        path.clone(),
        "# Review memory\n\n## Tests\n- Merged finding\n",
    );

    app.pr_review_compact_write().unwrap();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents, "# Review memory\n\n## Tests\n- Merged finding\n");
    assert!(
        app.toasts
            .last()
            .is_some_and(|t| t.message.contains("3") && t.message.contains("2"))
    );
}

#[test]
fn pr_review_compact_write_keeps_findings_appended_while_the_dialog_was_open() {
    // The compact proposal rewrites a snapshot taken before the agent pass. Any
    // AMF session can append to the same doc in that window (every session on
    // the machine shares the global one), so those findings have to survive the
    // rewrite instead of being clobbered by a stale snapshot.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    std::fs::write(
        &path,
        "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n",
    )
    .unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(
        &mut app,
        path.clone(),
        "# Review memory\n\n## Tests\n- Merged finding\n",
    );

    // Another session appends after the snapshot was taken.
    crate::app::review_memory::append_finding(
        &path,
        crate::app::review_memory::MemoryScope::Project,
        "Concurrency",
        "Guard the shared doc",
    )
    .unwrap();

    app.pr_review_compact_write().unwrap();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    let contents = std::fs::read_to_string(&path).unwrap();
    assert!(contents.contains("- Merged finding"));
    assert!(contents.contains("- Guard the shared doc"));
    assert!(
        app.toasts
            .last()
            .is_some_and(|t| t.message.contains("kept 1 finding added elsewhere"))
    );
}

#[test]
fn pr_review_compact_write_refuses_once_when_the_doc_diverged_then_overwrites() {
    // A change no append can explain (prose hand-edited, findings deleted)
    // can't be replayed onto the rewrite, so the first confirm reports it
    // inline and writes nothing; confirming again is a deliberate overwrite.
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    std::fs::write(
        &path,
        "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n",
    )
    .unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(
        &mut app,
        path.clone(),
        "# Review memory\n\n## Tests\n- Merged finding\n",
    );

    let hand_edited = "# Review memory\n\nHand-written note.\n\n## Tests\n- Stale one\n";
    std::fs::write(&path, hand_edited).unwrap();

    app.pr_review_compact_write().unwrap();

    match &app.mode {
        AppMode::ReviewMemoryCompactReview(state) => {
            assert!(
                state
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("changed on disk"))
            );
            assert!(state.overwrite_confirmed);
        }
        other => panic!(
            "expected the dialog to stay open, got {:?}",
            std::mem::discriminant(other)
        ),
    }
    assert_eq!(std::fs::read_to_string(&path).unwrap(), hand_edited);

    app.pr_review_compact_write().unwrap();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# Review memory\n\n## Tests\n- Merged finding\n"
    );
}

#[test]
fn pr_review_compact_discard_leaves_file_untouched() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("review-memory.md");
    let original = "# Review memory\n\n## Tests\n- Stale one\n- Stale two\n";
    std::fs::write(&path, original).unwrap();

    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_compact_review_for_test(&mut app, path.clone(), "# Review memory\n\n- edited away\n");

    app.pr_review_compact_discard();

    assert!(matches!(app.mode, AppMode::PrPicker(_)));
    let contents = std::fs::read_to_string(&path).unwrap();
    assert_eq!(contents, original);
}

#[test]
fn pr_review_fix_session_usage_reads_the_target_sessions_tokens() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    session.token_usage = Some(SessionTokenUsage {
        source: TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "s1".to_string(),
        },
        input_tokens: 1000,
        output_tokens: 500,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1500,
    });

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    let usage = app.pr_review_fix_session_usage();
    assert_eq!(usage.map(|u| u.total_tokens), Some(1500));
}

#[test]
fn pr_review_fix_session_usage_none_before_session_exists() {
    // Before the first `f`, the dedicated review session doesn't exist yet —
    // the header shouldn't show a stale or fabricated number.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert!(app.pr_review_fix_session_usage().is_none());
}

#[test]
fn pr_review_dedicated_session_status_is_scoped_to_its_session() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let other_session_id = store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Working session".to_string())
        .id
        .clone();
    let dedicated_session_id = store.projects[0].features[0]
        .add_session_named(
            SessionKind::Claude,
            crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
        )
        .id
        .clone();
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), Some(false));

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": tmux_session,
        "amf_feature_session_id": other_session_id,
    }));
    assert_eq!(
        app.pr_review_dedicated_session_working(),
        Some(false),
        "another agent window in the feature must not light the badge"
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));

    // Either activity signal independently keeps the exact session working.
    app.handle_ipc_message_value(serde_json::json!({
        "type": "tool-start",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-stop",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));

    app.handle_ipc_message_value(serde_json::json!({
        "type": "tool-stop",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(app.pr_review_dedicated_session_working(), Some(false));
}

#[test]
fn pr_review_dedicated_opencode_session_uses_sidebar_activity() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Opencode,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    session.set_token_usage_source_exact(TokenUsageSource {
        provider: TokenUsageProvider::Opencode,
        id: "opencode-triage".to_string(),
    });
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.opencode_sidebar_cache.insert(
        tmux_session,
        crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "opencode-triage".to_string(),
            status: Some("busy".to_string()),
            ..Default::default()
        },
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));
}

#[test]
fn pr_review_schedules_sidebar_load_for_dedicated_opencode_session() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Opencode,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    session.set_token_usage_source_exact(TokenUsageSource {
        provider: TokenUsageProvider::Opencode,
        id: "opencode-triage".to_string(),
    });
    let si = store.projects[0].features[0].sessions.len() - 1;
    let expected_signature = SidebarLoadRequest::from_opencode_session(
        &store.projects[0].features[0],
        &store.projects[0].features[0].sessions[si],
    )
    .signature();
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    app.schedule_sidebar_load_for_feature(0, 0);
    for _ in 0..20 {
        app.poll_sidebar_load_results();
        if !app.pending_sidebar_loads.contains(&tmux_session) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert_eq!(
        app.sidebar_load_signatures.get(&tmux_session),
        Some(&expected_signature)
    );
}

#[test]
fn pr_review_dedicated_pi_session_uses_thinking_marker() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(
        SessionKind::Pi,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    let tmux_session = format!("amf-pi-triage-test-{}", uuid::Uuid::new_v4());
    store.projects[0].features[0].tmux_session = tmux_session.clone();
    let marker = PathBuf::from("/tmp/amf-thinking").join(&tmux_session);
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, "").unwrap();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));

    std::fs::remove_file(marker).unwrap();
}

#[test]
fn pr_review_dedicated_session_status_is_absent_before_creation() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), None);
}

#[test]
fn pr_review_dedicated_session_status_is_absent_for_existing_live_target() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Working session".to_string());
    let session_id = session.id.clone();
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_target = crate::app::pr_review::FixTarget::ExistingLive;
        state.fix_target_picked = true;
    }
    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": tmux_session,
        "amf_feature_session_id": session_id,
    }));

    assert_eq!(
        app.pr_review_dedicated_session_working(),
        None,
        "existing-live activity must not be rendered as dedicated activity"
    );
}

#[test]
fn pr_review_triage_session_usage_reports_only_growth_since_baseline() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    let source = TokenUsageSource {
        provider: TokenUsageProvider::Claude,
        id: "triage-session".to_string(),
    };
    session.token_usage = Some(SessionTokenUsage {
        source: source.clone(),
        input_tokens: 1400,
        output_tokens: 300,
        cache_read_tokens: 200,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1900,
    });

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);
    let AppMode::PrReview(state) = &mut app.mode else {
        panic!("expected PR review pane");
    };
    state.usage_baselines.insert(
        source.clone(),
        SessionTokenUsage {
            source,
            input_tokens: 1000,
            output_tokens: 200,
            cache_read_tokens: 100,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            total_tokens: 1300,
        },
    );

    let usage = app.pr_review_triage_session_usage().unwrap();
    assert_eq!(usage.input_tokens, 400);
    assert_eq!(usage.output_tokens, 100);
    assert_eq!(usage.cache_read_tokens, 100);
    assert_eq!(usage.total_tokens, 600);
}

#[test]
fn pr_review_agent_draft_captures_harness_model_usage_and_cost() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    let source = TokenUsageSource {
        provider: TokenUsageProvider::Claude,
        id: "triage-session".to_string(),
    };
    let baseline = SessionTokenUsage {
        source: source.clone(),
        input_tokens: 1_000,
        output_tokens: 200,
        cache_read_tokens: 100,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1_300,
    };
    session.token_usage = Some(baseline.clone());

    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    let tmux_session = app.store.projects[0].features[0].tmux_session.clone();
    app.sidebar_model_cache
        .insert(tmux_session, "Model: claude-sonnet-4-6".to_string());
    let session_id = app.store.projects[0].features[0].sessions[0].id.clone();
    let usage = app.store.projects[0].features[0].sessions[0]
        .token_usage
        .as_mut()
        .unwrap();
    usage.input_tokens += 400;
    usage.output_tokens += 100;
    usage.cache_read_tokens += 100;
    usage.total_tokens += 600;
    // What `pr_review_inject_fix` records at injection time.
    persist_draft_with_provenance(
        &app,
        Some(&crate::app::pr_review::ReplyDraftProvenance {
            harness: "Claude".to_string(),
            session_id,
            model: None,
            usage_baseline: Some(baseline),
        }),
    );

    app.pr_review_open_reply_done();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    let metadata = state
        .reply
        .as_ref()
        .and_then(|reply| reply.generation_metadata.as_ref())
        .expect("captured drafts should disclose their generation details");
    assert_eq!(metadata.harness.as_deref(), Some("Claude"));
    assert_eq!(metadata.model.as_deref(), Some("claude-sonnet-4-6"));
    // Only the spend since the fix was injected, not the session's whole life.
    assert_eq!(metadata.estimated_tokens, Some(600));
    assert_eq!(metadata.estimated_cost.as_deref(), Some("<$0.01"));
    let _ = source;
}

/// Write a captured draft for PR 7 / comment 1 straight into the DB, with the
/// provenance a fix injection would have recorded alongside it.
fn persist_draft_with_provenance(
    app: &App,
    provenance: Option<&crate::app::pr_review::ReplyDraftProvenance>,
) {
    let encoded = provenance.map(|p| serde_json::to_string(p).unwrap());
    let db = app.db.as_ref().unwrap();
    db.begin_pr_comment_reply_draft(7, 1, "request-1", "sha", encoded.as_deref())
        .unwrap();
    assert!(
        db.capture_pr_comment_reply_draft(7, 1, "request-1", "Fixed the guard.")
            .unwrap()
    );
}

/// The reviewer's case: PR Triage is re-opened (which resets `fix_target` to the
/// default) and a *different* agent session is now what that target resolves to.
/// The disclosure has to keep naming the session that wrote the draft.
#[test]
fn pr_review_agent_draft_metadata_ignores_a_changed_fix_target() {
    let mut store = store_with_feature(ProjectStatus::Active);
    // The session the fix actually went to, and a second one the reset fix
    // target lands on instead.
    let drafting = store.projects[0].features[0].add_session_named(
        SessionKind::Codex,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    drafting.token_usage = Some(SessionTokenUsage {
        source: TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "drafting".to_string(),
        },
        input_tokens: 400,
        output_tokens: 100,
        cache_read_tokens: 100,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 600,
    });
    let drafting_id = drafting.id.clone();

    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    persist_draft_with_provenance(
        &app,
        Some(&crate::app::pr_review::ReplyDraftProvenance {
            harness: "Codex".to_string(),
            session_id: drafting_id,
            model: Some("gpt-5.5".to_string()),
            usage_baseline: None,
        }),
    );
    // Whatever the pane now points at is irrelevant: swap the session out for a
    // Claude one under the same triage label.
    app.store.projects[0].features[0].sessions[0].kind = SessionKind::Claude;
    app.store.projects[0].features[0].sessions[0].id = "some-other-session".to_string();

    app.pr_review_open_reply_done();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    let metadata = state
        .reply
        .as_ref()
        .and_then(|reply| reply.generation_metadata.as_ref())
        .expect("captured drafts should disclose their generation details");
    assert_eq!(metadata.harness.as_deref(), Some("Codex"));
    assert_eq!(metadata.model.as_deref(), Some("gpt-5.5"));
    // The drafting session is gone, so its usage is unknown — never the
    // replacement session's numbers.
    assert_eq!(metadata.estimated_tokens, None);
    assert_eq!(metadata.estimated_cost, None);
}

/// A draft persisted before AMF recorded provenance still discloses that it was
/// AI-generated; the details it cannot vouch for read as unreported.
#[test]
fn pr_review_agent_draft_without_provenance_discloses_unknown_details() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );

    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    persist_draft_with_provenance(&app, None);

    app.pr_review_open_reply_done();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    let reply = state.reply.as_ref().expect("expected a reply dialog");
    assert!(reply.agent_drafted);
    let metadata = reply
        .generation_metadata
        .as_ref()
        .expect("an agent draft always discloses, even with nothing to report");
    assert_eq!(metadata.harness, None);
    assert_eq!(
        metadata.source_disclosure(),
        "AI generation: harness unreported · model unreported"
    );
    assert_eq!(
        metadata.usage_disclosure(),
        "estimated tokens unavailable · Fix cost (est.): unavailable"
    );
}

#[test]
fn pr_review_triage_session_usage_hides_an_unchanged_baseline() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    let usage = SessionTokenUsage {
        source: TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "unchanged".to_string(),
        },
        input_tokens: 1000,
        output_tokens: 200,
        cache_read_tokens: 100,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1300,
    };
    session.token_usage = Some(usage.clone());

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);
    let AppMode::PrReview(state) = &mut app.mode else {
        panic!("expected PR review pane");
    };
    state.usage_baselines.insert(usage.source.clone(), usage);

    assert!(app.pr_review_triage_session_usage().is_none());
}

#[test]
fn pr_review_switching_to_live_target_ignores_its_earlier_usage() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let usage = SessionTokenUsage {
        source: TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "live-session".to_string(),
        },
        input_tokens: 800,
        output_tokens: 200,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1000,
    };
    store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Working".to_string())
        .token_usage = Some(usage.clone());

    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);

    // Pick "existing live session" from the fix-target picker (replaces the
    // old standalone `t` toggle).
    app.pr_review_open_fix_confirm();
    if let AppMode::PrReview(state) = &mut app.mode {
        let pick = state
            .harness_pick
            .as_mut()
            .expect("fix-target picker should be open");
        pick.selected = pick
            .rows
            .iter()
            .position(|r| matches!(r, crate::app::pr_review::FixTargetPickRow::ExistingLive(_)))
            .expect("existing-live row should be present");
    }
    app.pr_review_harness_pick_confirm();

    assert!(app.pr_review_triage_session_usage().is_none());

    let session = &mut app.store.projects[0].features[0].sessions[0];
    session.token_usage.as_mut().unwrap().input_tokens += 250;
    session.token_usage.as_mut().unwrap().total_tokens += 250;
    let delta = app.pr_review_triage_session_usage().unwrap();
    assert_eq!(delta.input_tokens, 250);
    assert_eq!(delta.total_tokens, 250);
}

#[test]
fn pr_review_first_fix_opens_harness_picker_then_confirm() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);

    // First `f`: no dedicated session and no harness chosen yet → the picker
    // opens; the fix confirm has NOT opened yet.
    app.pr_review_open_fix_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(
                state.harness_pick.is_some(),
                "harness picker should be open"
            );
            assert!(state.fix_confirm.is_none(), "fix confirm should wait");
            assert!(state.review_harness.is_none());
            // First row is always "existing live session"; default highlight
            // is the dedicated row for the project's preferred agent.
            let pick = state.harness_pick.as_ref().unwrap();
            assert_eq!(
                pick.rows[0],
                crate::app::pr_review::FixTargetPickRow::ExistingLive(None)
            );
            assert_eq!(
                pick.rows[pick.selected],
                crate::app::pr_review::FixTargetPickRow::Dedicated(AgentKind::default())
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    // Choosing the harness advances to the optional session-name step. A
    // second confirmation accepts the backwards-compatible default name and
    // continues into the fix confirm.
    app.pr_review_harness_pick_confirm();
    assert!(app.pr_review_harness_pick_naming());
    app.pr_review_harness_pick_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.harness_pick.is_none());
            assert_eq!(state.review_harness, Some(AgentKind::default()));
            assert_eq!(
                state.dedicated_session_label,
                crate::app::pr_review::TRIAGE_SESSION_LABEL
            );
            assert!(
                state.fix_confirm.is_some(),
                "fix confirm should now be open"
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_dedicated_target_accepts_a_custom_session_name() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);

    app.pr_review_open_fix_confirm();
    app.pr_review_harness_pick_confirm();
    assert!(app.pr_review_harness_pick_naming());
    for c in "PR 321 security".chars() {
        app.pr_review_harness_pick_name_push(c);
    }
    app.pr_review_harness_pick_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.dedicated_session_label, "PR 321 security");
            assert_eq!(state.review_harness, Some(AgentKind::default()));
            assert!(state.harness_pick.is_none());
            assert!(state.fix_confirm.is_some());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_dedicated_target_rejects_a_name_owned_by_another_harness() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "PR 321 security".to_string());
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_open_fix_confirm();
    if let AppMode::PrReview(state) = &mut app.mode {
        let pick = state.harness_pick.as_mut().unwrap();
        pick.selected = pick
            .rows
            .iter()
            .position(|row| {
                *row == crate::app::pr_review::FixTargetPickRow::Dedicated(AgentKind::Codex)
            })
            .unwrap();
    }
    app.pr_review_harness_pick_confirm();
    for c in "PR 321 security".chars() {
        app.pr_review_harness_pick_name_push(c);
    }
    app.pr_review_harness_pick_confirm();

    assert!(
        matches!(&app.mode, AppMode::PrReview(state)
            if state.harness_pick.is_some()
                && state.review_harness.is_none()
                && state.fix_confirm.is_none()),
        "the naming step should stay open after a harness collision"
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
fn pr_review_second_fix_skips_harness_picker() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);

    // Harness already chosen for this PR: `f` goes straight to the fix confirm.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review_harness = Some(AgentKind::Codex);
    }
    app.pr_review_open_fix_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(
                state.harness_pick.is_none(),
                "no picker on subsequent fixes"
            );
            assert!(state.fix_confirm.is_some());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_toggle_mark_adds_and_removes() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);

    // Mark the first comment, move down, mark the second.
    app.pr_review_toggle_mark();
    app.pr_review_select_next();
    app.pr_review_toggle_mark();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.marked.len(), 2);
            assert!(state.marked.contains(&1));
            assert!(state.marked.contains(&2));
        }
        _ => unreachable!(),
    }

    // Toggling the same comment again unmarks it.
    app.pr_review_toggle_mark();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.marked.len(), 1);
            assert!(!state.marked.contains(&2));
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_with_nothing_marked_hints() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);

    app.pr_review_open_batch_confirm();
    assert!(
        app.message.as_deref().unwrap_or("").contains("press space"),
        "expected a hint to mark comments, got {:?}",
        app.message
    );
    // No dialog opened.
    match &app.mode {
        AppMode::PrReview(state) => assert!(state.fix_confirm.is_none()),
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_opens_combined_dialog_for_marked() {
    // `pr_review_test_app` has no feature at /tmp/wd, so no harness pick is
    // needed — `B` builds the combined dialog directly.
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.marked.insert(1);
        state.marked.insert(3);
    }

    app.pr_review_open_batch_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("batch dialog should open");
            // The dialog carries both marked ids (list order).
            let mut ids = confirm.batch.clone().expect("dialog is a batch");
            ids.sort();
            assert_eq!(ids, vec![1, 3]);
            // One combined prompt, numbered, covering both files, unmarked one absent.
            let text = confirm.editor.text();
            assert!(text.starts_with("Address these PR review comments."));
            assert!(text.contains("Comment 1:") && text.contains("Comment 2:"));
            assert!(text.contains("File: src/file1.rs:1"));
            assert!(text.contains("File: src/file3.rs:3"));
            assert!(!text.contains("File: src/file2.rs"));
            assert_eq!(confirm.reply_draft_requests.len(), 2);
            assert_eq!(
                confirm
                    .reply_draft_requests
                    .iter()
                    .map(|request| request.comment_id)
                    .collect::<Vec<_>>(),
                vec![1, 3]
            );
            assert!(
                confirm
                    .reply_draft_requests
                    .iter()
                    .all(|request| request.base_head_sha == "sha")
            );
            assert!(text.contains("--comment-id 1 --request-id"));
            assert!(text.contains("--comment-id 3 --request-id"));
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_excludes_resolved_marks() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        // Mark both, but one is already resolved on GitHub.
        state.marked.insert(1);
        state.marked.insert(2);
        state.review.comments[0].is_resolved = true;
    }

    app.pr_review_open_batch_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("batch dialog should open");
            // Only the unresolved comment is included (token principle #6).
            assert_eq!(confirm.batch.clone().unwrap(), vec![2]);
            assert!(!confirm.editor.text().contains("File: src/file1.rs"));
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_confirm_all_resolved_hints() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.marked.insert(1);
        state.review.comments[0].is_resolved = true;
    }

    app.pr_review_open_batch_confirm();
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("all resolved"),
        "expected an all-resolved hint, got {:?}",
        app.message
    );
    match &app.mode {
        AppMode::PrReview(state) => assert!(state.fix_confirm.is_none()),
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_batch_routes_through_harness_pick_then_opens_combined() {
    // A dedicated-review PR with no session yet picks the harness before the
    // first fix — the batch flow must route the picker's continuation back to
    // the combined dialog (not the single-comment one).
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.marked.insert(1);
        state.marked.insert(2);
    }

    // `B`: picker opens, no dialog yet, and the pending action is the batch.
    app.pr_review_open_batch_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(
                state.harness_pick.is_some(),
                "harness picker should be open"
            );
            assert!(state.fix_confirm.is_none());
            assert!(state.pending_batch, "pending action should be the batch");
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    // Choosing the harness, then accepting the default session name, opens
    // the *combined* dialog and clears the flag.
    app.pr_review_harness_pick_confirm();
    assert!(app.pr_review_harness_pick_naming());
    app.pr_review_harness_pick_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(!state.pending_batch);
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("batch dialog should open");
            let mut ids = confirm.batch.clone().expect("dialog is a batch");
            ids.sort();
            assert_eq!(ids, vec![1, 2]);
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_cancel_harness_picker_aborts_fix() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_open_fix_confirm();
    assert!(app.pr_review_harness_picking());

    // Cancel: picker closes, nothing injected, harness stays unset so the next
    // `f` asks again.
    app.pr_review_harness_pick_cancel();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.harness_pick.is_none());
            assert!(state.fix_confirm.is_none());
            assert!(state.review_harness.is_none());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn pr_review_i_is_noop_for_comment_without_file_path() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    // Conversation/summary comments carry no path — nothing to highlight.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.comments[0].path = None;
    }

    app.open_syntax_language_picker_for_selected_diff_file();

    assert!(
        matches!(app.mode, AppMode::PrReview(_)),
        "expected to stay in the review pane, got {:?}",
        std::mem::discriminant(&app.mode)
    );
}

#[test]
fn pr_review_jump_sibling_cycles_within_the_batch_and_hints_otherwise() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 4);
    // Comments 1 and 3 (indices 0 and 2) were fixed in one combined batch.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.review.comments[0].batch_id = Some("batch-x".to_string());
        state.review.comments[2].batch_id = Some("batch-x".to_string());
        state.selected = 0;
    }

    app.pr_review_jump_sibling(true);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    assert_eq!(state.selected, 2, "forward jump lands on the other sibling");

    app.pr_review_jump_sibling(true);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    assert_eq!(state.selected, 0, "jump cycles back around");

    // A comment that isn't part of any batch: no move, and a hint toast.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
    }
    app.pr_review_jump_sibling(true);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    assert_eq!(
        state.selected, 1,
        "no sibling jump off a non-batched comment"
    );
}

/// Enter the review pane with comments built from `(id, path, author, is_bot)`
/// tuples, in that fetch order, for exercising `sort_mode`.
fn enter_pr_review_with_authors(app: &mut App, entries: &[(u64, &str, &str, bool)]) {
    let comments: Vec<crate::github::ReviewComment> = entries
        .iter()
        .map(|&(id, path, author, is_bot)| crate::github::ReviewComment {
            id,
            path: Some(path.to_string()),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("comment {id}"),
            user: crate::github::GhUser {
                login: author.to_string(),
                kind: if is_bot {
                    "Bot".to_string()
                } else {
                    "User".to_string()
                },
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let review = crate::app::pr_review::normalize(pr, comments, vec![], vec![], vec![]);
    app.mode = AppMode::PrReview(PrReviewState {
        workdir: std::path::PathBuf::from("/tmp/wd"),
        review,
        selected: 0,
        detail_scroll: 0,
        detail_content_lines: 0,
        hide_resolved: false,
        sort_mode: crate::app::pr_review::PrSortMode::default(),
        fix_target: crate::app::pr_review::FixTarget::default(),
        fix_target_picked: false,
        usage_baselines: std::collections::HashMap::new(),
        review_harness: None,
        dedicated_session_label: crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
        harness_pick: None,
        new_feature_setup: None,
        integrate: None,
        fix_confirm: None,
        fix_vim_enabled: false,
        mark_pick: None,
        reply_kind_pick: None,
        reply: None,
        memory_add: None,
        marked: std::collections::HashSet::new(),
        pending_batch: false,
        checked_out_branch: Some("main".to_string()),
        pending_ai_review_findings: 0,
        ai_review_last_run: None,
        investigations: Vec::new(),
        investigation_harness_pick: None,
        investigation_action_pick: None,
        investigation_follow_up: None,
        pending_follow_up: None,
        investigation_context: Default::default(),
    });
}

/// Enter the review pane with a mix of inline (code-anchored) comments and
/// top-level conversation comments, in fetch order: every id in `inline_ids`
/// first, then every id in `conversation_ids` — matching how `normalize`
/// orders review comments ahead of issue comments. For exercising
/// `PrSortMode::Conversations`.
fn enter_pr_review_with_conversation(app: &mut App, inline_ids: &[u64], conversation_ids: &[u64]) {
    let review_comments: Vec<crate::github::ReviewComment> = inline_ids
        .iter()
        .map(|&id| crate::github::ReviewComment {
            id,
            path: Some(format!("f{id}.rs")),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("inline {id}"),
            user: crate::github::GhUser {
                login: "alice".into(),
                kind: "User".into(),
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let issue_comments: Vec<crate::github::IssueComment> = conversation_ids
        .iter()
        .map(|&id| crate::github::IssueComment {
            id,
            body: format!("conversation {id}"),
            user: crate::github::GhUser {
                login: "bob".into(),
                kind: "User".into(),
            },
        })
        .collect();
    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let review =
        crate::app::pr_review::normalize(pr, review_comments, vec![], issue_comments, vec![]);
    app.mode = AppMode::PrReview(PrReviewState {
        workdir: std::path::PathBuf::from("/tmp/wd"),
        review,
        selected: 0,
        detail_scroll: 0,
        detail_content_lines: 0,
        hide_resolved: false,
        sort_mode: crate::app::pr_review::PrSortMode::default(),
        fix_target: crate::app::pr_review::FixTarget::default(),
        fix_target_picked: false,
        usage_baselines: std::collections::HashMap::new(),
        review_harness: None,
        dedicated_session_label: crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
        harness_pick: None,
        new_feature_setup: None,
        integrate: None,
        fix_confirm: None,
        fix_vim_enabled: false,
        mark_pick: None,
        reply_kind_pick: None,
        reply: None,
        memory_add: None,
        marked: std::collections::HashSet::new(),
        pending_batch: false,
        checked_out_branch: Some("main".to_string()),
        pending_ai_review_findings: 0,
        ai_review_last_run: None,
        investigations: Vec::new(),
        investigation_harness_pick: None,
        investigation_action_pick: None,
        investigation_follow_up: None,
        pending_follow_up: None,
        investigation_context: Default::default(),
    });
}

fn pr_review_ids_in_visible_order(app: &App) -> Vec<u64> {
    match &app.mode {
        AppMode::PrReview(state) => state
            .visible_indices()
            .iter()
            .map(|&i| state.review.comments[i].id)
            .collect(),
        _ => panic!("not in PrReview mode"),
    }
}

fn install_amf_outbound_comment_mix(app: &mut App) {
    use crate::app::pr_review::{
        AI_REVIEW_ATTRIBUTION_FOOTER, AMF_ATTRIBUTION_FOOTER, CommentKind,
    };

    enter_pr_review_with_conversation(app, &[], &[]);
    if let AppMode::PrReview(state) = &mut app.mode {
        let mut root = pr_comment_of_kind(1, CommentKind::Inline);
        root.author = "reviewer".into();

        let mut amf_reply = pr_comment_of_kind(2, CommentKind::Inline);
        amf_reply.author = "author".into();
        amf_reply.in_reply_to = Some(1);
        amf_reply.body = format!("Done in `abc123`.\n\n{AMF_ATTRIBUTION_FOOTER}");

        let mut ai_finding = pr_comment_of_kind(3, CommentKind::Inline);
        ai_finding.author = "author".into();
        ai_finding.body = format!("AI finding.\n\n{AI_REVIEW_ATTRIBUTION_FOOTER}");

        let mut human_reply = pr_comment_of_kind(4, CommentKind::Inline);
        human_reply.author = "second-reviewer".into();
        human_reply.in_reply_to = Some(1);
        human_reply.body = "This still needs a regression test.".into();

        let mut orphaned_amf_reply = pr_comment_of_kind(5, CommentKind::Inline);
        orphaned_amf_reply.author = "author".into();
        orphaned_amf_reply.in_reply_to = Some(999);
        orphaned_amf_reply.body = format!("Done elsewhere.\n\n{AMF_ATTRIBUTION_FOOTER}");

        state.review.comments = vec![root, amf_reply, ai_finding, human_reply, orphaned_amf_reply];
    }
}

#[test]
fn pr_review_collates_amf_followup_but_keeps_standalone_findings_actionable() {
    let mut app = pr_review_test_app();
    install_amf_outbound_comment_mix(&mut app);

    // The attributed inline reply is represented under comment 1's Replies
    // section, not as a duplicate row. A top-level AI finding stays fully
    // actionable; an orphaned follow-up stays visible for context; and the
    // unrelated human reply remains actionable.
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![1, 3, 4, 5]);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("not in PrReview mode");
    };
    assert_eq!(state.review.open_count(), 3);
    assert!(state.review.comments[3].is_actionable());
    assert!(state.review.comments[2].is_actionable());
    assert!(!state.review.comments[4].is_actionable());
}

#[test]
fn pr_review_fixes_standalone_amf_findings_but_not_followup_replies() {
    let mut app = pr_review_test_app();
    install_amf_outbound_comment_mix(&mut app);

    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 2; // top-level AI Review finding
    }
    app.pr_review_open_fix_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("standalone AI Review finding is fixable");
            assert!(confirm.editor.text().contains("AI finding."));
        }
        _ => panic!("not in PrReview mode"),
    }

    // Even a stale mark set assembled before refresh filters the orphaned
    // AMF follow-up out of the combined prompt, but retains the finding.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_confirm = None;
        state.marked.extend([3, 4, 5]);
    }
    app.pr_review_open_batch_confirm();
    let AppMode::PrReview(state) = &app.mode else {
        panic!("not in PrReview mode");
    };
    let confirm = state
        .fix_confirm
        .as_ref()
        .expect("standalone finding and human reply are batchable");
    assert_eq!(confirm.batch.as_deref(), Some(&[3, 4][..]));
    assert!(
        confirm
            .editor
            .text()
            .contains("This still needs a regression test.")
    );
    assert!(confirm.editor.text().contains("AI finding."));
    assert!(!confirm.editor.text().contains("Done elsewhere."));
}

#[test]
fn pr_review_cycle_sort_wraps_through_all_modes() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);

    use crate::app::pr_review::PrSortMode;
    let sort_mode = |app: &App| match &app.mode {
        AppMode::PrReview(state) => state.sort_mode,
        _ => panic!("not in PrReview mode"),
    };

    assert_eq!(sort_mode(&app), PrSortMode::FetchOrder);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::ByFile);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::ByAuthor);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::HumansFirst);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::Conversations);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::FetchOrder);
}

#[test]
fn pr_review_sort_by_file_groups_and_orders_paths() {
    let mut app = pr_review_test_app();
    // Fetch order: 1 (z.rs), 2 (a.rs), 3 (m.rs).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "z.rs", "alice", false),
            (2, "a.rs", "alice", false),
            (3, "m.rs", "alice", false),
        ],
    );
    app.pr_review_cycle_sort(); // FetchOrder -> ByFile
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 3, 1]);
}

#[test]
fn pr_review_sort_by_author_is_stable_within_ties() {
    let mut app = pr_review_test_app();
    // Fetch order: 1 (bob), 2 (alice), 3 (alice).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "a.rs", "bob", false),
            (2, "b.rs", "alice", false),
            (3, "c.rs", "alice", false),
        ],
    );
    app.pr_review_cycle_sort(); // FetchOrder -> ByFile
    app.pr_review_cycle_sort(); // ByFile -> ByAuthor
    // alice's two comments keep their fetch-order relative to each other.
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 3, 1]);
}

#[test]
fn pr_review_sort_humans_first_keeps_bots_last() {
    let mut app = pr_review_test_app();
    // Fetch order: 1 (bot), 2 (human), 3 (bot), 4 (human).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "a.rs", "coderabbit", true),
            (2, "b.rs", "alice", false),
            (3, "c.rs", "copilot", true),
            (4, "d.rs", "bob", false),
        ],
    );
    for _ in 0..3 {
        app.pr_review_cycle_sort(); // FetchOrder -> ByFile -> ByAuthor -> HumansFirst
    }
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 4, 1, 3]);
}

#[test]
fn pr_review_sort_conversations_groups_them_after_code_anchored_comments() {
    let mut app = pr_review_test_app();
    // Fetch order: inline 1, 2 then conversation 3, 4 — already matches the
    // desired grouping, so this proves the mode is a no-op here, not just
    // coincidentally correct because fetch order already separates them
    // (the interleaved case below is the real test).
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[3, 4]);
    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> ByFile -> ByAuthor -> HumansFirst -> Conversations
    }
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![1, 2, 3, 4]);
}

fn pr_comment_of_kind(
    id: u64,
    kind: crate::app::pr_review::CommentKind,
) -> crate::app::pr_review::PrComment {
    crate::app::pr_review::PrComment {
        id,
        kind,
        author: "someone".into(),
        is_bot: false,
        path: None,
        line: None,
        side: None,
        outdated: false,
        file_level: false,
        diff_hunk: None,
        body: format!("comment {id}"),
        snippet: format!("comment {id}"),
        in_reply_to: None,
        thread_id: None,
        is_resolved: false,
        triage: crate::app::pr_review::TriageState::Untriaged,
        local_note: None,
        batch_id: None,
        github_id: None,
        github_review_id: None,
    }
}

#[test]
fn pr_review_sort_conversations_reorders_interleaved_comments() {
    use crate::app::pr_review::CommentKind;

    let mut app = pr_review_test_app();
    enter_pr_review_with_conversation(&mut app, &[], &[]);
    if let AppMode::PrReview(state) = &mut app.mode {
        // Fetch order: 1 (inline), 3 (conversation), 2 (inline), 4 (conversation).
        state.review.comments = vec![
            pr_comment_of_kind(1, CommentKind::Inline),
            pr_comment_of_kind(3, CommentKind::Conversation),
            pr_comment_of_kind(2, CommentKind::Inline),
            pr_comment_of_kind(4, CommentKind::Conversation),
        ];
    }

    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> ByFile -> ByAuthor -> HumansFirst -> Conversations
    }
    // Every conversation comment moves after every inline one; relative order
    // within each group is preserved (stable sort).
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![1, 2, 3, 4]);
}

#[test]
fn pr_review_conversation_section_start_marks_where_the_group_begins() {
    let mut app = pr_review_test_app();
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[3, 4]);
    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> Conversations
    }
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PrReview");
    };
    assert_eq!(state.conversation_section_start(), Some(2));
}

#[test]
fn pr_review_conversation_section_start_is_none_outside_conversations_mode() {
    let mut app = pr_review_test_app();
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[3, 4]);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PrReview");
    };
    assert_eq!(state.conversation_section_start(), None);
}

#[test]
fn pr_review_conversation_section_start_is_none_without_both_groups() {
    let mut app = pr_review_test_app();
    // All inline, no conversation comments — nothing to separate.
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[]);
    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> Conversations
    }
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PrReview");
    };
    assert_eq!(state.conversation_section_start(), None);
}

#[test]
fn pr_review_toggle_resolved_snaps_using_current_sort_order() {
    let mut app = pr_review_test_app();
    // Fetch order 1,2,3 by id; sorted by file: 2 (a.rs), 3 (m.rs), 1 (z.rs).
    // Resolve comment 3 (the middle one in file-sort order).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "z.rs", "alice", false),
            (2, "a.rs", "alice", false),
            (3, "m.rs", "alice", false),
        ],
    );
    if let AppMode::PrReview(state) = &mut app.mode {
        // Mark comment 3 resolved directly on the model.
        state
            .review
            .comments
            .iter_mut()
            .find(|c| c.id == 3)
            .unwrap()
            .is_resolved = true;
    }
    app.pr_review_cycle_sort(); // FetchOrder -> ByFile: order is [2, 3, 1]
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = state
            .review
            .comments
            .iter()
            .position(|c| c.id == 3)
            .unwrap();
    }

    app.pr_review_toggle_resolved();
    // Comment 3 is hidden; the visible file-sort order is now [2, 1].
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 1]);
    match &app.mode {
        AppMode::PrReview(state) => {
            let selected_id = state.review.comments[state.selected].id;
            assert_eq!(selected_id, 1);
        }
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn pr_review_open_without_selection_shows_message() {
    // No projects/features selected, so opening should not enter a PR mode.
    let mut app = pr_review_test_app();
    app.open_pr_review();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.message.is_some());
}

#[test]
fn feature_for_view_resolves_by_project_and_feature_name() {
    let store = store_with_feature(ProjectStatus::Active);
    let app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let view = view_state_for("my-project", "my-feat");

    let feature = app.feature_for_view(&view).expect("feature should resolve");
    assert_eq!(feature.id, "feat-1");
}

#[test]
fn feature_for_view_returns_none_for_unknown_feature() {
    let store = store_with_feature(ProjectStatus::Active);
    let app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let view = view_state_for("my-project", "does-not-exist");

    assert!(app.feature_for_view(&view).is_none());
}

#[test]
fn open_pr_review_from_view_is_noop_outside_viewing_mode() {
    // Leader commands only fire while `Viewing`, but guard the entry point
    // itself in case something else ever calls it from another mode.
    let mut app = pr_review_test_app();
    app.mode = AppMode::Normal;

    app.open_pr_review_from_view();

    assert!(matches!(app.mode, AppMode::Normal));
    assert!(app.message.is_none());
}

#[test]
fn open_pr_review_from_view_shows_message_for_unresolvable_feature() {
    // The session view names a project/feature that no longer exists in the
    // store — resolve must fail gracefully rather than panicking or spending
    // a `gh` call on a bogus workdir.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(view_state_for("my-project", "does-not-exist"));

    app.open_pr_review_from_view();

    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert_eq!(app.message.as_deref(), Some("No active feature to review"));
}

#[test]
fn dedicated_review_session_working_for_workdir_reads_outside_pr_review_mode() {
    // The ambient Viewing-mode badge calls this while `self.mode` is
    // `Viewing`, not `PrReview` — unlike `pr_review_dedicated_session_working`,
    // it must not depend on the pane being open.
    let mut store = store_with_feature(ProjectStatus::Active);
    let dedicated_session_id = store.projects[0].features[0]
        .add_session_named(
            SessionKind::Claude,
            crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
        )
        .id
        .clone();
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let workdir = store.projects[0].features[0].workdir.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.mode = AppMode::Viewing(view_state_for("my-project", "my-feat"));

    assert_eq!(
        app.dedicated_review_session_working_for_workdir(&workdir),
        Some(false)
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(
        app.dedicated_review_session_working_for_workdir(&workdir),
        Some(true)
    );
}

#[test]
fn ai_review_running_for_workdir_matches_the_pending_reviews_workdir() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let workdir = origin.workdir.clone();
    let other_workdir = PathBuf::from("/tmp/other-workdir");

    // No background job yet.
    assert!(!app.ai_review_running_for_workdir(&workdir));

    let (_tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin));

    assert!(app.ai_review_running_for_workdir(&workdir));
    assert!(!app.ai_review_running_for_workdir(&other_workdir));
}

/// Enter the review pane with comments 1..=n where the given ids are resolved.
fn enter_pr_review_with_resolved(app: &mut App, n: u64, resolved: &[u64]) {
    let comments: Vec<crate::github::ReviewComment> = (1..=n)
        .map(|id| crate::github::ReviewComment {
            id,
            path: Some(format!("src/file{id}.rs")),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("comment {id}"),
            user: crate::github::GhUser {
                login: "alice".to_string(),
                kind: "User".to_string(),
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let threads: Vec<crate::github::ReviewThread> = resolved
        .iter()
        .map(|&id| crate::github::ReviewThread {
            id: format!("T{id}"),
            is_resolved: true,
            comment_ids: vec![id],
        })
        .collect();
    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let review = crate::app::pr_review::normalize(pr, comments, vec![], vec![], threads);
    app.mode = AppMode::PrReview(PrReviewState {
        workdir: std::path::PathBuf::from("/tmp/wd"),
        review,
        selected: 0,
        detail_scroll: 0,
        detail_content_lines: 0,
        hide_resolved: false,
        sort_mode: crate::app::pr_review::PrSortMode::default(),
        fix_target: crate::app::pr_review::FixTarget::default(),
        fix_target_picked: false,
        usage_baselines: std::collections::HashMap::new(),
        review_harness: None,
        dedicated_session_label: crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
        harness_pick: None,
        new_feature_setup: None,
        integrate: None,
        fix_confirm: None,
        fix_vim_enabled: false,
        mark_pick: None,
        reply_kind_pick: None,
        reply: None,
        memory_add: None,
        marked: std::collections::HashSet::new(),
        pending_batch: false,
        checked_out_branch: Some("main".to_string()),
        pending_ai_review_findings: 0,
        ai_review_last_run: None,
        investigations: Vec::new(),
        investigation_harness_pick: None,
        investigation_action_pick: None,
        investigation_follow_up: None,
        pending_follow_up: None,
        investigation_context: Default::default(),
    });
}

#[test]
fn pr_review_hide_resolved_skips_resolved_comments() {
    // Comments 1,2,3; comment 2 is resolved on GitHub.
    let mut app = pr_review_test_app();
    enter_pr_review_with_resolved(&mut app, 3, &[2]);

    app.pr_review_toggle_resolved();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.hide_resolved);
            assert_eq!(state.visible_indices(), vec![0, 2]);
            assert_eq!(state.hidden_resolved_count(), 1);
        }
        _ => panic!("not in PrReview mode"),
    }

    // Navigation jumps over the hidden (resolved) comment at index 1.
    assert_eq!(pr_review_selected(&app), 0);
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 2);
    app.pr_review_select_prev();
    assert_eq!(pr_review_selected(&app), 0);
}

#[test]
fn pr_review_toggle_snaps_selection_off_hidden_comment() {
    let mut app = pr_review_test_app();
    enter_pr_review_with_resolved(&mut app, 3, &[2]);

    // Park the selection on the resolved comment, then hide resolved.
    app.pr_review_select_next();
    assert_eq!(pr_review_selected(&app), 1);
    app.pr_review_toggle_resolved();

    // Index 1 is now hidden, so selection snaps to the next visible comment.
    assert_eq!(pr_review_selected(&app), 2);
}

#[test]
fn pr_review_detail_scroll_clamps() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1); // single short comment

    // Scrolling up at the top is a no-op.
    app.pr_review_scroll_detail_up(5);
    match &app.mode {
        AppMode::PrReview(state) => assert_eq!(state.detail_scroll, 0),
        _ => panic!("not in PrReview mode"),
    }

    // Render once so the detail pane records how many lines it actually drew;
    // the scroll-down clamp bounds against that recorded count.
    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    terminal.draw(|f| crate::ui::draw(f, &mut app)).unwrap();

    // Scrolling far down clamps to the rendered line count, never past it.
    app.pr_review_scroll_detail_down(1000);
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.detail_content_lines > 0);
            assert_eq!(state.detail_scroll, state.detail_content_lines - 1);
        }
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn pr_review_selection_change_resets_detail_scroll() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 3);

    app.pr_review_scroll_detail_down(2);
    app.pr_review_select_next();
    match &app.mode {
        AppMode::PrReview(state) => assert_eq!(state.detail_scroll, 0),
        _ => panic!("not in PrReview mode"),
    }
}

fn pr_review_fix_confirm(app: &App) -> &crate::app::FixConfirmState {
    match &app.mode {
        AppMode::PrReview(state) => state
            .fix_confirm
            .as_ref()
            .expect("fix confirm dialog should be open"),
        _ => panic!("not in PrReview mode"),
    }
}

#[test]
fn pr_review_open_fix_confirm_seeds_editor_from_selection() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);

    assert_eq!(app.pr_review_fix_editing(), None);
    app.pr_review_open_fix_confirm();

    // Dialog opens in confirm (not edit) mode, seeded with the comment's prompt.
    assert_eq!(app.pr_review_fix_editing(), Some(false));
    let confirm = pr_review_fix_confirm(&app);
    let expected = match &app.mode {
        AppMode::PrReview(state) => state.selected_comment().unwrap().fix_prompt(),
        _ => unreachable!(),
    };
    assert!(confirm.editor.text().starts_with(&expected));
    assert!(
        confirm
            .editor
            .text()
            .contains("Address this PR review comment.")
    );
    assert!(
        confirm
            .editor
            .text()
            .contains("Do not post replies to GitHub")
    );
    assert!(
        confirm
            .editor
            .text()
            .contains("amf reply-draft --pr-number 7 --comment-id 1 --request-id")
    );
    assert_eq!(confirm.reply_draft_requests.len(), 1);
    assert_eq!(confirm.reply_draft_requests[0].comment_id, 1);
    assert_eq!(confirm.reply_draft_requests[0].base_head_sha, "sha");
    assert!(
        confirm
            .editor
            .text()
            .contains(&confirm.reply_draft_requests[0].request_id)
    );
}

#[test]
fn pr_review_fix_edit_mode_forwards_keys_and_cancel_closes() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_fix_confirm();

    // Keys are ignored until edit mode is entered.
    assert!(!app.pr_review_fix_editor_key(KeyEvent::from(KeyCode::Char('Z'))));
    let before = pr_review_fix_confirm(&app).editor.text().to_string();

    app.pr_review_fix_edit();
    assert_eq!(app.pr_review_fix_editing(), Some(true));
    assert!(app.pr_review_fix_editor_key(KeyEvent::from(KeyCode::Char('Z'))));
    assert_eq!(
        pr_review_fix_confirm(&app).editor.text(),
        format!("{before}Z")
    );

    // Leaving edit mode keeps the edited text; cancel closes the dialog.
    app.pr_review_fix_stop_edit();
    assert_eq!(app.pr_review_fix_editing(), Some(false));
    app.pr_review_cancel_fix();
    assert_eq!(app.pr_review_fix_editing(), None);
}

#[test]
fn pr_review_fix_vim_toggle_persists_across_reopen() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    // Default keymap is plain.
    app.pr_review_open_fix_confirm();
    assert!(app.pr_review_fix_vim_mode().is_none());

    // Toggling vim flips the editor and the remembered pane preference.
    app.pr_review_fix_toggle_vim();
    assert!(app.pr_review_fix_vim_mode().is_some());
    match &app.mode {
        AppMode::PrReview(state) => assert!(state.fix_vim_enabled),
        _ => unreachable!(),
    }

    // Closing and reopening the dialog (e.g. for another comment) keeps vim on.
    app.pr_review_cancel_fix();
    app.pr_review_open_fix_confirm();
    assert!(
        app.pr_review_fix_vim_mode().is_some(),
        "reopened dialog should remember the vim choice"
    );

    // Toggling back off is likewise remembered.
    app.pr_review_fix_toggle_vim();
    app.pr_review_cancel_fix();
    app.pr_review_open_fix_confirm();
    assert!(app.pr_review_fix_vim_mode().is_none());
}

#[test]
fn pr_review_fix_scroll_moves_offset_and_clears_cursor_follow() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_fix_confirm();

    app.pr_review_fix_scroll(3);
    let confirm = pr_review_fix_confirm(&app);
    assert_eq!(confirm.scroll, 3);
    assert!(
        !confirm.sync_to_cursor,
        "an explicit scroll should stop following the cursor"
    );

    // Scrolling up saturates at zero rather than underflowing.
    app.pr_review_fix_scroll(-10);
    assert_eq!(pr_review_fix_confirm(&app).scroll, 0);
}

#[test]
fn pr_review_fix_confirm_scrolls_before_editing() {
    use crossterm::event::{KeyCode, KeyModifiers};

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_fix_confirm();

    crate::handlers::handle_pr_review_key(
        &mut app,
        KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE),
    )
    .unwrap();
    assert_eq!(pr_review_fix_confirm(&app).scroll, 10);
    assert!(!pr_review_fix_confirm(&app).sync_to_cursor);

    crate::handlers::handle_pr_review_key(
        &mut app,
        KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL),
    )
    .unwrap();
    assert_eq!(pr_review_fix_confirm(&app).scroll, 9);
}

#[test]
fn pr_review_fix_confirm_can_change_target_without_losing_edits() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    app.pr_review_harness_pick_confirm();
    app.pr_review_harness_pick_confirm();
    app.pr_review_fix_edit();
    assert!(app.pr_review_fix_editor_key(KeyEvent::from(KeyCode::Char('Z'))));
    app.pr_review_fix_stop_edit();
    let prompt = pr_review_fix_confirm(&app).editor.text().to_string();

    app.pr_review_change_fix_target();
    assert!(app.pr_review_harness_picking());
    app.pr_review_harness_pick_move(-1); // Dedicated default -> existing live.
    app.pr_review_harness_pick_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.fix_target,
                crate::app::pr_review::FixTarget::ExistingLive
            );
            assert_eq!(state.fix_confirm.as_ref().unwrap().editor.text(), prompt);
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_reopened_target_picker_highlights_current_target() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    app.pr_review_harness_pick_confirm();
    app.pr_review_harness_pick_confirm();

    // Redirect the fix to the existing live session, then reopen the picker
    // the way "review and redirect" does.
    app.pr_review_change_fix_target();
    app.pr_review_harness_pick_move(-1); // Dedicated default -> existing live (row 0).
    app.pr_review_harness_pick_confirm();
    app.pr_review_change_fix_target();

    match &app.mode {
        AppMode::PrReview(state) => {
            let pick = state.harness_pick.as_ref().expect("picker reopened");
            assert_eq!(
                pick.selected, 0,
                "reopened picker should highlight the current ExistingLive target, not the dedicated default"
            );
        }
        _ => unreachable!(),
    }
}

fn reply_editor_text(app: &App) -> String {
    match &app.mode {
        AppMode::PrReview(state) => state.reply.as_ref().unwrap().editor.text().to_string(),
        _ => unreachable!(),
    }
}

fn reply_is_agent_drafted(app: &App) -> bool {
    match &app.mode {
        AppMode::PrReview(state) => state.reply.as_ref().unwrap().agent_drafted,
        _ => unreachable!(),
    }
}

fn reply_effective_agent_drafted(app: &App) -> bool {
    match &app.mode {
        AppMode::PrReview(state) => {
            crate::app::pr_review::reply_effective_agent_drafted(state.reply.as_ref().unwrap())
        }
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_open_reply_not_needed_starts_in_edit_mode() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_reply_not_needed();
    // The not-needed reply needs a typed reason, so it opens straight into edit
    // mode with an empty buffer.
    assert_eq!(app.pr_review_reply_view(), Some(true));
    assert_eq!(reply_editor_text(&app), "");
}

#[test]
fn pr_review_open_reply_done_seeds_template_in_confirm_view() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_reply_done();
    // The done reply is post-ready (a template), so it opens in the confirm
    // view. The workdir isn't a git repo in tests, so it falls back to "Done.".
    assert_eq!(app.pr_review_reply_view(), Some(false));
    assert_eq!(reply_editor_text(&app), "Done.");
}

#[test]
fn pr_review_reply_done_prefers_the_agent_draft() {
    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review(&mut app, 1);

    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", "sha", None)
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Updated the lock scope and added a regression test."
    }));

    app.pr_review_open_reply_done();
    assert_eq!(
        reply_editor_text(&app),
        "Updated the lock scope and added a regression test."
    );
    assert!(reply_is_agent_drafted(&app));
    assert_eq!(app.pr_review_reply_view(), Some(false));
}

#[test]
fn pr_review_reply_not_needed_ignores_a_captured_fix_draft() {
    // A captured draft is the fixing agent's description of what it *did* —
    // not a rationale for why a fix isn't needed. NotNeeded must never seed
    // from it, or a single confirm keystroke would post the fix summary as
    // the not-needed explanation.
    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review(&mut app, 1);

    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", "sha", None)
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Updated the lock scope and added a regression test."
    }));

    app.pr_review_open_reply_not_needed();
    assert_eq!(reply_editor_text(&app), "");
    assert!(!reply_is_agent_drafted(&app));
    assert_eq!(
        app.pr_review_reply_view(),
        Some(true),
        "not-needed always starts in edit mode so the user types their own reason"
    );
}

#[test]
fn reply_draft_toast_context_uses_the_cached_comments_path_and_snippet() {
    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());

    let review = pr_review_with_comments(1);
    app.db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&review)
        .unwrap();
    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", "sha", None)
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Fixed it."
    }));

    // A toast fired while the user is elsewhere (dashboard, unrelated tmux
    // view) needs more than bare numbers to mean anything — pull the file
    // path from the review cached at fix-injection time.
    let context = app.reply_draft_toast_context(7, 1).unwrap();
    assert!(context.contains("src/file1.rs"));
}

#[test]
fn reply_draft_toast_context_none_without_a_cached_review() {
    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", "sha", None)
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Fixed it."
    }));

    // No review was ever cached for PR #7 / head "sha" — falls back to
    // `None` so the caller uses the bare-numbers message instead of panicking
    // or showing a broken context string.
    assert_eq!(app.reply_draft_toast_context(7, 1), None);
}

#[test]
fn pr_review_reply_draft_ipc_ignores_an_expired_request() {
    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "current", "sha", None)
        .unwrap();

    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "expired",
        "body": "Stale reply"
    }));

    assert_eq!(
        app.db
            .as_ref()
            .unwrap()
            .load_pr_comment_reply_draft(7, 1)
            .unwrap(),
        None
    );
    assert!(app.toasts.is_empty(), "stale drafts are ignored quietly");
}

#[test]
fn pr_review_reply_pick_opens_defaulting_to_done_and_confirm_routes_to_it() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_reply_pick();
    assert!(app.pr_review_reply_pick_picking());

    // Confirming without moving picks the default (first) row, `Done`.
    app.pr_review_reply_pick_confirm();
    assert!(!app.pr_review_reply_pick_picking());
    assert_eq!(app.pr_review_reply_view(), Some(false));
    assert_eq!(reply_editor_text(&app), "Done.");
}

#[test]
fn pr_review_reply_pick_move_and_confirm_routes_to_not_needed() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_reply_pick();
    app.pr_review_reply_pick_move(1);
    app.pr_review_reply_pick_confirm();

    assert!(!app.pr_review_reply_pick_picking());
    // Not-needed opens straight into edit mode with an empty buffer.
    assert_eq!(app.pr_review_reply_view(), Some(true));
    assert_eq!(reply_editor_text(&app), "");
}

#[test]
fn pr_review_reply_pick_cancel_closes_without_opening_a_reply() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_reply_pick();
    app.pr_review_reply_pick_cancel();

    assert!(!app.pr_review_reply_pick_picking());
    assert_eq!(app.pr_review_reply_view(), None);
}

#[test]
fn pr_review_mark_pick_confirm_applies_the_chosen_action() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    // Row 0 is `Done`.
    app.pr_review_open_mark_pick();
    assert!(app.pr_review_mark_pick_picking());
    app.pr_review_mark_pick_confirm();
    assert!(!app.pr_review_mark_pick_picking());
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.selected_comment().unwrap().triage,
                crate::app::pr_review::TriageState::Done
            );
        }
        _ => panic!("expected PrReview"),
    }

    // Row 1 (`Skip`) toggles it back off Done and onto Skipped.
    app.pr_review_open_mark_pick();
    app.pr_review_mark_pick_move(1);
    app.pr_review_mark_pick_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.selected_comment().unwrap().triage,
                crate::app::pr_review::TriageState::Skipped
            );
        }
        _ => panic!("expected PrReview"),
    }
}

#[test]
fn pr_review_mark_pick_cancel_leaves_triage_untouched() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_mark_pick();
    app.pr_review_mark_pick_cancel();

    assert!(!app.pr_review_mark_pick_picking());
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.selected_comment().unwrap().triage,
                crate::app::pr_review::TriageState::Untriaged
            );
        }
        _ => panic!("expected PrReview"),
    }
}

#[test]
fn pr_review_open_reply_done_seeds_the_commit_that_touched_the_comments_line() {
    let repo = TempDir::new().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    let file = repo.path().join("src/file1.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "line1\nline2\nline3\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "first"]);
    std::fs::write(&file, "line1\nFIXED\nline3\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "second"]);
    let fix_sha = git(&["rev-parse", "--short", "HEAD"]);

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo.path().to_path_buf();
        // `pr_review_with_comments` seeds comment 1 at src/file1.rs:1; point it
        // at the line the second commit actually changed.
        state.review.comments[0].line = Some(2);
    }

    app.pr_review_open_reply_done();
    assert_eq!(reply_editor_text(&app), format!("Done in `{fix_sha}`."));
    assert!(!reply_is_agent_drafted(&app));
}

#[test]
fn pr_review_agent_draft_includes_the_post_injection_commit_that_touched_the_file() {
    let repo = TempDir::new().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    let file = repo.path().join("src/file1.rs");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "line1\nreturn value\nline3\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "first"]);
    let base_sha = git(&["rev-parse", "HEAD"]);
    // The fix inserts a guard beside the unchanged commented line. Plain
    // line history would cite the first commit; the recorded injection head
    // makes the later file-touching commit unambiguous.
    std::fs::write(&file, "line1\nguard\nreturn value\nline3\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "second"]);
    let fix_sha = git(&["rev-parse", "--short", "HEAD"]);

    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo.path().to_path_buf();
        state.review.comments[0].line = Some(3);
    }
    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", &base_sha, None)
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Guarded zero divisors and added regression coverage."
    }));

    app.pr_review_open_reply_done();

    assert_eq!(
        reply_editor_text(&app),
        format!("Guarded zero divisors and added regression coverage.\n\nDone in `{fix_sha}`.")
    );
    assert!(reply_is_agent_drafted(&app));
}

#[test]
fn pr_review_open_reply_done_flags_a_bare_head_fallback() {
    let repo = TempDir::new().unwrap();
    let git = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };
    git(&["init", "-q"]);
    git(&["config", "user.email", "test@example.com"]);
    git(&["config", "user.name", "Test"]);
    std::fs::write(repo.path().join("README.md"), "hello\n").unwrap();
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
    let head_sha = git(&["rev-parse", "--short", "HEAD"]);

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo.path().to_path_buf();
        // Comment's path was never committed here — no line/file history to
        // find, so the reply falls back to bare HEAD with a caveat.
    }

    app.pr_review_open_reply_done();
    assert_eq!(
        reply_editor_text(&app),
        format!("Done in `{head_sha}` (latest commit).")
    );
}

#[test]
fn pr_review_reply_edit_forwards_keys_and_cancel_closes() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_reply_done();

    // Keys are ignored until edit mode is entered.
    let before = reply_editor_text(&app);
    app.pr_review_reply_editor_key(KeyEvent::from(KeyCode::Char('Z')));
    assert_eq!(reply_editor_text(&app), before);

    app.pr_review_reply_edit();
    assert_eq!(app.pr_review_reply_view(), Some(true));
    app.pr_review_reply_editor_key(KeyEvent::from(KeyCode::Char('!')));
    assert_eq!(reply_editor_text(&app), format!("{before}!"));

    app.pr_review_reply_stop_edit();
    assert_eq!(app.pr_review_reply_view(), Some(false));
    app.pr_review_cancel_reply();
    assert_eq!(app.pr_review_reply_view(), None);
}

#[test]
fn pr_review_editing_a_captured_draft_drops_ai_attribution() {
    use crossterm::event::{KeyCode, KeyEvent};

    let db_dir = TempDir::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review(&mut app, 1);

    app.db
        .as_ref()
        .unwrap()
        .begin_pr_comment_reply_draft(7, 1, "request-1", "sha", None)
        .unwrap();
    app.handle_ipc_message_value(serde_json::json!({
        "type": "pr-reply-draft",
        "pr_number": 7,
        "comment_id": 1,
        "draft_request_id": "request-1",
        "body": "Updated the lock scope and added a regression test."
    }));

    app.pr_review_open_reply_done();
    assert!(reply_is_agent_drafted(&app));
    assert!(reply_effective_agent_drafted(&app));

    app.pr_review_reply_edit();
    app.pr_review_reply_editor_key(KeyEvent::from(KeyCode::Char('!')));
    app.pr_review_reply_stop_edit();

    // `agent_drafted` stays true — it's a historical fact about how the reply
    // was seeded — but the *effective* attribution (what actually gets
    // posted) drops to AMF's channel-only footer now that the user has
    // changed the agent's words.
    assert!(reply_is_agent_drafted(&app));
    assert!(!reply_effective_agent_drafted(&app));
}

#[test]
fn pr_review_post_empty_reply_is_rejected() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    // Not-needed opens empty; posting without a reason is rejected (no write).
    app.pr_review_open_reply_not_needed();

    app.pr_review_post_reply().unwrap();
    assert_eq!(app.pr_review_reply_view(), Some(true));
    assert!(app.message.is_some());
}

fn memory_add_editor_text(app: &App) -> String {
    match &app.mode {
        AppMode::PrReview(state) => state.memory_add.as_ref().unwrap().editor.text().to_string(),
        _ => unreachable!(),
    }
}

fn memory_add_category(app: &App) -> usize {
    match &app.mode {
        AppMode::PrReview(state) => state.memory_add.as_ref().unwrap().category,
        _ => unreachable!(),
    }
}

#[test]
fn pr_review_open_memory_add_seeds_finding_with_file_hint_and_default_category() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    // Post-ready like the "done" reply template: opens in the confirm view,
    // not straight into editing.
    assert_eq!(app.pr_review_memory_add_view(), Some(false));
    assert_eq!(memory_add_editor_text(&app), "comment 1 (src/file1.rs:1)");
    assert_eq!(memory_add_category(&app), 0, "defaults to General");
}

#[test]
fn pr_review_open_memory_add_without_comments_shows_message() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 0);

    app.pr_review_open_memory_add();
    assert_eq!(app.pr_review_memory_add_view(), None);
    assert!(app.message.is_some());
}

#[test]
fn pr_review_memory_add_edit_forwards_keys_and_cancel_closes() {
    use crossterm::event::{KeyCode, KeyEvent};

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    // Keys are ignored until edit mode is entered.
    let before = memory_add_editor_text(&app);
    app.pr_review_memory_add_editor_key(KeyEvent::from(KeyCode::Char('Z')));
    assert_eq!(memory_add_editor_text(&app), before);

    app.pr_review_memory_add_edit();
    assert_eq!(app.pr_review_memory_add_view(), Some(true));
    app.pr_review_memory_add_editor_key(KeyEvent::from(KeyCode::Char('!')));
    assert_eq!(memory_add_editor_text(&app), format!("{before}!"));

    app.pr_review_memory_add_stop_edit();
    assert_eq!(app.pr_review_memory_add_view(), Some(false));
    app.pr_review_cancel_memory_add();
    assert_eq!(app.pr_review_memory_add_view(), None);
}

#[test]
fn pr_review_cycle_memory_category_wraps() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    let n = crate::app::pr_review::MEMORY_CATEGORIES.len();
    for i in 1..=n {
        app.pr_review_cycle_memory_category();
        assert_eq!(memory_add_category(&app), i % n);
    }
}

#[test]
fn pr_review_append_empty_finding_is_rejected() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().editor = crate::editor::TextEditor::new(String::new());
    }

    // No `expect_repo_root()` was set on the mock: the empty check short-
    // circuits before the doc path is ever resolved.
    app.pr_review_append_memory().unwrap();
    assert_eq!(
        app.pr_review_memory_add_view(),
        Some(false),
        "dialog stays open"
    );
    assert!(app.message.is_some());
}

#[test]
fn pr_review_append_memory_writes_and_dedups() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(2)
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    app.pr_review_append_memory().unwrap();
    assert_eq!(
        app.pr_review_memory_add_view(),
        None,
        "dialog closes once appended"
    );

    let doc_path = repo.join(".amf").join("review-memory.md");
    let contents = std::fs::read_to_string(&doc_path).unwrap();
    assert!(contents.contains("## General"));
    assert!(contents.contains("- comment 1 (src/file1.rs:1)"));

    // Re-adding the same finding is a dedup no-op, not a duplicate bullet.
    app.pr_review_open_memory_add();
    app.pr_review_append_memory().unwrap();
    let contents_after = std::fs::read_to_string(&doc_path).unwrap();
    assert_eq!(
        contents_after.matches("comment 1 (src/file1.rs:1)").count(),
        1,
        "dedup should skip the second append"
    );
}

#[test]
fn pr_review_append_memory_honors_project_review_memory_path_override() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().to_path_buf();

    // A project-level override in `{repo}/.amf/config.json` should redirect
    // the append away from the default `.amf/review-memory.md`.
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("config.json"),
        r#"{"review_memory_path": ".amf/team-review-memory.md"}"#,
    )
    .unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(1)
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    app.pr_review_append_memory().unwrap();

    let default_path = repo.join(".amf").join("review-memory.md");
    assert!(
        !default_path.exists(),
        "the default path should be untouched when a project override is set"
    );
    let overridden_path = repo.join(".amf").join("team-review-memory.md");
    let contents = std::fs::read_to_string(&overridden_path).unwrap();
    assert!(contents.contains("- comment 1 (src/file1.rs:1)"));
}

#[test]
fn pr_review_memory_add_defaults_to_project_scope_and_toggles() {
    use crate::app::review_memory::MemoryScope;

    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    let scope = |app: &App| match &app.mode {
        AppMode::PrReview(state) => state.memory_add.as_ref().unwrap().scope,
        _ => panic!("expected PrReview"),
    };
    assert_eq!(
        scope(&app),
        MemoryScope::Project,
        "a finding from this PR is about this repo until the user says otherwise"
    );

    app.pr_review_toggle_memory_scope();
    assert_eq!(scope(&app), MemoryScope::Global);
    app.pr_review_toggle_memory_scope();
    assert_eq!(scope(&app), MemoryScope::Project);
}

#[test]
fn pr_review_memory_ai_summary_single_harness_skips_picker_and_gates_the_run() {
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|p| Ok(p.to_path_buf()));
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![AgentKind::Claude],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    app.pr_review_open_memory_ai_summary_pick();

    assert!(
        !app.pr_review_memory_ai_summary_picking(),
        "exactly one harness auto-launches, no picker"
    );
    match &app.mode {
        AppMode::PromptPrecall(pending) => {
            assert_eq!(
                pending.prompt_id,
                crate::prompts::PromptId::ReviewMemoryAiSummary
            );
            assert_eq!(pending.harness, AgentKind::Claude);
            match pending.prior_mode.as_ref() {
                AppMode::PrReview(state) => {
                    // `ai_summary` is not switched to `Generating` until the
                    // gate clears, so the stashed mode still shows the
                    // untouched confirm view — a cancel must not leave the
                    // dialog stuck on "Generating" for a run that never
                    // started.
                    assert!(
                        state.memory_add.as_ref().unwrap().ai_summary.is_none(),
                        "prior mode should not show Generating before the gate clears"
                    );
                }
                other => panic!(
                    "expected the stashed mode to be PrReview, got {:?}",
                    std::mem::discriminant(other)
                ),
            }
        }
        other => panic!(
            "expected PromptPrecall, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn pr_review_memory_ai_summary_cancel_precall_leaves_dialog_untouched_and_confirm_still_generates()
{
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|p| Ok(p.to_path_buf()));
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![AgentKind::Claude],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    app.pr_review_open_memory_ai_summary_pick();
    assert!(matches!(app.mode, AppMode::PromptPrecall(_)));

    app.precall_cancel();

    // Cancelling the notice must not leave the dialog stuck showing
    // "Generating" for a run that was never spawned.
    assert!(!app.pr_review_memory_ai_summary_generating());
    assert!(app.memory_ai_summary_bg.is_none());
    assert_eq!(app.pr_review_memory_add_view(), Some(false));
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.memory_add.as_ref().unwrap().ai_summary.is_none())
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    // Pressing "summarize with AI" again still gates and, on confirm,
    // actually starts generating this time.
    app.pr_review_open_memory_ai_summary_pick();
    assert!(matches!(app.mode, AppMode::PromptPrecall(_)));
    app.precall_confirm().unwrap();
    assert!(app.pr_review_memory_ai_summary_generating());
    assert!(app.memory_ai_summary_bg.is_some());
}

#[test]
fn pr_review_memory_ai_summary_pick_multiple_harnesses_move_cancel_and_confirm() {
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|p| Ok(p.to_path_buf()));
    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![AgentKind::Claude, AgentKind::Codex],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();

    app.pr_review_open_memory_ai_summary_pick();
    assert!(app.pr_review_memory_ai_summary_picking());

    // Cancelling the picker starts nothing and returns to the ordinary
    // confirm view.
    app.pr_review_memory_ai_summary_pick_move(1);
    app.pr_review_memory_ai_summary_pick_cancel();
    assert!(!app.pr_review_memory_ai_summary_picking());
    assert!(!app.pr_review_memory_ai_summary_generating());
    assert_eq!(app.pr_review_memory_add_view(), Some(false));

    // Re-open, move to the second harness, confirm: gates on that harness.
    app.pr_review_open_memory_ai_summary_pick();
    app.pr_review_memory_ai_summary_pick_move(1);
    app.pr_review_memory_ai_summary_pick_confirm();
    match &app.mode {
        AppMode::PromptPrecall(pending) => assert_eq!(pending.harness, AgentKind::Codex),
        other => panic!(
            "expected PromptPrecall, got {:?}",
            std::mem::discriminant(other)
        ),
    }
}

#[test]
fn pr_review_cancel_watching_memory_ai_summary_leaves_raw_text_and_clears_state() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    let raw = memory_add_editor_text(&app);

    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().ai_summary =
            Some(crate::app::MemoryAiSummaryState::Generating {
                harness: AgentKind::Claude,
            });
    }
    assert!(app.pr_review_memory_ai_summary_generating());

    app.pr_review_cancel_watching_memory_ai_summary();

    assert!(!app.pr_review_memory_ai_summary_generating());
    assert_eq!(
        memory_add_editor_text(&app),
        raw,
        "cancelling never touches the raw/edited text"
    );
    assert_eq!(
        app.pr_review_memory_add_view(),
        Some(false),
        "the dialog stays open, nothing was persisted"
    );
}

#[test]
fn poll_memory_ai_summary_bg_success_overwrites_editor_and_lands_on_review() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().ai_summary =
            Some(crate::app::MemoryAiSummaryState::Generating {
                harness: AgentKind::Claude,
            });
    }

    let (tx, rx) = std::sync::mpsc::channel();
    app.memory_ai_summary_bg = Some(rx);
    tx.send(crate::app::pr_review::MemoryAiSummaryDone {
        comment_id: 1,
        result: Ok("Guard shared state before mutation (src/file1.rs:1).".to_string()),
    })
    .unwrap();

    assert!(app.poll_memory_ai_summary_bg());
    assert!(app.memory_ai_summary_bg.is_none());
    assert_eq!(
        memory_add_editor_text(&app),
        "Guard shared state before mutation (src/file1.rs:1)."
    );
    assert_eq!(
        app.pr_review_memory_add_view(),
        Some(false),
        "success lands back on the ordinary review/edit confirm view"
    );
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.memory_add.as_ref().unwrap().ai_summary.is_none())
        }
        _ => panic!("expected PrReview"),
    }
}

#[test]
fn poll_memory_ai_summary_bg_failure_shows_inline_error_and_keeps_raw_text() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    let raw = memory_add_editor_text(&app);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().ai_summary =
            Some(crate::app::MemoryAiSummaryState::Generating {
                harness: AgentKind::Claude,
            });
    }

    let (tx, rx) = std::sync::mpsc::channel();
    app.memory_ai_summary_bg = Some(rx);
    tx.send(crate::app::pr_review::MemoryAiSummaryDone {
        comment_id: 1,
        result: Err("harness not installed".to_string()),
    })
    .unwrap();

    assert!(app.poll_memory_ai_summary_bg());
    assert_eq!(
        memory_add_editor_text(&app),
        raw,
        "a failed run never touches the raw/edited text"
    );
    match &app.mode {
        AppMode::PrReview(state) => assert!(matches!(
            &state.memory_add.as_ref().unwrap().ai_summary,
            Some(crate::app::MemoryAiSummaryState::Failed(m)) if m == "harness not installed"
        )),
        _ => panic!("expected PrReview"),
    }
}

#[test]
fn pr_review_dismiss_memory_ai_summary_error_returns_to_idle_with_raw_text() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    let raw = memory_add_editor_text(&app);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().ai_summary =
            Some(crate::app::MemoryAiSummaryState::Failed("boom".to_string()));
    }
    assert_eq!(
        app.pr_review_memory_ai_summary_error(),
        Some("boom".to_string())
    );

    app.pr_review_dismiss_memory_ai_summary_error();

    assert_eq!(app.pr_review_memory_ai_summary_error(), None);
    assert_eq!(memory_add_editor_text(&app), raw);
    assert_eq!(app.pr_review_memory_add_view(), Some(false));
}

#[test]
fn poll_memory_ai_summary_bg_drops_a_stale_result_when_no_longer_watching() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    let raw = memory_add_editor_text(&app);
    // The dialog never entered `Generating` (or already stopped watching) —
    // there is nowhere live to land this result.
    let (tx, rx) = std::sync::mpsc::channel();
    app.memory_ai_summary_bg = Some(rx);
    tx.send(crate::app::pr_review::MemoryAiSummaryDone {
        comment_id: 1,
        result: Ok("Some AI text".to_string()),
    })
    .unwrap();

    assert!(app.poll_memory_ai_summary_bg());
    assert_eq!(
        memory_add_editor_text(&app),
        raw,
        "a result with nowhere live to land it must not be applied"
    );
}

#[test]
fn pr_review_append_memory_after_ai_summary_persists_the_generated_text_not_the_raw_seed() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    enter_pr_review(&mut app, 1);
    app.pr_review_open_memory_add();
    let raw = memory_add_editor_text(&app);

    if let AppMode::PrReview(state) = &mut app.mode {
        state.memory_add.as_mut().unwrap().ai_summary =
            Some(crate::app::MemoryAiSummaryState::Generating {
                harness: AgentKind::Claude,
            });
    }
    let (tx, rx) = std::sync::mpsc::channel();
    app.memory_ai_summary_bg = Some(rx);
    tx.send(crate::app::pr_review::MemoryAiSummaryDone {
        comment_id: 1,
        result: Ok("Never mutate shared state without the lock (src/file1.rs:1).".to_string()),
    })
    .unwrap();
    assert!(app.poll_memory_ai_summary_bg());

    app.pr_review_append_memory().unwrap();

    let contents = std::fs::read_to_string(repo.join(".amf").join("review-memory.md")).unwrap();
    assert!(contents.contains("- Never mutate shared state without the lock (src/file1.rs:1)."));
    assert!(
        !contents.contains(&raw),
        "the raw seed must not survive once the AI summary was confirmed"
    );
}

#[test]
fn pr_review_append_memory_global_scope_writes_the_cross_project_doc() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    let global_doc = tmp.path().join("global").join("review-memory.md");

    let mut worktree = MockWorktreeOps::new();
    let repo_clone = repo.clone();
    worktree
        .expect_repo_root()
        .times(1)
        .returning(move |_| Ok(repo_clone.clone()));

    let mut app = App::new_for_test(
        ProjectStore {
            version: 5,
            projects: vec![],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        },
        Box::new(MockTmuxOps::new()),
        Box::new(worktree),
    );
    // An absolute override keeps the test off the developer's real
    // `~/.config/amf/review-memory.md`.
    app.config.global_review_memory_path = Some(global_doc.display().to_string());
    enter_pr_review(&mut app, 1);

    app.pr_review_open_memory_add();
    app.pr_review_toggle_memory_scope();
    app.pr_review_append_memory().unwrap();

    assert_eq!(app.pr_review_memory_add_view(), None, "dialog closes");
    let contents = std::fs::read_to_string(&global_doc).unwrap();
    assert!(
        contents.starts_with("# Review memory (cross-project)"),
        "a freshly created global doc gets the cross-project header, got: {contents}"
    );
    assert!(contents.contains("- comment 1 (src/file1.rs:1)"));
    assert!(
        !repo.join(".amf").join("review-memory.md").exists(),
        "the project doc should be untouched when the global scope is picked"
    );
}

#[test]
fn review_memory_paths_resolves_project_override_and_global_together() {
    let tmp = TempDir::new().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir_all(repo.join(".amf")).unwrap();
    std::fs::write(
        repo.join(".amf").join("config.json"),
        r#"{"review_memory_path": ".amf/team-review-memory.md"}"#,
    )
    .unwrap();
    let global_doc = tmp.path().join("global-lessons.md");

    let mut app = App::new_for_test(
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
    app.config.global_review_memory_path = Some(global_doc.display().to_string());

    let paths = app.review_memory_paths(&repo);
    assert_eq!(
        paths.project,
        repo.join(".amf").join("team-review-memory.md")
    );
    assert_eq!(paths.global, global_doc);
}

#[test]
fn pr_review_open_fix_confirm_without_comments_shows_message() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 0);

    app.pr_review_open_fix_confirm();
    assert_eq!(app.pr_review_fix_editing(), None);
    assert!(app.message.is_some());
}

#[test]
fn poll_ai_pr_review_bg_warns_when_reviewing_and_done_arrive_together() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    tx.send(crate::app::ai_review::AiReviewProgress::Reviewing {
        token_estimate: 50_000,
    })
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![],
            summary: Some("No actionable issues found.".to_string()),
            raw_output: String::new(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();
    assert!(app.poll_ai_pr_review_bg());
    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("Large diff") && t.message.contains("50000")),
        "toasts: {:?}",
        app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
    assert!(
        !app.toasts
            .iter()
            .any(|toast| toast.message.contains("check the debug log")),
        "a valid summary-only response is a clean zero-finding review"
    );
}

#[test]
fn completed_ai_review_updates_stashed_triage_pending_count_and_summary() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    app.open_ai_review_from_triage();
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![
                sample_ai_review_finding("first"),
                sample_ai_review_finding("second"),
            ],
            summary: Some("Two correctness risks need attention.".to_string()),
            raw_output: "review output".to_string(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(
                state.summary.as_deref(),
                Some("Two correctness risks need attention.")
            );
            assert_eq!(state.findings.len(), 2);
        }
        _ => panic!("expected AI Review pane"),
    }
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_eq!(state.pending_ai_review_findings, 2);
            assert!(matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Findings(2))
            ));
        }
        _ => panic!("expected stashed PR Triage pane"),
    }

    app.ai_review_toggle_skip();
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => assert_eq!(state.pending_ai_review_findings, 1),
        _ => panic!("expected stashed PR Triage pane"),
    }
}

#[test]
fn completed_ai_review_carries_run_attribution_into_the_pane_and_cache() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    app.open_ai_review_from_triage();
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let pr = origin.pr.clone();

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });
    let attribution = crate::app::ai_review::AiReviewAttribution {
        harness: Some("claude".to_string()),
        model: Some("sonnet".to_string()),
        input_tokens: Some(9_000),
        output_tokens: Some(1_200),
        estimated_cost: Some("$0.05".to_string()),
        ..Default::default()
    };
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![sample_ai_review_finding("first")],
            summary: Some("One risk.".to_string()),
            raw_output: "review output".to_string(),
            attribution: attribution.clone(),
        },
    )))
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.attribution.as_ref(), Some(&attribution));
        }
        _ => panic!("expected AI Review pane"),
    }
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_ai_review_cache(pr.number, &pr.head_sha)
        .unwrap()
        .unwrap();
    assert_eq!(cached.attribution.as_ref(), Some(&attribution));
}

#[test]
fn completed_zero_ai_review_updates_visible_triage_immediately() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let origin = match &app.mode {
        AppMode::PrReview(state) => {
            sample_ai_review_state(state.workdir.clone(), state.review.pr.clone())
        }
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin));
    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![],
            summary: Some("No actionable issues found.".to_string()),
            raw_output: "## Summary\nNo actionable issues found.".to_string(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(state.pending_ai_review_findings, 0);
            assert!(matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Findings(0))
            ));
        }
        _ => panic!("expected visible PR Triage pane"),
    }
}

#[test]
fn ai_review_errors_update_visible_triage_for_agent_and_diff_failures() {
    for detail in ["agent exited with status 1", "failed to fetch PR diff"] {
        let store = store_with_feature(ProjectStatus::Active);
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        enter_pr_review_for_feature(&mut app, 1);
        let origin = match &app.mode {
            AppMode::PrReview(state) => {
                sample_ai_review_state(state.workdir.clone(), state.review.pr.clone())
            }
            _ => unreachable!(),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        app.ai_review_run.set_receiver_for_test(Some(rx));
        app.ai_review_run.set_origin_for_test(Some(origin));
        tx.send(crate::app::ai_review::AiReviewProgress::Done(Err(
            anyhow::anyhow!(detail),
        )))
        .unwrap();

        assert!(app.poll_ai_pr_review_bg());
        match &app.mode {
            AppMode::PrReview(state) => assert!(matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Error(message))
                    if message == detail
            )),
            _ => panic!("expected visible PR Triage pane"),
        }
    }
}

#[test]
fn disconnected_ai_review_worker_persists_error_and_updates_triage() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    let (workdir, pr) = match &app.mode {
        AppMode::PrReview(state) => (state.workdir.clone(), state.review.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run
        .set_origin_for_test(Some(sample_ai_review_state(workdir, pr.clone())));
    drop(tx);

    assert!(app.poll_ai_pr_review_bg());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Error(message))
                    if message.contains("disconnected")
            )
    ));
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_ai_review_cache(pr.number, &pr.head_sha)
        .unwrap()
        .unwrap();
    assert!(matches!(
        cached.last_run.unwrap().outcome,
        crate::app::ai_review::AiReviewRunOutcome::Error(message)
            if message.contains("disconnected")
    ));
}

#[test]
fn escape_keeps_ai_review_running_through_triage_return_and_completion() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    app.open_ai_review_from_triage();
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    app.cancel_ai_pr_review();
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    assert!(app.ai_review_run.is_pending());
    app.close_ai_review();
    match &app.mode {
        AppMode::PrReview(state) => assert!(matches!(
            app.ai_review_triage_status(state),
            crate::app::ai_review::AiReviewTriageStatus::Running
        )),
        _ => panic!("expected restored PR Triage pane"),
    }

    tx.send(crate::app::ai_review::AiReviewProgress::Done(Ok(
        crate::app::ai_review::AiReviewOutcome {
            findings: vec![],
            summary: Some("No actionable issues found.".to_string()),
            raw_output: "## Summary\nNo actionable issues found.".to_string(),
            attribution: crate::app::ai_review::AiReviewAttribution::default(),
        },
    )))
    .unwrap();
    assert!(app.poll_ai_pr_review_bg());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if matches!(
                state.ai_review_last_run.as_ref().map(|run| &run.outcome),
                Some(crate::app::ai_review::AiReviewRunOutcome::Findings(0))
            )
    ));
}

#[test]
fn poll_ai_pr_review_bg_surfaces_streamed_activity_and_usage() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.mode = AppMode::AiReviewRunning(crate::app::AiReviewRunState {
        origin,
        progress: crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::Reviewing {
                token_estimate: 42_000,
            },
            started_at: std::time::Instant::now(),
            activity: None,
            usage: None,
        },
    });

    tx.send(crate::app::ai_review::AiReviewProgress::Activity(
        "Inspecting the repository".to_string(),
    ))
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Usage {
        input_tokens: 41_000,
        output_tokens: 900,
    })
    .unwrap();

    assert!(app.poll_ai_pr_review_bg());
    match &app.mode {
        AppMode::AiReviewRunning(state) => {
            assert_eq!(
                state.progress.activity.as_deref(),
                Some("Inspecting the repository")
            );
            assert_eq!(state.progress.usage, Some((41_000, 900)));
        }
        _ => panic!("expected running AI review"),
    }
}

#[test]
fn running_ai_review_can_reopen_preserved_progress_after_escape() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    let origin = match &app.mode {
        AppMode::AiReview(state) => state.clone(),
        _ => unreachable!(),
    };
    let started_at = std::time::Instant::now() - std::time::Duration::from_secs(75);
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_run.set_receiver_for_test(Some(rx));
    app.ai_review_run.set_origin_for_test(Some(origin.clone()));
    app.ai_review_run
        .set_progress_for_test(Some(crate::app::AiReviewRunProgress {
            stage: crate::app::ai_review::AiReviewStage::PreparingDiff,
            started_at,
            activity: None,
            usage: None,
        }));

    // Progress that lands after leaving the full-screen view is retained in
    // the app-level run state rather than discarded with the old mode.
    tx.send(crate::app::ai_review::AiReviewProgress::Reviewing {
        token_estimate: 95_000,
    })
    .unwrap();
    tx.send(crate::app::ai_review::AiReviewProgress::Activity(
        "Inspecting the repository".to_string(),
    ))
    .unwrap();
    assert!(app.poll_ai_pr_review_bg());

    app.start_ai_pr_review();
    match &app.mode {
        AppMode::AiReviewRunning(state) => {
            assert_eq!(state.progress.started_at, started_at);
            assert_eq!(
                state.progress.stage,
                crate::app::ai_review::AiReviewStage::Reviewing {
                    token_estimate: 95_000
                }
            );
            assert_eq!(
                state.progress.activity.as_deref(),
                Some("Inspecting the repository")
            );
        }
        _ => panic!("expected preserved AI review progress"),
    }
}

#[test]
fn open_ai_review_for_pr_starts_empty_with_no_stashed_return() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    // A stash left over from a previous PR Triage-originated visit must not
    // leak into a fresh dashboard/session/picker entry.
    let pr = crate::github::PrRef {
        number: 1,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/1".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    app.ai_review_return_to = Some(Box::new(AppMode::Normal));

    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), pr.clone());

    assert!(app.ai_review_return_to.is_none());
    match &app.mode {
        AppMode::AiReview(state) => {
            assert!(state.findings.is_empty());
            assert_eq!(state.pr.number, 1);
            assert_eq!(state.workdir, PathBuf::from("/tmp/test-workdir"));
        }
        other => panic!("expected AiReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn open_ai_review_for_pr_reopens_cached_findings_and_summary() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    let pr = pr_review_with_comments(1).pr;
    app.db
        .as_ref()
        .unwrap()
        .save_ai_review_cache(
            pr.number,
            &pr.head_sha,
            &crate::app::ai_review::AiReviewCacheEntry {
                findings: vec![sample_ai_review_finding("cached finding")],
                last_run: Some(crate::app::ai_review::AiReviewRun {
                    ran_at: chrono::Local::now(),
                    outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(1),
                }),
                summary: Some("Cached review summary.".to_string()),
                attribution: None,
            },
        )
        .unwrap();
    assert_eq!(app.ai_review_triage_snapshot(&pr).pending_findings, 1);

    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), pr);

    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.findings[0].body, "cached finding");
            assert_eq!(state.summary.as_deref(), Some("Cached review summary."));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn seed_ai_review_fixture_persists_and_opens_without_running_an_agent() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    let workdir = tempfile::tempdir().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());

    let response = app
        .seed_ai_review_from_request(&SeedAiReviewRequest {
            pr_number: 55,
            head_sha: "fixture-sha".to_string(),
            summary: "Deterministic completed review.".to_string(),
            findings: vec![SeedAiReviewFinding {
                body: "A deterministic finding.".to_string(),
                ..Default::default()
            }],
            open: true,
            workdir: Some(workdir.path().to_path_buf()),
            repository: Some("owner/repository".to_string()),
            head_ref: Some("fixture-branch".to_string()),
        })
        .unwrap();

    assert!(response.ok);
    assert!(response.opened);
    assert_eq!(response.finding_count, 1);
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.pr.number, 55);
            assert_eq!(state.pr.head_sha, "fixture-sha");
            assert_eq!(state.findings[0].body, "A deterministic finding.");
            assert_eq!(
                state.summary.as_deref(),
                Some("Deterministic completed review.")
            );
        }
        _ => panic!("expected seeded AI Review pane"),
    }
}

#[test]
fn pr_triage_zero_result_survives_reopen_and_restart_but_not_new_head() {
    let db_dir = TempDir::new().unwrap();
    let db_path = db_dir.path().join("amf.db");
    let review = pr_review_with_comments(1);
    let zero_run = crate::app::ai_review::AiReviewRun {
        ran_at: chrono::Local::now(),
        outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(0),
    };

    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(&db_path).unwrap());
    app.db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&review)
        .unwrap();
    app.db
        .as_ref()
        .unwrap()
        .save_ai_review_cache(
            review.pr.number,
            &review.pr.head_sha,
            &crate::app::ai_review::AiReviewCacheEntry {
                findings: vec![],
                last_run: Some(zero_run.clone()),
                summary: None,
                attribution: None,
            },
        )
        .unwrap();

    app.enter_pr_review(PathBuf::from("/tmp/test-workdir"), review.pr.clone());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if state.ai_review_last_run.as_ref() == Some(&zero_run)
    ));
    app.mode = AppMode::Normal;
    app.enter_pr_review(PathBuf::from("/tmp/test-workdir"), review.pr.clone());
    assert!(matches!(
        &app.mode,
        AppMode::PrReview(state)
            if state.ai_review_last_run.as_ref() == Some(&zero_run)
    ));
    drop(app);

    let store = store_with_feature(ProjectStatus::Idle);
    let mut restarted = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    restarted.db = Some(crate::db::AmfDb::open(&db_path).unwrap());
    restarted.enter_pr_review(PathBuf::from("/tmp/test-workdir"), review.pr.clone());
    assert!(matches!(
        &restarted.mode,
        AppMode::PrReview(state)
            if state.ai_review_last_run.as_ref() == Some(&zero_run)
    ));

    let mut moved = review;
    moved.pr.head_sha = "new-head".to_string();
    restarted
        .db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&moved)
        .unwrap();
    restarted.enter_pr_review(PathBuf::from("/tmp/test-workdir"), moved.pr.clone());
    assert!(matches!(
        &restarted.mode,
        AppMode::PrReview(state)
            if state.review.pr.head_sha == "new-head"
                && state.ai_review_last_run.is_none()
                && state.pending_ai_review_findings == 0
    ));
}

#[test]
fn refreshed_pr_head_clears_stale_ai_review_completion() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.ai_review_last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(0),
        });
    }
    app.open_ai_review_from_triage();
    let (workdir, old_pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let mut moved = pr_review_with_comments(2);
    moved.pr.head_sha = "new-head".to_string();
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh {
        workdir,
        pr: old_pr,
    });
    tx.send(Ok(moved)).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_eq!(state.review.pr.head_sha, "new-head");
            assert!(state.ai_review_last_run.is_none());
            assert_eq!(state.pending_ai_review_findings, 0);
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn ai_review_post_dialog_is_seeded_with_generated_summary_and_attribution() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.findings = vec![sample_ai_review_finding("A general finding")];
        state.summary = Some("The patch has one correctness risk.".to_string());
        state.last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(1),
        });
    }

    app.ai_review_open_post_confirm();

    match &app.mode {
        AppMode::AiReview(state) => {
            let body = state.post_confirm.as_ref().unwrap().editor.text();
            assert!(body.starts_with("The patch has one correctness risk."));
            assert!(body.contains("A general finding"));
            assert!(body.ends_with("— AI review via AMF"));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn ai_review_post_dialog_seeds_usage_summary_only_on_the_overall_review() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        let mut anchored = sample_ai_review_finding("An anchored finding");
        anchored.path = Some("src/lib.rs".to_string());
        anchored.line = Some(10);
        anchored.side = Some(crate::diff::DiffSide::New);
        anchored.diff_hunk = Some("@@ -1 +1 @@".to_string());
        state.findings = vec![anchored];
        state.summary = Some("One risk.".to_string());
        state.last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(1),
        });
        state.attribution = Some(crate::app::ai_review::AiReviewAttribution {
            harness: Some("claude".to_string()),
            model: Some("sonnet".to_string()),
            input_tokens: Some(12_300),
            output_tokens: Some(4_500),
            estimated_cost: Some("$0.10".to_string()),
            ..Default::default()
        });
    }

    app.ai_review_open_post_confirm();

    match &app.mode {
        AppMode::AiReview(state) => {
            let post = state.post_confirm.as_ref().unwrap();
            let body = post.editor.text();
            assert!(
                body.contains("### AI review usage\n- Harness: claude\n- Model: sonnet"),
                "summary should carry the usage section: {body}"
            );
            assert!(body.contains("Input tokens: 12.3k"));
            assert!(body.contains("Estimated cost: $0.10"));
            assert!(body.ends_with("— AI review via AMF"));
            assert!(
                !post.inline[0].body.contains("AI review usage"),
                "inline comment must not carry the usage section: {}",
                post.inline[0].body
            );
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn ai_review_post_dialog_drops_generated_summary_when_a_finding_is_skipped() {
    // Regression: `state.summary` is model prose written over the *complete*
    // finding set, so it can still describe a finding the user has since
    // skipped (a false positive, or one too sensitive to post) even though
    // that finding is excluded from the posted findings below it. Once
    // anything's skipped, the dialog must fall back to the generic
    // placeholder rather than risk republishing what `skipped` was meant to
    // suppress.
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        let mut skipped = sample_ai_review_finding("A sensitive finding");
        skipped.skipped = true;
        state.findings = vec![skipped, sample_ai_review_finding("A kept finding")];
        state.summary = Some("The patch has a sensitive issue plus a kept finding.".to_string());
        state.last_run = Some(crate::app::ai_review::AiReviewRun {
            ran_at: chrono::Local::now(),
            outcome: crate::app::ai_review::AiReviewRunOutcome::Findings(2),
        });
    }

    app.ai_review_open_post_confirm();

    match &app.mode {
        AppMode::AiReview(state) => {
            let body = state.post_confirm.as_ref().unwrap().editor.text();
            assert!(
                !body.contains("sensitive issue"),
                "the model summary describing the skipped finding must not be posted: {body}"
            );
            assert!(body.starts_with("AI review, via AMF."));
            assert!(body.contains("A kept finding"));
        }
        _ => panic!("expected AI Review pane"),
    }
}

#[test]
fn open_ai_review_from_triage_stashes_the_pane_and_close_restores_it() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    let triage_workdir = match &app.mode {
        AppMode::PrReview(state) => state.workdir.clone(),
        _ => unreachable!(),
    };

    app.open_ai_review_from_triage();

    match &app.mode {
        AppMode::AiReview(state) => assert_eq!(state.workdir, triage_workdir),
        other => panic!("expected AiReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(app.ai_review_return_to.is_some());

    app.close_ai_review();
    assert!(matches!(&app.mode, AppMode::PrReview(_)));
    assert!(app.ai_review_return_to.is_none());
}

#[test]
fn post_success_refresh_updates_stashed_triage_without_leaving_ai_review() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1;
        state.hide_resolved = true;
        state.marked.insert(2);
    }
    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh { workdir, pr });
    tx.send(Ok(pr_review_with_comments(3))).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_eq!(state.review.comments.len(), 3);
            assert_eq!(state.selected_comment().map(|comment| comment.id), Some(2));
            assert!(state.hide_resolved);
            assert!(state.marked.contains(&2));
            assert_eq!(state.pending_ai_review_findings, 0);
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_snaps_selection_off_a_newly_resolved_comment() {
    // Regression: restoring the selection by id after a refresh can land it
    // on a comment that `hide_resolved` now excludes (its thread resolved
    // upstream since the last fetch), leaving `selected` pointing at a row
    // `visible_indices()` doesn't include.
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 3);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 1; // comment id 2, not yet resolved
        state.hide_resolved = true;
    }
    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh { workdir, pr });

    // The refresh comes back with comment 2's thread now resolved.
    let comments: Vec<crate::github::ReviewComment> = (1..=3u64)
        .map(|id| crate::github::ReviewComment {
            id,
            path: Some(format!("src/file{id}.rs")),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("comment {id}"),
            user: crate::github::GhUser {
                login: "alice".to_string(),
                kind: "User".to_string(),
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let threads = vec![crate::github::ReviewThread {
        id: "T2".to_string(),
        is_resolved: true,
        comment_ids: vec![2],
    }];
    let refreshed_pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let refreshed =
        crate::app::pr_review::normalize(refreshed_pr, comments, vec![], vec![], threads);
    tx.send(Ok(refreshed)).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_ne!(
                state.selected_comment().map(|comment| comment.id),
                Some(2),
                "selection must not stay on a comment hide_resolved now excludes"
            );
            assert!(
                state.visible_indices().contains(&state.selected),
                "selection must land on a row the current filter shows"
            );
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_snaps_selection_off_a_newly_collated_amf_reply() {
    use crate::app::pr_review::AMF_ATTRIBUTION_FOOTER;

    // Regression: `selected` can itself be an orphaned AMF follow-up reply
    // (its root wasn't fetched, so it stood as its own row). If a refresh
    // fetches the root, the reply becomes collated under it and vanishes
    // from `visible_indices()`. `all_sorted_indices()` used to also drop
    // collated replies, so `position()` couldn't find `selected` there
    // either and both neighbor searches short-circuited to `visible[0]`
    // instead of the nearest neighbor (the reply's own root).
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);

    let pr = crate::github::PrRef {
        number: 7,
        head_sha: "sha".to_string(),
        url: "https://github.com/o/r/pull/7".to_string(),
        owner: "o".to_string(),
        repo: "r".to_string(),
        head_ref: "main".to_string(),
    };
    let user = |login: &str| crate::github::GhUser {
        login: login.to_string(),
        kind: "User".to_string(),
    };

    // An unrelated comment that sorts before the reply's root in fetch order.
    // Its presence is what distinguishes "snap to the nearest neighbor" from
    // "snap to the first visible row" — with only the reply and its root,
    // the root would also happen to be `visible[0]`, masking the bug.
    let unrelated = crate::github::ReviewComment {
        id: 3,
        path: Some("src/other.rs".into()),
        line: Some(4),
        original_line: Some(4),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: "Unrelated finding.".into(),
        user: user("reviewer"),
        in_reply_to_id: None,
        pull_request_review_id: None,
    };

    // Initial fetch: the AMF reply came back but its root did not, so it's
    // orphaned and shown as its own row. It's selected.
    let orphan_reply = crate::github::ReviewComment {
        id: 2,
        path: Some("src/lib.rs".into()),
        line: Some(10),
        original_line: Some(10),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: format!("Done in `abc123`.\n\n{AMF_ATTRIBUTION_FOOTER}"),
        user: user("author"),
        in_reply_to_id: Some(1),
        pull_request_review_id: Some(91),
    };
    app.mode = AppMode::PrReview(PrReviewState {
        selected: 1,
        review: crate::app::pr_review::normalize(
            pr.clone(),
            vec![unrelated.clone(), orphan_reply.clone()],
            vec![],
            vec![],
            vec![],
        ),
        ..match app.mode {
            AppMode::PrReview(state) => state,
            _ => unreachable!(),
        }
    });

    app.open_ai_review_from_triage();
    let (workdir, pr) = match &app.mode {
        AppMode::AiReview(state) => (state.workdir.clone(), state.pr.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh {
        workdir,
        pr: pr.clone(),
    });

    // The refresh comes back with the reply's root now present, so the
    // reply collates under it and drops out of `visible_indices()`.
    let root = crate::github::ReviewComment {
        id: 1,
        path: Some("src/lib.rs".into()),
        line: Some(10),
        original_line: Some(10),
        side: Some("RIGHT".into()),
        diff_hunk: Some("@@".into()),
        subject_type: None,
        body: "Please add a test here.".into(),
        user: user("reviewer"),
        in_reply_to_id: None,
        pull_request_review_id: Some(90),
    };
    let refreshed = crate::app::pr_review::normalize(
        pr,
        vec![unrelated, root, orphan_reply],
        vec![],
        vec![],
        vec![],
    );
    tx.send(Ok(refreshed)).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    match app.ai_review_return_to.as_deref() {
        Some(AppMode::PrReview(state)) => {
            assert_ne!(
                state.selected_comment().map(|comment| comment.id),
                Some(2),
                "selection must not stay on the now-collated AMF reply"
            );
            assert_eq!(
                state.selected_comment().map(|comment| comment.id),
                Some(1),
                "selection should snap to the reply's own root, its nearest visible neighbor"
            );
            assert!(
                state.visible_indices().contains(&state.selected),
                "selection must land on a row the current filter shows"
            );
        }
        _ => panic!("expected refreshed stashed PR Triage pane"),
    }
}

#[test]
fn post_success_refresh_caches_fresh_triage_when_ai_review_has_no_return_pane() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = tempfile::NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    let stale = pr_review_with_comments(1);
    app.db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&stale)
        .unwrap();
    app.open_ai_review_for_pr(PathBuf::from("/tmp/test-workdir"), stale.pr.clone());

    let (tx, rx) = std::sync::mpsc::channel();
    app.ai_review_triage_refresh_bg = Some(rx);
    app.ai_review_triage_refresh_pending = Some(crate::app::AiReviewTriageRefresh {
        workdir: PathBuf::from("/tmp/test-workdir"),
        pr: stale.pr.clone(),
    });
    tx.send(Ok(pr_review_with_comments(3))).unwrap();

    assert!(app.poll_ai_review_triage_refresh_bg());
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    let cached = app
        .db
        .as_ref()
        .unwrap()
        .load_pr_review_cache(stale.pr.number, &stale.pr.head_sha)
        .unwrap()
        .unwrap();
    assert_eq!(cached.comments.len(), 3);
}

#[test]
fn close_ai_review_with_no_stash_returns_to_the_dashboard() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_ai_review_for_feature(&mut app);
    assert!(app.ai_review_return_to.is_none());

    app.close_ai_review();

    assert!(matches!(&app.mode, AppMode::Normal));
}

#[test]
fn pr_picker_choose_ai_review_is_noop_outside_picker_mode() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.pr_picker_choose_ai_review();
    assert!(matches!(&app.mode, AppMode::Normal));
}

// ---------------------------------------------------------------------------
// PR Triage: the `New feature…` fix target (companion triage feature)
// ---------------------------------------------------------------------------

/// An app whose single feature lives at `/tmp/test-workdir`, with a worktree
/// mock that resolves the repo root — enough for the fix-target picker and the
/// triage-feature setup overlay, neither of which touches git.
fn triage_target_app(store: ProjectStore) -> App {
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree))
}

fn triage_setup(app: &App) -> &crate::app::TriageFeatureSetupState {
    match &app.mode {
        AppMode::PrReview(state) => state
            .new_feature_setup
            .as_ref()
            .expect("triage-feature setup overlay should be open"),
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

/// Move the fix-target picker's highlight onto the `New feature…` row.
fn select_new_feature_row(app: &mut App) {
    if let AppMode::PrReview(state) = &mut app.mode {
        let pick = state
            .harness_pick
            .as_mut()
            .expect("fix-target picker should be open");
        pick.selected = pick
            .rows
            .iter()
            .position(|r| matches!(r, crate::app::pr_review::FixTargetPickRow::NewFeature))
            .expect("New feature… row should be present");
    }
}

#[test]
fn pr_review_fix_target_picker_offers_new_feature_last() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 2);

    app.pr_review_open_fix_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            let pick = state.harness_pick.as_ref().expect("picker should be open");
            assert_eq!(
                pick.rows.last(),
                Some(&crate::app::pr_review::FixTargetPickRow::NewFeature),
                "the isolated option is last so it reads as the deliberate choice"
            );
            // …and it is not what the cursor lands on by default.
            assert_ne!(pick.selected, pick.rows.len() - 1);
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
}

#[test]
fn choosing_new_feature_opens_setup_instead_of_the_fix_confirm() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 2);

    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.harness_pick.is_none(), "picker closes");
            assert!(
                state.fix_confirm.is_none(),
                "nothing is injected until the feature exists"
            );
            assert!(state.new_feature_setup.is_some(), "setup overlay opens");
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    // The branch is pre-filled from the PR's head branch, and is deliberately
    // NOT that branch — git can't check one branch out in two worktrees.
    let setup = triage_setup(&app);
    assert_eq!(setup.branch, "main-triage");
    assert_ne!(setup.branch, "main");
}

#[test]
fn triage_feature_records_the_prs_own_branch_not_the_checked_out_one() {
    // The documented "other PR" flow triages a PR whose head branch is not what
    // the feature has checked out. `TriageSource::pr_branch` is used verbatim as
    // the `git push origin <triage>:<dest>` destination, so reading it off the
    // source feature would push review fixes onto an unrelated remote branch.
    let repo_dir = TempDir::new().unwrap();
    let repo = repo_dir.path().to_path_buf();
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {:?}", out.stderr);
    };
    git(&repo, &["init", "-q", "-b", "my-feat"]);
    git(&repo, &["config", "user.email", "t@example.com"]);
    git(&repo, &["config", "user.name", "T"]);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "base"]);
    // The PR lives on its own branch, one commit ahead of what's checked out —
    // so `my-feat` does not even contain the PR head.
    git(&repo, &["checkout", "-q", "-b", "pr-head"]);
    std::fs::write(repo.join("a.txt"), "one\ntwo\n").unwrap();
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-qm", "pr work"]);
    let head_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&repo)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    git(&repo, &["checkout", "-q", "my-feat"]);

    let triage_workdir = repo.join(".worktrees").join("pr-head-triage");
    std::fs::create_dir_all(&triage_workdir).unwrap();

    let mut worktree = MockWorktreeOps::new();
    let repo_for_root = repo.clone();
    worktree
        .expect_repo_root()
        .returning(move |_| Ok(repo_for_root.clone()));
    let triage_workdir_clone = triage_workdir.clone();
    worktree
        .expect_create_from()
        .times(1)
        // The worktree is branched off the PR's branch, not the checkout's.
        .withf(|_, _, branch, base| branch == "pr-head-triage" && base == "pr-head")
        .returning(move |_, _, _, _| Ok(triage_workdir_clone.clone()));

    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().returning(|_| false);
    tmux.expect_create_session_with_window()
        .returning(|_, _, _| Ok(()));
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].repo = repo.clone();
    store.projects[0].features[0].workdir = repo.clone();
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(worktree));
    let store_file = NamedTempFile::new().unwrap();
    app.store_path = store_file.path().to_path_buf();

    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = repo.clone();
        state.review.pr.head_ref = "pr-head".to_string();
        state.review.pr.head_sha = head_sha.clone();
        state.checked_out_branch = Some("my-feat".to_string());
    }

    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();
    // The overlay pre-fills from the PR's branch, and the created feature has
    // to agree with it.
    assert_eq!(triage_setup(&app).branch, "pr-head-triage");
    app.pr_review_triage_setup_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => assert!(
            state.new_feature_setup.is_none(),
            "creation failed: {:?}",
            state
                .new_feature_setup
                .as_ref()
                .and_then(|s| s.error.clone())
        ),
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    let companion = app.store.projects[0]
        .features
        .iter()
        .find(|f| f.name == "pr-head-triage")
        .expect("the companion feature should have been created");
    let link = companion
        .triage_source
        .as_ref()
        .expect("the companion is linked back to the PR");
    assert_eq!(
        link.pr_branch, "pr-head",
        "fixes land on the PR's branch, not the source feature's `my-feat`"
    );
    assert_eq!(link.source_feature_id, "feat-1");
}

#[test]
fn triage_setup_branch_is_deduplicated_against_existing_features() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    // A previous triage feature for this branch already claimed the name.
    let mut earlier = store.projects[0].features[0].clone();
    earlier.id = "feat-2".to_string();
    earlier.name = "main-triage".to_string();
    earlier.workdir = PathBuf::from("/tmp/test-workdir-triage");
    store.projects[0].features.push(earlier);

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    assert_eq!(triage_setup(&app).branch, "main-triage-2");
}

#[test]
fn triage_setup_rows_cycle_their_values() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    // Row 0 is Preset; move to Mode and cycle it. This is the setting the
    // whole target exists for — triaging in a different mode than the PR was
    // built in.
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibeless);
    app.pr_review_triage_setup_move(2);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Mode
    );
    app.pr_review_triage_setup_adjust(1);
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibe);
    app.pr_review_triage_setup_adjust(-1);
    assert_eq!(triage_setup(&app).mode, VibeMode::Vibeless);

    // Booleans toggle regardless of direction.
    app.pr_review_triage_setup_move(1);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Review
    );
    assert!(!triage_setup(&app).review);
    app.pr_review_triage_setup_adjust(1);
    assert!(triage_setup(&app).review);

    // Row movement wraps: from Review (index 3), -4 lands on Branch (index 5).
    app.pr_review_triage_setup_move(-4);
    assert_eq!(
        triage_setup(&app).focused_row(),
        crate::app::TriageSetupRow::Branch
    );
}

#[test]
fn triage_setup_branch_row_takes_typed_input() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    // Not on the branch row: typing is not routed here at all.
    assert!(!app.pr_review_triage_setup_on_branch_row());

    app.pr_review_triage_setup_move(5);
    assert!(app.pr_review_triage_setup_on_branch_row());
    app.pr_review_triage_setup_branch_backspace();
    app.pr_review_triage_setup_branch_push('X');
    assert_eq!(triage_setup(&app).branch, "main-triagX");

    // Cycling is a no-op on the text row (nothing to cycle through).
    app.pr_review_triage_setup_adjust(1);
    assert_eq!(triage_setup(&app).branch, "main-triagX");
}

#[test]
fn triage_setup_preset_fills_in_the_rows_below_it() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];
    let mut app = triage_target_app(store);
    app.active_extension.feature_presets = vec![crate::extension::FeaturePreset {
        name: "Careful triage".to_string(),
        branch_prefix: Some("triage/".to_string()),
        mode: VibeMode::Vibeless,
        agent: AgentKind::Codex,
        review: true,
        plan_mode: true,
        quick_plan: false,
        enable_chrome: true,
        remote_control: false,
    }];
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    assert_eq!(triage_setup(&app).preset_label(), "Manual");
    // Row 0 is the preset row; cycling forward selects the only preset.
    app.pr_review_triage_setup_adjust(1);

    let setup = triage_setup(&app);
    assert_eq!(setup.preset_label(), "Careful triage");
    assert_eq!(setup.agent(), AgentKind::Codex);
    assert!(setup.review);
    assert!(setup.enable_chrome);
    assert_eq!(
        setup.branch, "triage/main-triage",
        "the preset's branch prefix applies to the companion branch too"
    );
}

#[test]
fn cancelling_triage_setup_leaves_the_fix_target_unresolved() {
    // Backing out of the setup means no target was chosen — the next `f` must
    // re-offer every option rather than silently using the default.
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.pr_review_open_fix_confirm();
    select_new_feature_row(&mut app);
    app.pr_review_harness_pick_confirm();

    app.pr_review_triage_setup_cancel();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(state.new_feature_setup.is_none());
            assert!(state.fix_confirm.is_none());
            assert!(!state.fix_target_picked);
            assert!(!state.pending_batch);
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_some()),
        "the picker is offered again"
    );
}

/// Attach a companion triage feature for PR #7 to the store, linked back to
/// the source feature the way `create_triage_feature` persists it.
fn push_companion_feature(store: &mut ProjectStore, agent: AgentKind, mode: VibeMode) {
    let source_id = store.projects[0].features[0].id.clone();
    let mut companion = store.projects[0].features[0].clone();
    companion.id = "feat-triage".to_string();
    companion.name = "main-triage".to_string();
    companion.branch = "main-triage".to_string();
    companion.workdir = PathBuf::from("/tmp/test-workdir/.worktrees/main-triage");
    companion.is_worktree = true;
    companion.agent = agent;
    companion.mode = mode;
    companion.sessions = vec![];
    companion.add_session_named(SessionKind::Claude, "PR Triage".to_string());
    companion.triage_source = Some(crate::project::TriageSource {
        pr_number: 7,
        source_feature_id: source_id,
        // The PR's head branch (`pr_review_with_comments` puts the PR on
        // `main`), not the source feature's own branch — that's what
        // `create_triage_feature` records, and it's the push destination.
        pr_branch: "main".to_string(),
        base_sha: "basesha".to_string(),
    });
    store.projects[0].features.push(companion);
}

#[test]
fn companion_target_resolves_sessions_in_the_triage_feature() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    // A same-named session in the *source* feature must not be mistaken for
    // the companion's — that would inject fixes into the wrong worktree.
    store.projects[0].features[0].add_session_named(SessionKind::Claude, "PR Triage".to_string());
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibeless);

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }

    assert_eq!(
        app.pr_review_target_feature(),
        Some((0, 1)),
        "the companion feature, not the source one"
    );
}

#[test]
fn opening_a_pr_with_an_existing_companion_adopts_it() {
    // The companion is reused for every fix in the PR, across pane re-opens
    // and restarts — so entering the pane must not re-ask for a target.
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);

    app.adopt_existing_triage_feature();

    match &app.mode {
        AppMode::PrReview(state) => {
            assert_eq!(
                state.fix_target,
                crate::app::pr_review::FixTarget::NewFeature
            );
            assert!(state.fix_target_picked, "no re-ask on the next fix");
            assert_eq!(state.review_harness, Some(AgentKind::Codex));
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_none()
            && state.new_feature_setup.is_none()
            && state.fix_confirm.is_some()),
        "the fix goes straight to the confirm dialog in the adopted feature"
    );
}

#[test]
fn adopt_is_a_no_op_without_a_companion_for_this_pr() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    // Same feature, different PR: the link is keyed on the PR number too.
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .pr_number = 99;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if !state.fix_target_picked),
        "an unrelated PR's triage feature is not adopted"
    );
}

#[test]
fn a_vanished_companion_really_does_reopen_the_picker() {
    // The error text promises "press f and pick a target again", so the pane
    // must actually un-resolve the target — both `fix_target_picked` and
    // `review_harness` short-circuit the picker.
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibe);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    // The companion is deleted mid-visit (from the dashboard, say).
    app.store.projects[0].features.remove(1);
    app.pr_review_inject_fix().unwrap();

    assert!(
        app.toasts
            .iter()
            .any(|t| t.message.contains("no longer exists")),
        "the pane reports it: {:?}",
        app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
    );
    match &app.mode {
        AppMode::PrReview(state) => {
            assert!(!state.fix_target_picked);
            assert!(
                state.review_harness.is_none(),
                "the dead harness is dropped"
            );
        }
        other => panic!(
            "the pane stays open so `f` is still reachable, got {:?}",
            std::mem::discriminant(other)
        ),
    }

    app.pr_review_open_fix_confirm();
    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.harness_pick.is_some()),
        "`f` re-offers every target instead of resolving against the dead one"
    );
}

#[test]
fn fix_confirm_names_the_companion_feature_and_its_mode() {
    let mut store = store_with_feature(ProjectStatus::Stopped);
    push_companion_feature(&mut store, AgentKind::Codex, VibeMode::Vibeless);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    app.adopt_existing_triage_feature();

    assert_eq!(
        app.pr_review_triage_feature_summary().as_deref(),
        Some("main-triage · Codex · Vibeless")
    );

    // The in-feature targets say nothing extra — they run where the user
    // already is.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_target = crate::app::pr_review::FixTarget::DedicatedReview;
    }
    assert!(app.pr_review_triage_feature_summary().is_none());
}

#[test]
fn integrate_is_rejected_for_the_in_feature_targets() {
    let store = store_with_feature(ProjectStatus::Stopped);
    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);

    app.pr_review_open_integrate();

    assert!(
        matches!(&app.mode, AppMode::PrReview(state) if state.integrate.is_none()),
        "the dedicated/live targets already commit on the PR branch"
    );
    assert!(!app.toasts.is_empty(), "the user is told why");
}

#[test]
fn integrate_reports_the_commits_and_blocks_cherry_pick_on_a_dirty_source() {
    use crate::app::TriageIntegration;

    // Real git: a source repo with a commit, and a companion worktree branched
    // from it carrying one triage commit.
    let repo = TempDir::new().unwrap();
    let source = repo.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
    };
    git(&source, &["init", "-q", "-b", "main"]);
    git(&source, &["config", "user.email", "t@example.com"]);
    git(&source, &["config", "user.name", "T"]);
    std::fs::write(source.join("a.txt"), "one\n").unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "base"]);
    let base_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&source)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let triage = repo.path().join("triage");
    git(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            "main-triage",
            triage.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(triage.join("a.txt"), "one\ntwo\n").unwrap();
    git(&triage, &["add", "-A"]);
    git(&triage, &["commit", "-qm", "apply review comment"]);

    // Dirty the source worktree — the cherry-pick must refuse rather than
    // clobber in-progress work.
    std::fs::write(source.join("b.txt"), "wip\n").unwrap();

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].workdir = source.clone();
    store.projects[0].features[0].branch = "main".to_string();
    push_companion_feature(&mut store, AgentKind::Claude, VibeMode::Vibeless);
    store.projects[0].features[1].workdir = triage.clone();
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .base_sha = base_sha;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = source.clone();
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }

    app.pr_review_open_integrate();

    let (commits, source_dirty) = match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().expect("overlay should be open");
            assert_eq!(integrate.triage_branch, "main-triage");
            assert_eq!(integrate.pr_branch, "main");
            (integrate.commits.clone(), integrate.source_dirty.clone())
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    };
    assert_eq!(commits.len(), 1, "one triage commit to land");
    assert!(commits[0].contains("apply review comment"));
    assert!(
        source_dirty.is_some(),
        "the dirty source worktree disables the cherry-pick"
    );

    // Confirming the cherry-pick refuses instead of running it, and leaves the
    // source worktree exactly as it was.
    if let AppMode::PrReview(state) = &mut app.mode {
        let integrate = state.integrate.as_mut().unwrap();
        integrate.selected = TriageIntegration::ALL
            .iter()
            .position(|o| *o == TriageIntegration::CherryPick)
            .unwrap();
    }
    app.pr_review_integrate_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().unwrap();
            assert!(
                integrate
                    .error
                    .as_deref()
                    .is_some_and(|e| e.contains("Cherry-pick refused")),
                "got {:?}",
                integrate.error
            );
            assert!(integrate.done.is_none());
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert_eq!(
        std::fs::read_to_string(source.join("a.txt")).unwrap(),
        "one\n",
        "the source worktree is untouched"
    );
}

#[test]
fn integrate_cherry_picks_into_a_clean_source_worktree() {
    let repo = TempDir::new().unwrap();
    let source = repo.path().join("source");
    std::fs::create_dir_all(&source).unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
    };
    git(&source, &["init", "-q", "-b", "main"]);
    git(&source, &["config", "user.email", "t@example.com"]);
    git(&source, &["config", "user.name", "T"]);
    std::fs::write(source.join("a.txt"), "one\n").unwrap();
    git(&source, &["add", "-A"]);
    git(&source, &["commit", "-qm", "base"]);
    let base_sha = String::from_utf8(
        std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&source)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();

    let triage = repo.path().join("triage");
    git(
        &source,
        &[
            "worktree",
            "add",
            "-b",
            "main-triage",
            triage.to_str().unwrap(),
            "main",
        ],
    );
    std::fs::write(triage.join("b.txt"), "fix\n").unwrap();
    git(&triage, &["add", "-A"]);
    git(&triage, &["commit", "-qm", "apply review comment"]);

    let mut store = store_with_feature(ProjectStatus::Stopped);
    store.projects[0].features[0].workdir = source.clone();
    store.projects[0].features[0].branch = "main".to_string();
    push_companion_feature(&mut store, AgentKind::Claude, VibeMode::Vibeless);
    store.projects[0].features[1].workdir = triage.clone();
    store.projects[0].features[1]
        .triage_source
        .as_mut()
        .unwrap()
        .base_sha = base_sha;

    let mut app = triage_target_app(store);
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.workdir = source.clone();
        state.fix_target = crate::app::pr_review::FixTarget::NewFeature;
    }
    app.pr_review_open_integrate();
    if let AppMode::PrReview(state) = &mut app.mode {
        let integrate = state.integrate.as_mut().unwrap();
        assert!(integrate.source_dirty.is_none());
        integrate.selected = crate::app::TriageIntegration::ALL
            .iter()
            .position(|o| *o == crate::app::TriageIntegration::CherryPick)
            .unwrap();
    }

    app.pr_review_integrate_confirm().unwrap();

    match &app.mode {
        AppMode::PrReview(state) => {
            let integrate = state.integrate.as_ref().unwrap();
            assert!(integrate.error.is_none(), "got {:?}", integrate.error);
            assert!(
                integrate
                    .done
                    .as_deref()
                    .is_some_and(|d| d.contains("Cherry-picked 1 commit")),
                "got {:?}",
                integrate.done
            );
        }
        other => panic!("expected PrReview, got {:?}", std::mem::discriminant(other)),
    }
    assert!(
        source.join("b.txt").exists(),
        "the triage commit landed in the source worktree"
    );
}

#[test]
fn closed_fetch_cannot_replace_a_reopened_pr_with_its_queued_result() {
    let mut app = pr_review_test_app();
    let workdir = tempfile::TempDir::new().unwrap();
    let old_review = pr_review_with_comments(1);
    let (old_tx, old_rx) = std::sync::mpsc::channel();
    app.pr_review_work.begin_fetch(old_rx);
    app.mode = AppMode::PrReviewLoading(PrReviewLoadState {
        workdir: workdir.path().to_path_buf(),
        pr: old_review.pr.clone(),
        usage_baselines: HashMap::new(),
    });
    old_tx.send(Ok(old_review)).unwrap();

    app.close_pr_review();
    assert!(!app.poll_pr_review_bg());
    assert!(matches!(app.mode, AppMode::Normal));

    let mut new_review = pr_review_with_comments(0);
    new_review.pr.number = 2;
    let (new_tx, new_rx) = std::sync::mpsc::channel();
    app.pr_review_work.begin_fetch(new_rx);
    app.mode = AppMode::PrReviewLoading(PrReviewLoadState {
        workdir: workdir.path().to_path_buf(),
        pr: new_review.pr.clone(),
        usage_baselines: HashMap::new(),
    });
    assert!(
        old_tx
            .send(Err(anyhow::anyhow!("late old failure")))
            .is_err()
    );
    new_tx.send(Ok(new_review)).unwrap();
    assert!(app.poll_pr_review_bg());
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected the reopened PR");
    };
    assert_eq!(state.review.pr.number, 2);
    assert!(state.review.comments.is_empty());
    assert!(!app.pr_review_work.fetch_pending());
}

/// PR Triage opened, then AI Review opened over it from `A`, with `n` findings.
fn ai_review_over_triage(app: &mut App, n: usize) {
    enter_pr_review(app, 2);
    app.open_ai_review_from_triage();
    if let AppMode::AiReview(state) = &mut app.mode {
        state.findings = (0..n)
            .map(|i| sample_ai_review_finding(&format!("finding {i}")))
            .collect();
    }
}

#[test]
fn ai_review_fix_hands_the_finding_to_triage_without_posting() {
    let mut app = pr_review_test_app();
    ai_review_over_triage(&mut app, 2);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.selected = 1;
    }

    app.ai_review_fix_selected();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR Triage after handing off a finding");
    };
    let comment = state.selected_comment().unwrap();
    assert!(comment.is_local_finding());
    assert!(comment.body.contains("finding 1"));
    let confirm = state.fix_confirm.as_ref().expect("fix dialog should open");
    assert!(confirm.batch.is_none());
    assert!(confirm.editor.text().contains("finding 1"));
    // Nothing to reply to on GitHub, so no reply-draft handoff.
    assert!(!confirm.editor.text().contains("amf reply-draft"));
    assert_eq!(confirm.reply_draft_requests[0].comment_id, comment.id);
    // The stashed pane was restored, not replaced by a fresh one.
    assert!(app.ai_review_return_to.is_none());
    assert_eq!(state.review.comments.len(), 3);
}

#[test]
fn ai_review_batch_fix_combines_marked_findings() {
    let mut app = pr_review_test_app();
    ai_review_over_triage(&mut app, 3);
    app.ai_review_toggle_mark();
    app.ai_review_select_next();
    app.ai_review_select_next();
    app.ai_review_toggle_mark();

    app.ai_review_fix_marked();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR Triage");
    };
    let confirm = state
        .fix_confirm
        .as_ref()
        .expect("batch dialog should open");
    assert_eq!(confirm.batch.as_ref().map(Vec::len), Some(2));
    assert!(confirm.editor.text().contains("finding 0"));
    assert!(confirm.editor.text().contains("finding 2"));
    assert!(!confirm.editor.text().contains("finding 1"));
    assert!(!confirm.editor.text().contains("amf reply-draft"));
}

#[test]
fn ai_review_fix_refuses_posted_and_skipped_findings() {
    let mut app = pr_review_test_app();
    ai_review_over_triage(&mut app, 2);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.findings[0].published = true;
        state.findings[1].skipped = true;
    }

    app.ai_review_fix_selected();
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    assert!(app.message.as_deref().unwrap().contains("Already posted"));

    app.ai_review_select_next();
    app.ai_review_toggle_mark();
    assert!(app.message.as_deref().unwrap().contains("Skipped"));
    app.ai_review_fix_marked();
    assert!(matches!(app.mode, AppMode::AiReview(_)));
}

#[test]
fn a_local_finding_cannot_be_replied_to_on_github() {
    let mut app = pr_review_test_app();
    ai_review_over_triage(&mut app, 1);
    app.ai_review_fix_selected();
    app.pr_review_cancel_fix();

    app.pr_review_open_reply_pick();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR Triage");
    };
    assert!(state.reply_kind_pick.is_none());
    assert!(app.message.as_deref().unwrap().contains("not posted"));
}

// ── PR picker Review tab ────────────────────────────────────────────────────

fn reviewable(number: u32, title: &str) -> crate::github::ReviewablePr {
    crate::github::ReviewablePr {
        number,
        title: title.to_string(),
        author: "alice".to_string(),
        is_draft: false,
        updated_at: String::new(),
        base_ref: "main".to_string(),
        base_oid: "base".to_string(),
        head_ref: "topic".to_string(),
        head_oid: "head".to_string(),
        is_cross_repository: false,
        head_owner: "acme".to_string(),
    }
}

use crate::app::pr_review::runtime::ReviewListLoaded;

/// The draft key the test loaders report, and the test opener pins.
const TEST_REPO_KEY: &str = "github.com/acme/widgets";

fn two_open_prs(_: &std::path::Path, _: bool) -> ReviewListLoaded {
    ReviewListLoaded {
        prs: Ok(vec![reviewable(7, "first"), reviewable(9, "second")]),
        current_user: Some("me".to_string()),
        repo_key: Some(TEST_REPO_KEY.to_string()),
    }
}

fn gh_not_signed_in(_: &std::path::Path, _: bool) -> ReviewListLoaded {
    ReviewListLoaded {
        prs: Err(anyhow::anyhow!(
            "`gh pr list` failed: To get started with GitHub CLI, please run:  gh auth login"
        )),
        current_user: None,
        repo_key: None,
    }
}

fn press(app: &mut App, code: KeyCode) {
    crate::handlers::handle_key(app, KeyEvent::from(code), 40).unwrap();
}

/// Poll until the Review tab's worker result has been applied.
fn settle_review_list(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.pr_review_work.review_list_pending() {
        app.poll_pr_review_list_bg();
        assert!(
            std::time::Instant::now() < deadline,
            "review list never loaded"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn review_list(app: &App) -> &PrReviewListState {
    match &app.mode {
        AppMode::PrReviewList(state) => state,
        _ => panic!("not on the Review tab"),
    }
}

#[test]
fn tab_switches_from_triage_to_review_and_back_restoring_the_triage_picker() {
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    enter_pr_picker_for_test(&mut app);
    if let AppMode::PrPicker(picker) = &mut app.mode {
        picker.include_closed = true;
    }

    press(&mut app, KeyCode::Tab);
    // The load is off-thread: the tab opens on "Loading", never blocking.
    assert_eq!(review_list(&app).load, PrReviewListLoad::Loading);
    assert!(review_list(&app).triage.is_some());
    settle_review_list(&mut app);
    match &review_list(&app).load {
        PrReviewListLoad::Loaded(prs) => {
            assert_eq!(prs.iter().map(|p| p.number).collect::<Vec<_>>(), [7, 9]);
        }
        other => panic!("expected a loaded list, got {other:?}"),
    }
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(review_list(&app).selected, 1);

    press(&mut app, KeyCode::BackTab);
    match &app.mode {
        AppMode::PrPicker(picker) => {
            assert!(picker.include_closed, "triage state came back verbatim");
            assert_eq!(
                picker.workdir,
                std::path::PathBuf::from("/tmp/test-workdir")
            );
        }
        _ => panic!("Tab should return to the Triage tab"),
    }
}

#[test]
fn review_list_load_failure_then_retry_then_success() {
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_review_list_loader_for_test(gh_not_signed_in);
    enter_pr_picker_for_test(&mut app);
    press(&mut app, KeyCode::Tab);
    settle_review_list(&mut app);
    match &review_list(&app).load {
        PrReviewListLoad::Failed(err) => {
            assert_eq!(err.kind, PrReviewListErrorKind::Auth);
            assert!(err.hint().contains("gh auth login"));
        }
        other => panic!("expected a failed load, got {other:?}"),
    }
    // A failed list must not cache "no gh user": the lookup never ran.
    assert_eq!(app.gh_current_user, None);

    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    press(&mut app, KeyCode::Char('r'));
    assert_eq!(review_list(&app).load, PrReviewListLoad::Loading);
    settle_review_list(&mut app);
    assert!(matches!(
        &review_list(&app).load,
        PrReviewListLoad::Loaded(prs) if prs.len() == 2
    ));
    assert_eq!(review_list(&app).current_user.as_deref(), Some("me"));
}

#[test]
fn a_review_list_result_after_the_tab_closed_is_dropped() {
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    enter_pr_picker_for_test(&mut app);
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(!app.pr_review_work.review_list_pending());
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!app.poll_pr_review_list_bg());
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn a_retry_supersedes_the_load_in_flight() {
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_review_list_loader_for_test(gh_not_signed_in);
    enter_pr_picker_for_test(&mut app);
    press(&mut app, KeyCode::Tab);
    let first = review_list(&app).request_id;
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    press(&mut app, KeyCode::Char('r'));
    assert_ne!(review_list(&app).request_id, first);
    settle_review_list(&mut app);
    // Only the retry's result lands, however the two threads were ordered.
    assert!(matches!(
        &review_list(&app).load,
        PrReviewListLoad::Loaded(_)
    ));
}

#[test]
fn dashboard_g_on_a_project_row_opens_the_review_tab_without_a_feature() {
    let mut app = App::new_for_test(
        store_with_feature(crate::project::ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    app.selection = Selection::Project(0);

    app.open_pr_review();

    let state = review_list(&app);
    assert_eq!(state.workdir, app.store.projects[0].repo);
    assert!(state.triage.is_none());
    settle_review_list(&mut app);
    assert!(matches!(
        &review_list(&app).load,
        PrReviewListLoad::Loaded(_)
    ));
}

#[test]
fn review_list_errors_are_classified_by_what_the_user_can_do() {
    let classify = |s: &str| PrReviewListError::classify(s.to_string()).kind;
    assert_eq!(
        classify("Failed to run `gh`."),
        PrReviewListErrorKind::GhMissing
    );
    assert_eq!(
        classify("gh: HTTP 401: Bad credentials"),
        PrReviewListErrorKind::Auth
    );
    assert_eq!(
        classify("`gh pr list` failed: error connecting to api.github.com"),
        PrReviewListErrorKind::Other
    );
}

// ── Opening a PR for review from the Review tab ─────────────────────────────

fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// A repo whose `teammate-pr` branch stands in for PR #9, checked out on an
/// unrelated feature branch with a dirty tracked file and an untracked one.
fn repo_with_teammate_pr() -> TempDir {
    let repo = TempDir::new().unwrap();
    let dir = repo.path();
    git_out(dir, &["init", "--quiet", "--initial-branch=main"]);
    git_out(dir, &["config", "user.name", "AMF Test"]);
    git_out(dir, &["config", "user.email", "amf@example.com"]);
    std::fs::write(dir.join("shared.rs"), "fn shared() {}\n").unwrap();
    git_out(dir, &["add", "shared.rs"]);
    git_out(dir, &["commit", "--quiet", "-m", "initial"]);
    git_out(dir, &["checkout", "--quiet", "-b", "teammate-pr"]);
    std::fs::write(dir.join("pr_only.rs"), "fn pr() {}\n").unwrap();
    git_out(dir, &["add", "pr_only.rs"]);
    git_out(dir, &["commit", "--quiet", "-m", "pr change"]);
    git_out(dir, &["checkout", "--quiet", "main"]);
    git_out(dir, &["checkout", "--quiet", "-b", "my-feature"]);
    std::fs::write(dir.join("shared.rs"), "local dirt\n").unwrap();
    std::fs::write(dir.join("untracked.rs"), "stray\n").unwrap();
    repo
}

/// Stands in for fetching PR #9: pins the repo's own branches and, like the
/// real fetch, leaves a private review ref behind for close to remove.
fn open_teammate_pr(
    workdir: &std::path::Path,
    pr: &crate::github::ReviewablePr,
) -> anyhow::Result<PrDiffTarget> {
    let head = git_out(workdir, &["rev-parse", "teammate-pr"]);
    let base = git_out(workdir, &["rev-parse", "main"]);
    let merge_base = git_out(workdir, &["merge-base", &base, &head]);
    git_out(workdir, &["update-ref", "refs/amf/review/9/head", &head]);
    let mut pinned = pr.clone();
    pinned.head_oid = head;
    pinned.base_oid = base;
    Ok(PrDiffTarget {
        repo: TEST_REPO_KEY.to_string(),
        pr: pinned,
        merge_base_oid: merge_base,
    })
}

fn refuse_to_open(
    _: &std::path::Path,
    _: &crate::github::ReviewablePr,
) -> anyhow::Result<PrDiffTarget> {
    Err(anyhow::anyhow!(
        "`git fetch` for PR #9 failed: network down"
    ))
}

fn settle_review_open(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.pr_review_work.review_open_pending() {
        app.poll_pr_review_open_bg();
        assert!(std::time::Instant::now() < deadline, "PR never opened");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// An app on the Review tab of `repo`'s project, loaded with PRs #7 and #9
/// and the cursor on #9. The tmux mock has no expectations: any tmux call
/// (a window, a session) fails the test.
fn review_tab_on(repo: &std::path::Path) -> App {
    let mut app = App::new_for_test(
        store_with_repo(repo.to_path_buf(), crate::project::ProjectStatus::Active),
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    app.selection = Selection::Project(0);
    app.open_pr_review();
    settle_review_list(&mut app);
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(review_list(&app).selected, 1);
    app
}

fn viewer(app: &App) -> &DiffViewerState {
    match &app.mode {
        AppMode::DiffViewer(state) | AppMode::DiffViewerLoading(state) => state,
        _ => panic!("not in the diff viewer"),
    }
}

#[test]
fn enter_opens_the_pr_in_the_review_viewer_and_esc_returns_to_the_same_row() {
    let repo = repo_with_teammate_pr();
    let dir = repo.path();
    let mut app = review_tab_on(dir);
    app.pr_review_work
        .set_review_opener_for_test(open_teammate_pr);
    let features_before = app.store.projects[0].features.len();

    press(&mut app, KeyCode::Enter);
    assert_eq!(
        review_list(&app).opening.as_ref().map(|o| o.number),
        Some(9)
    );
    settle_review_open(&mut app);
    assert!(matches!(app.mode, AppMode::DiffViewerLoading(_)));
    app.complete_diff_viewer_loading();

    let state = viewer(&app);
    assert!(state.review && state.is_pr_review());
    let paths: Vec<&str> = state.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        ["pr_only.rs"],
        "only the PR's files, none of the feature's"
    );
    // No feature, session, or tmux window came into being.
    assert_eq!(app.store.projects[0].features.len(), features_before);
    assert!(
        app.store.projects[0]
            .features
            .iter()
            .all(|f| f.sessions.is_empty())
    );

    press(&mut app, KeyCode::Esc);
    let list = review_list(&app);
    assert_eq!(list.selected, 1, "the cursor is where it was");
    assert!(matches!(&list.load, PrReviewListLoad::Loaded(prs) if prs.len() == 2));
    assert!(list.opening.is_none());

    // Leaving drops the private review refs (off the UI thread).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !git_out(dir, &["for-each-ref", "refs/amf/review/"]).is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "review refs never removed"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn a_pr_review_never_writes_into_the_checkout_and_says_why_it_refuses() {
    let repo = repo_with_teammate_pr();
    let dir = repo.path();
    let status_before = git_out(dir, &["status", "--porcelain=v1", "--untracked-files=all"]);
    let mut app = review_tab_on(dir);
    app.pr_review_work
        .set_review_opener_for_test(open_teammate_pr);
    press(&mut app, KeyCode::Enter);
    settle_review_open(&mut app);
    app.complete_diff_viewer_loading();

    // Local-checkout actions refuse with a reason, and the review stays open.
    // (`q` is not among them: it submits to GitHub instead of finishing into
    // an agent — see the submit tests.)
    for key in ['t', 'X', 'b', 'w', 'A', 'O', 'H', 'I'] {
        app.message = None;
        press(&mut app, KeyCode::Char(key));
        let message = app.message.clone().unwrap_or_default();
        assert!(
            message.starts_with("Not available in a PR review:"),
            "`{key}` gave {message:?}"
        );
        assert!(
            matches!(app.mode, AppMode::DiffViewer(_)),
            "`{key}` left the review"
        );
    }
    // `q` opens the submit dialog rather than finishing, and Esc closes it.
    press(&mut app, KeyCode::Char('q'));
    assert!(app.pr_submit_open());
    press(&mut app, KeyCode::Esc);
    assert!(!app.pr_submit_open());
    assert!(matches!(app.mode, AppMode::DiffViewer(_)));
    for refusal in [
        App::diff_review_open_in_editor as fn(&mut App),
        App::diff_review_apply_suggestion_under_cursor,
    ] {
        app.message = None;
        refusal(&mut app);
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.starts_with("Not available in a PR review:"))
        );
    }

    // Reviewing actions still work, and persist nothing into the checkout.
    press(&mut app, KeyCode::Char('a'));
    assert!(viewer(&app).decisions.contains_key("pr_only.rs"));
    app.persist_review_progress();
    assert!(!dir.join(".claude").exists());
    assert_eq!(
        git_out(dir, &["status", "--porcelain=v1", "--untracked-files=all"]),
        status_before
    );
}

#[test]
fn esc_while_a_pr_is_opening_stops_it_and_keeps_the_tab() {
    let repo = repo_with_teammate_pr();
    let mut app = review_tab_on(repo.path());
    app.pr_review_work
        .set_review_opener_for_test(open_teammate_pr);
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Esc);

    assert!(review_list(&app).opening.is_none());
    assert!(!app.pr_review_work.review_open_pending());
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(!app.poll_pr_review_open_bg());
    assert!(matches!(app.mode, AppMode::PrReviewList(_)));
}

#[test]
fn a_pr_that_fails_to_open_stays_on_the_tab_with_the_reason() {
    let repo = repo_with_teammate_pr();
    let mut app = review_tab_on(repo.path());
    app.pr_review_work
        .set_review_opener_for_test(refuse_to_open);
    press(&mut app, KeyCode::Enter);
    settle_review_open(&mut app);

    let list = review_list(&app);
    assert!(list.opening.is_none());
    assert!(
        list.open_error
            .as_deref()
            .is_some_and(|e| e.contains("network down"))
    );
}

// ── PR review drafts ────────────────────────────────────────────────────────

/// `review_tab_on`, with a real database attached and the test opener.
fn review_tab_with_db(repo: &std::path::Path, db_file: &NamedTempFile) -> App {
    let mut app = review_tab_on(repo);
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    app.pr_review_work
        .set_review_opener_for_test(open_teammate_pr);
    // Reload so the badges read the database that is now attached.
    press(&mut app, KeyCode::Char('r'));
    settle_review_list(&mut app);
    app
}

fn open_selected_pr(app: &mut App) {
    press(app, KeyCode::Enter);
    settle_review_open(app);
    app.complete_diff_viewer_loading();
    assert!(viewer(app).is_pr_review());
}

/// Comment on the first commentable line of the current file.
fn comment_first_line(app: &mut App, text: &str) {
    press(app, KeyCode::Char('c'));
    press(app, KeyCode::Enter);
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
    press(app, KeyCode::Tab);
    press(app, KeyCode::Esc); // leave the line cursor
}

fn saved_draft(app: &App) -> Option<crate::db::pr_review_drafts::PrReviewDraft> {
    app.db
        .as_ref()
        .unwrap()
        .load_pr_review_draft(TEST_REPO_KEY, 9)
        .unwrap()
}

fn line_comment_texts(app: &App) -> Vec<String> {
    let mut texts: Vec<String> = viewer(app)
        .line_comments
        .values()
        .flatten()
        .map(|c| c.text.clone())
        .collect();
    texts.sort();
    texts
}

#[test]
fn a_pr_review_draft_survives_leaving_and_reopening() {
    let repo = repo_with_teammate_pr();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(repo.path(), &db_file);
    open_selected_pr(&mut app);

    comment_first_line(&mut app, "why pr?");
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, AppMode::PrReviewList(_)));
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("draft saved"))
    );

    let draft = saved_draft(&app).expect("the draft is in the database");
    assert_eq!(
        draft.head_oid,
        git_out(repo.path(), &["rev-parse", "teammate-pr"])
    );
    assert!(draft.file_fingerprints.contains_key("pr_only.rs"));
    // …and nothing was written into the checkout.
    assert!(!repo.path().join(".claude").exists());

    // The list shows the draft; reopening resumes it.
    press(&mut app, KeyCode::Char('r'));
    settle_review_list(&mut app);
    assert_eq!(
        review_list(&app).drafts.get(&9).map(|d| d.comments),
        Some(1)
    );
    open_selected_pr(&mut app);
    assert_eq!(line_comment_texts(&app), ["why pr?"]);
    assert!(viewer(&app).decisions.contains_key("pr_only.rs"));
}

#[test]
fn opening_a_pr_without_reviewing_it_leaves_no_draft() {
    let repo = repo_with_teammate_pr();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(repo.path(), &db_file);
    open_selected_pr(&mut app);
    press(&mut app, KeyCode::Esc);
    assert_eq!(saved_draft(&app), None);
}

#[test]
fn a_moved_pr_keeps_its_draft_comments_but_not_its_verdicts() {
    let repo = repo_with_teammate_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    open_selected_pr(&mut app);
    comment_first_line(&mut app, "keep me");
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Esc);

    // The author pushes again (without touching the user's checkout).
    git_out(
        dir,
        &["worktree", "add", "--quiet", "../pr-wt", "teammate-pr"],
    );
    let pr_wt = dir.parent().unwrap().join("pr-wt");
    std::fs::write(pr_wt.join("pr_only.rs"), "fn pr() {}\nfn more() {}\n").unwrap();
    git_out(&pr_wt, &["commit", "--quiet", "-am", "second push"]);
    git_out(
        dir,
        &["worktree", "remove", "--force", pr_wt.to_str().unwrap()],
    );
    // The list's rows now report the new head, so the badge says "updated".
    let moved_head = git_out(dir, &["rev-parse", "teammate-pr"]);
    press(&mut app, KeyCode::Char('r'));
    settle_review_list(&mut app);
    assert_ne!(
        review_list(&app)
            .drafts
            .get(&9)
            .map(|d| d.head_oid.as_str()),
        Some(moved_head.as_str())
    );

    open_selected_pr(&mut app);
    assert_eq!(line_comment_texts(&app), ["keep me"]);
    assert!(
        viewer(&app).decisions.is_empty(),
        "an approval of older code is not kept"
    );
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("changed since your draft")),
        "{:?}",
        app.message
    );
}

#[test]
fn comments_on_a_file_that_left_the_pr_are_kept_in_the_draft() {
    let repo = repo_with_teammate_pr();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(repo.path(), &db_file);
    // A draft saved when the PR still touched `gone.rs`.
    let draft = crate::db::pr_review_drafts::PrReviewDraft {
        repo_key: TEST_REPO_KEY.to_string(),
        pr_number: 9,
        base_oid: "old-base".to_string(),
        head_oid: "old-head".to_string(),
        merge_base_oid: "old-mb".to_string(),
        status: crate::db::pr_review_drafts::PrReviewDraftStatus::Draft,
        progress: serde_json::json!({
            "line_comments": {"gone.rs": [{
                "location": {"old_line": null, "new_line": 1},
                "text": "about a file the PR dropped"
            }]}
        })
        .to_string(),
        file_fingerprints: Default::default(),
        updated_at: "2026-09-25T00:00:00Z".to_string(),
    };
    app.db
        .as_ref()
        .unwrap()
        .upsert_pr_review_draft(&draft)
        .unwrap();

    open_selected_pr(&mut app);
    assert!(
        viewer(&app).line_comments.is_empty(),
        "not shown on a file not in the diff"
    );
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("1 comment(s) are on files no longer in the PR")),
        "{:?}",
        app.message
    );
    comment_first_line(&mut app, "new one");
    press(&mut app, KeyCode::Esc);

    let saved = saved_draft(&app).unwrap();
    assert_eq!(crate::app::review::draft_comment_count(&saved.progress), 2);
    assert!(saved.progress.contains("about a file the PR dropped"));
}

#[test]
fn reloading_the_review_list_keeps_the_rows_and_the_highlighted_pr() {
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs);
    enter_pr_picker_for_test(&mut app);
    press(&mut app, KeyCode::Tab);
    settle_review_list(&mut app);
    press(&mut app, KeyCode::Char('j'));

    press(&mut app, KeyCode::Char('r'));
    // The rows stay up (and navigable) while the reload is in flight.
    assert!(review_list(&app).reloading);
    assert!(matches!(
        &review_list(&app).load,
        PrReviewListLoad::Loaded(_)
    ));
    settle_review_list(&mut app);

    assert!(!review_list(&app).reloading);
    assert_eq!(
        review_list(&app).selected,
        1,
        "still on #9, not back to the top"
    );
}

#[test]
fn leaving_a_pr_review_says_so_when_the_draft_could_not_be_saved() {
    let repo = repo_with_teammate_pr();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(repo.path(), &db_file);
    open_selected_pr(&mut app);
    // Break the table out from under AMF with a second connection.
    rusqlite::Connection::open(db_file.path())
        .unwrap()
        .execute_batch("DROP TABLE pr_review_drafts;")
        .unwrap();

    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Esc);

    let message = app.message.clone().unwrap_or_default();
    assert!(message.contains("draft NOT saved"), "{message}");
    assert!(!message.contains("draft saved;"), "{message}");
}

// ── PR updates: what a new push does to a saved draft ───────────────────────

/// PR #9 (`teammate-pr`) touching two files: `edit.rs`, whose added lines the
/// author will later replace, and `keep.rs`, which the author won't touch
/// again. The checkout sits on an unrelated branch.
fn repo_with_two_file_pr() -> TempDir {
    let repo = TempDir::new().unwrap();
    let dir = repo.path();
    git_out(dir, &["init", "--quiet", "--initial-branch=main"]);
    git_out(dir, &["config", "user.name", "AMF Test"]);
    git_out(dir, &["config", "user.email", "amf@example.com"]);
    std::fs::write(dir.join("edit.rs"), "fn edit() {}\n").unwrap();
    std::fs::write(dir.join("keep.rs"), "fn keep() {}\n").unwrap();
    git_out(dir, &["add", "."]);
    git_out(dir, &["commit", "--quiet", "-m", "initial"]);
    git_out(dir, &["checkout", "--quiet", "-b", "teammate-pr"]);
    std::fs::write(
        dir.join("edit.rs"),
        "fn edit() {}\nfn old_a() {}\nfn old_b() {}\n",
    )
    .unwrap();
    std::fs::write(dir.join("keep.rs"), "fn keep() {}\nfn kept_line() {}\n").unwrap();
    git_out(dir, &["commit", "--quiet", "-am", "pr v1"]);
    git_out(dir, &["checkout", "--quiet", "main"]);
    git_out(dir, &["checkout", "--quiet", "-b", "my-feature"]);
    repo
}

/// The author pushes a new commit to the PR branch, via a scratch worktree
/// so the user's checkout is never touched.
fn push_to_pr(dir: &std::path::Path, path: &str, content: &str) {
    let wt = dir.parent().unwrap().join(format!(
        "pr-push-{}",
        dir.file_name().unwrap().to_string_lossy()
    ));
    git_out(
        dir,
        &[
            "worktree",
            "add",
            "--quiet",
            wt.to_str().unwrap(),
            "teammate-pr",
        ],
    );
    std::fs::write(wt.join(path), content).unwrap();
    git_out(&wt, &["commit", "--quiet", "-am", "pr push"]);
    git_out(
        dir,
        &["worktree", "remove", "--force", wt.to_str().unwrap()],
    );
}

fn select_file(app: &mut App, path: &str) {
    let AppMode::DiffViewer(state) = &mut app.mode else {
        panic!("not in the viewer");
    };
    state.selected_file = state.files.iter().position(|f| f.path == path).unwrap();
    state.on_file_changed();
}

/// Comment (with the real keys) on the current file's line containing `needle`.
fn comment_on_line_containing(app: &mut App, needle: &str, text: &str) {
    let AppMode::DiffViewer(state) = &mut app.mode else {
        panic!("not in the viewer");
    };
    let file = &state.files[state.selected_file];
    let index = file
        .addressable_line_texts()
        .iter()
        .position(|line| line.contains(needle))
        .unwrap_or_else(|| panic!("no line containing {needle:?}"));
    state.comment_cursor = Some(index);
    press(app, KeyCode::Enter);
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
    press(app, KeyCode::Tab);
    press(app, KeyCode::Esc); // leave the line cursor
}

fn comment<'a>(app: &'a App, path: &str, text: &str) -> &'a LineComment {
    viewer(app).line_comments[path]
        .iter()
        .find(|c| c.text == text)
        .unwrap_or_else(|| panic!("no comment {text:?} on {path}"))
}

#[test]
fn a_pr_update_flags_only_changed_files_and_marks_comments_on_removed_code_outdated() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    open_selected_pr(&mut app);

    // Review v1: approve both files, comment on code in each.
    select_file(&mut app, "edit.rs");
    comment_on_line_containing(&mut app, "old_a", "rename this");
    select_file(&mut app, "keep.rs");
    comment_on_line_containing(&mut app, "kept_line", "nice");
    for path in ["edit.rs", "keep.rs"] {
        select_file(&mut app, path);
        press(&mut app, KeyCode::Char('a'));
    }
    assert_eq!(viewer(&app).decisions.len(), 2);
    press(&mut app, KeyCode::Esc);

    // The author replaces the lines the edit.rs comment was on.
    push_to_pr(dir, "edit.rs", "fn edit() {}\nfn new_c() {}\n");
    open_selected_pr(&mut app);
    let state = viewer(&app);

    // Only the changed file is flagged, filtered to, and loses its verdict.
    assert_eq!(
        state.changed_since_last,
        std::collections::HashSet::from(["edit.rs".to_string()])
    );
    assert!(state.has_prior_review);
    assert_eq!(state.file_filter, FileFilter::Changed);
    assert_eq!(state.files[state.selected_file].path, "edit.rs");
    assert!(
        !state.decisions.contains_key("edit.rs"),
        "changed code is re-reviewed"
    );
    assert_eq!(
        state.decisions.get("keep.rs"),
        Some(&ReviewDecision::Approve)
    );

    // Both comments are kept: the one on removed code is outdated, the one on
    // untouched code still sits on its line.
    assert!(comment(&app, "edit.rs", "rename this").anchor_lost);
    let kept = comment(&app, "keep.rs", "nice");
    assert!(!kept.anchor_lost);
    let keep_file = state.files.iter().find(|f| f.path == "keep.rs").unwrap();
    let kept_index = keep_file
        .addressable_lines()
        .iter()
        .position(|loc| *loc == kept.location)
        .unwrap();
    assert!(keep_file.addressable_line_texts()[kept_index].contains("kept_line"));

    let message = app.message.clone().unwrap_or_default();
    assert!(message.contains("1 of 2 file(s) changed"), "{message}");
    assert!(
        message.contains("1 verdict(s) on them cleared"),
        "{message}"
    );
    assert!(message.contains("1 comment(s) are outdated"), "{message}");

    // The outdated comment survives the next save too.
    press(&mut app, KeyCode::Esc);
    let saved = saved_draft(&app).unwrap();
    assert!(saved.progress.contains("rename this"));
    assert_eq!(
        saved.head_oid,
        git_out(dir, &["rev-parse", "teammate-pr"]),
        "the draft now describes the revision just reviewed"
    );
}

#[test]
fn a_new_push_that_changes_no_file_keeps_every_verdict() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    open_selected_pr(&mut app);
    for path in ["edit.rs", "keep.rs"] {
        select_file(&mut app, path);
        press(&mut app, KeyCode::Char('a'));
    }
    press(&mut app, KeyCode::Esc);

    // A new, empty commit: the head moves, but no file's diff changes.
    let empty = git_out(
        dir,
        &[
            "commit-tree",
            "-m",
            "empty push",
            "-p",
            "teammate-pr",
            "teammate-pr^{tree}",
        ],
    );
    git_out(dir, &["update-ref", "refs/heads/teammate-pr", &empty]);

    open_selected_pr(&mut app);
    let state = viewer(&app);
    assert!(state.changed_since_last.is_empty());
    assert_eq!(state.decisions.len(), 2);
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("none of its files' changes differ")),
        "{:?}",
        app.message
    );
}

#[test]
fn the_pr_notes_panel_shows_changes_and_outdated_comments_and_the_footer_only_usable_keys() {
    use ratatui::{Terminal, backend::TestBackend};

    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    open_selected_pr(&mut app);
    select_file(&mut app, "edit.rs");
    comment_on_line_containing(&mut app, "old_a", "rename this");
    press(&mut app, KeyCode::Esc);
    push_to_pr(dir, "edit.rs", "fn edit() {}\nfn new_c() {}\n");
    open_selected_pr(&mut app);
    select_file(&mut app, "edit.rs");
    press(&mut app, KeyCode::Char('e')); // expand the notes panel

    let mut terminal = Terminal::new(TestBackend::new(160, 50)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(screen.contains("PR Review Notes"), "{screen}");
    assert!(screen.contains("Changed since your draft"), "{screen}");
    assert!(screen.contains("Outdated comments"), "{screen}");
    assert!(screen.contains("rename this"), "{screen}");
    assert!(
        !screen.contains("review-notes.md"),
        "no feature-notes placeholder"
    );

    // The footer offers only what a PR review can do.
    press(&mut app, KeyCode::Char('e'));
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    for refused in [
        "$EDITOR",
        "gen walkthrough",
        "AI review",
        " overview",
        " history",
        "since last review",
        "base ref",
        "target:",
        "→ finish",
    ] {
        assert!(!screen.contains(refused), "footer still offers {refused:?}");
    }
    assert!(screen.contains("pause (draft saved)"), "{screen}");
}

// ── Submitting a PR review ──────────────────────────────────────────────────

use crate::app::pr_review::runtime::{PrPostOutcome, PrPostRequest};

/// What the stub posters were asked to post, by the test's repo path, so
/// parallel tests never read each other's requests.
fn posted_requests() -> &'static std::sync::Mutex<HashMap<std::path::PathBuf, Vec<PrPostRequest>>> {
    static POSTED: std::sync::OnceLock<
        std::sync::Mutex<HashMap<std::path::PathBuf, Vec<PrPostRequest>>>,
    > = std::sync::OnceLock::new();
    POSTED.get_or_init(Default::default)
}

fn record(request: &PrPostRequest) {
    posted_requests()
        .lock()
        .unwrap()
        .entry(request.workdir.clone())
        .or_default()
        .push(request.clone());
}

fn requests_for(dir: &std::path::Path) -> Vec<PrPostRequest> {
    posted_requests()
        .lock()
        .unwrap()
        .get(dir)
        .cloned()
        .unwrap_or_default()
}

fn github_accepts(request: &PrPostRequest) -> PrPostOutcome {
    record(request);
    PrPostOutcome::Posted {
        file_comment_failures: vec![],
    }
}

fn github_is_down(request: &PrPostRequest) -> PrPostOutcome {
    record(request);
    PrPostOutcome::Failed("`gh api` (create review) failed: HTTP 502".to_string())
}

fn pr_has_moved(request: &PrPostRequest) -> PrPostOutcome {
    record(request);
    PrPostOutcome::HeadMoved {
        current_head: "feedface00000000000000000000000000000000".to_string(),
    }
}

fn settle_review_post(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.pr_review_work.review_post_pending() {
        app.poll_pr_review_post_bg();
        assert!(std::time::Instant::now() < deadline, "post never answered");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn submit_status(app: &App) -> PrSubmitStatus {
    viewer(app)
        .pr_submit
        .as_ref()
        .expect("the submit dialog is open")
        .status
        .clone()
}

/// Type into the open general-feedback (summary) editor and save it.
fn write_summary(app: &mut App, text: &str) {
    press(app, KeyCode::Char('e'));
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
    press(app, KeyCode::Tab);
}

/// The two-file PR, opened for review, with a single-line comment on `edit.rs`.
fn reviewed_two_file_pr(
    repo: &std::path::Path,
    db_file: &NamedTempFile,
    poster: crate::app::pr_review::runtime::ReviewPoster,
) -> App {
    let mut app = review_tab_with_db(repo, db_file);
    app.pr_review_work.set_review_poster_for_test(poster);
    open_selected_pr(&mut app);
    select_file(&mut app, "edit.rs");
    comment_on_line_containing(&mut app, "old_a", "rename this");
    app
}

#[test]
fn each_review_event_posts_with_its_github_name_pinned_to_the_reviewed_head() {
    for (steps, expected) in [(0, "COMMENT"), (1, "APPROVE"), (2, "REQUEST_CHANGES")] {
        let repo = repo_with_two_file_pr();
        let dir = repo.path();
        let db_file = NamedTempFile::new().unwrap();
        let mut app = reviewed_two_file_pr(dir, &db_file, github_accepts);

        press(&mut app, KeyCode::Char('q'));
        assert_eq!(submit_status(&app), PrSubmitStatus::Ready);
        for _ in 0..steps {
            press(&mut app, KeyCode::Right);
        }
        write_summary(&mut app, "overall fine");
        press(&mut app, KeyCode::Enter);
        settle_review_post(&mut app);

        let requests = requests_for(dir);
        assert_eq!(requests.len(), 1, "{expected}");
        let request = &requests[0];
        assert_eq!(request.event.api_name(), expected);
        assert_eq!(request.pr.number, 9);
        assert_eq!(
            (request.pr.owner.as_str(), request.pr.repo.as_str()),
            ("acme", "widgets")
        );
        assert_eq!(
            request.pr.head_sha,
            git_out(dir, &["rev-parse", "teammate-pr"])
        );
        assert_eq!(request.body, "overall fine");
        assert_eq!(request.comments.len(), 1);
        assert!(request.comments[0].body.contains("rename this"));

        // Posted: back on the list, the draft marked posted, no badge.
        assert!(matches!(app.mode, AppMode::PrReviewList(_)), "{expected}");
        assert!(!review_list(&app).drafts.contains_key(&9));
        assert_eq!(
            saved_draft(&app).unwrap().status,
            crate::db::pr_review_drafts::PrReviewDraftStatus::Posted
        );
        assert!(
            app.message
                .as_deref()
                .is_some_and(|m| m.contains("Posted a") && m.contains("1 inline comment")),
            "{:?}",
            app.message
        );
    }
}

#[test]
fn a_range_comment_and_a_suggestion_post_as_github_expects() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    app.pr_review_work
        .set_review_poster_for_test(github_accepts);
    open_selected_pr(&mut app);
    select_file(&mut app, "edit.rs");

    // A range over old_a..old_b.
    {
        let AppMode::DiffViewer(state) = &mut app.mode else {
            unreachable!()
        };
        let file = &state.files[state.selected_file];
        let start = file
            .addressable_line_texts()
            .iter()
            .position(|l| l.contains("old_a"))
            .unwrap();
        state.comment_cursor = Some(start);
    }
    press(&mut app, KeyCode::Char('v'));
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Enter);
    for ch in "both of these".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Esc);

    // A suggestion on keep.rs's added line.
    select_file(&mut app, "keep.rs");
    {
        let AppMode::DiffViewer(state) = &mut app.mode else {
            unreachable!()
        };
        let file = &state.files[state.selected_file];
        state.comment_cursor = file
            .addressable_line_texts()
            .iter()
            .position(|l| l.contains("kept_line"));
    }
    press(&mut app, KeyCode::Char('S'));
    for ch in "_v2".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Esc);

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);
    settle_review_post(&mut app);

    let request = requests_for(dir).pop().expect("posted");
    let range = request
        .comments
        .iter()
        .find(|c| c.path == "edit.rs")
        .unwrap();
    assert_eq!(
        (range.start_line, range.line, range.side, range.start_side),
        (Some(2), 3, "RIGHT", Some("RIGHT"))
    );
    assert!(range.body.contains("both of these"));
    let suggestion = request
        .comments
        .iter()
        .find(|c| c.path == "keep.rs")
        .unwrap();
    assert_eq!(suggestion.line, 2);
    assert!(
        suggestion.body.contains("```suggestion\n"),
        "{}",
        suggestion.body
    );
    assert!(suggestion.body.contains("_v2"), "{}", suggestion.body);
}

#[test]
fn a_failed_post_keeps_the_draft_and_a_retry_posts_it() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = reviewed_two_file_pr(dir, &db_file, github_is_down);

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);
    settle_review_post(&mut app);

    match submit_status(&app) {
        PrSubmitStatus::Failed(err) => assert!(err.contains("HTTP 502"), "{err}"),
        other => panic!("expected a failure, got {other:?}"),
    }
    assert!(
        matches!(app.mode, AppMode::DiffViewer(_)),
        "still reviewing"
    );
    let draft = saved_draft(&app).expect("saved before posting");
    assert_eq!(
        draft.status,
        crate::db::pr_review_drafts::PrReviewDraftStatus::Draft
    );

    app.pr_review_work
        .set_review_poster_for_test(github_accepts);
    press(&mut app, KeyCode::Enter); // retry
    settle_review_post(&mut app);

    assert_eq!(requests_for(dir).len(), 2);
    assert!(matches!(app.mode, AppMode::PrReviewList(_)));
    assert_eq!(
        saved_draft(&app).unwrap().status,
        crate::db::pr_review_drafts::PrReviewDraftStatus::Posted
    );

    // Reopening starts a new review rather than re-posting the old one.
    open_selected_pr(&mut app);
    assert!(viewer(&app).line_comments.is_empty());
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("already posted")),
        "{:?}",
        app.message
    );
}

#[test]
fn a_pr_that_moved_blocks_the_post_and_o_reopens_it_at_the_new_head() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = reviewed_two_file_pr(dir, &db_file, pr_has_moved);

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);
    settle_review_post(&mut app);
    assert!(matches!(
        submit_status(&app),
        PrSubmitStatus::HeadMoved { current_head } if current_head.starts_with("feedface")
    ));

    // Enter doesn't try again: the only way on is to look at the new code.
    press(&mut app, KeyCode::Enter);
    assert!(!app.pr_review_work.review_post_pending());
    assert_eq!(requests_for(dir).len(), 1);

    press(&mut app, KeyCode::Char('o'));
    settle_review_open(&mut app);
    app.complete_diff_viewer_loading();
    assert!(viewer(&app).is_pr_review());
    assert!(viewer(&app).pr_submit.is_none());
    assert_eq!(line_comment_texts(&app), ["rename this"], "draft kept");
}

#[test]
fn approve_and_request_changes_are_refused_on_your_own_pr_before_posting() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = reviewed_two_file_pr(dir, &db_file, github_accepts);
    app.gh_current_user = Some(Some("Alice".to_string())); // the PR's author

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Right); // Approve
    press(&mut app, KeyCode::Enter);

    match submit_status(&app) {
        PrSubmitStatus::Failed(err) => assert!(err.contains("your own PR"), "{err}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert!(!app.pr_review_work.review_post_pending());
    assert!(requests_for(dir).is_empty());
}

#[test]
fn an_empty_review_is_refused_before_posting() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = review_tab_with_db(dir, &db_file);
    app.pr_review_work
        .set_review_poster_for_test(github_accepts);
    open_selected_pr(&mut app);

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        submit_status(&app),
        PrSubmitStatus::Failed(err) if err.contains("Nothing to post")
    ));

    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Right); // Request changes
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        submit_status(&app),
        PrSubmitStatus::Failed(err) if err.contains("needs a summary")
    ));
    assert!(requests_for(dir).is_empty());
}

#[test]
fn esc_cannot_abandon_a_post_in_flight() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = reviewed_two_file_pr(dir, &db_file, github_accepts);
    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);

    // The answer isn't polled yet, so the post is still in flight.
    press(&mut app, KeyCode::Esc);
    assert!(matches!(
        submit_status(&app),
        PrSubmitStatus::Posting { .. }
    ));
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("wait for its answer"))
    );
    settle_review_post(&mut app);
    assert!(matches!(app.mode, AppMode::PrReviewList(_)));
}

#[test]
fn comments_that_cannot_go_on_the_diff_are_posted_in_the_summary() {
    let repo = repo_with_two_file_pr();
    let dir = repo.path();
    let db_file = NamedTempFile::new().unwrap();
    let mut app = reviewed_two_file_pr(dir, &db_file, github_accepts);
    press(&mut app, KeyCode::Esc);
    // The lines the comment was on are replaced: it becomes outdated.
    push_to_pr(dir, "edit.rs", "fn edit() {}\nfn new_c() {}\n");
    open_selected_pr(&mut app);
    assert!(comment(&app, "edit.rs", "rename this").anchor_lost);

    press(&mut app, KeyCode::Char('q'));
    press(&mut app, KeyCode::Enter);
    settle_review_post(&mut app);

    let request = requests_for(dir).pop().expect("posted");
    assert!(
        request.comments.is_empty(),
        "nothing inline: its code is gone"
    );
    assert!(
        request
            .body
            .contains("Comments that couldn't be placed on the diff"),
        "{}",
        request.body
    );
    assert!(request.body.contains("`edit.rs`"), "{}", request.body);
    assert!(
        request.body.contains("code no longer in the PR"),
        "{}",
        request.body
    );
    assert!(request.body.contains("rename this"), "{}", request.body);
}

// ── Acceptance: a real PR on GitHub (opt-in) ────────────────────────────────

/// Everything a PR review must leave byte-identical in a checkout.
fn checkout_snapshot(dir: &std::path::Path) -> Vec<String> {
    vec![
        git_out(dir, &["rev-parse", "--symbolic-full-name", "HEAD"]),
        git_out(dir, &["rev-parse", "HEAD"]),
        git_out(dir, &["status", "--porcelain=v1", "--untracked-files=all"]),
        git_out(dir, &["ls-files", "--stage"]),
        git_out(dir, &["diff", "--cached"]),
        git_out(dir, &["diff"]),
        git_out(dir, &["stash", "list", "--format=%H %gs"]),
        git_out(
            dir,
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                "refs/heads",
                "refs/remotes",
            ],
        ),
    ]
}

fn gh_json(args: &[&str]) -> serde_json::Value {
    let output = std::process::Command::new("gh")
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "gh {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

/// Poll a background job until `pending` clears, allowing for the network.
fn settle_slowly(app: &mut App, pending: fn(&App) -> bool, poll: fn(&mut App) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    while pending(app) {
        poll(app);
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting on GitHub"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Dirt of every kind in `dir`: staged, unstaged, untracked, and a stash.
fn make_dirty(dir: &std::path::Path) {
    git_out(dir, &["config", "user.name", "AMF Acceptance"]);
    git_out(dir, &["config", "user.email", "amf@example.com"]);
    let readme = dir.join("README.md");
    let original = std::fs::read_to_string(&readme).unwrap();
    std::fs::write(&readme, format!("{original}\nstashed\n")).unwrap();
    git_out(dir, &["stash", "push", "--quiet", "-m", "keep me"]);
    std::fs::write(&readme, format!("{original}\nunstaged edit\n")).unwrap();
    let changelog = dir.join("CHANGELOG.md");
    let log = std::fs::read_to_string(&changelog).unwrap();
    std::fs::write(&changelog, format!("{log}\nstaged edit\n")).unwrap();
    git_out(dir, &["add", "CHANGELOG.md"]);
    std::fs::write(dir.join("local-only-untracked.txt"), "stray\n").unwrap();
}

/// Task 11: review a real PR while the project's checkout and a feature
/// worktree are dirty. Talks to GitHub — it pushes a commit to the PR's
/// branch and posts a Comment review — so it runs only when asked:
///
/// ```text
/// AMF_PR_REVIEW_ACCEPTANCE_PR=654 cargo test -j 2 --lib -- --ignored acceptance_
/// ```
///
/// Needs `gh` signed in with push access to the PR's branch.
#[test]
#[ignore = "talks to GitHub; set AMF_PR_REVIEW_ACCEPTANCE_PR and run with --ignored"]
fn acceptance_review_a_real_pr_while_a_local_feature_is_dirty() {
    let Ok(number) = std::env::var("AMF_PR_REVIEW_ACCEPTANCE_PR") else {
        eprintln!("AMF_PR_REVIEW_ACCEPTANCE_PR not set; skipping");
        return;
    };
    let number: u32 = number.parse().expect("a PR number");
    let repo_url = std::env::var("AMF_PR_REVIEW_ACCEPTANCE_REPO")
        .unwrap_or_else(|_| "https://github.com/eldridgerdev/agent-mainframe".to_string());
    let repo_slug = repo_url
        .trim_start_matches("https://github.com/")
        .to_string();

    // A project clone with a dirty checkout and a dirty feature worktree.
    let root = TempDir::new().unwrap();
    let clone = root.path().join("project");
    git_out(root.path(), &["clone", "--quiet", &repo_url, "project"]);
    let feature = root.path().join("feature");
    git_out(
        &clone,
        &[
            "worktree",
            "add",
            "--quiet",
            "-b",
            "my-feature",
            feature.to_str().unwrap(),
        ],
    );
    make_dirty(&clone);
    make_dirty(&feature);
    let clone_before = checkout_snapshot(&clone);
    let feature_before = checkout_snapshot(&feature);

    // What GitHub says the PR is.
    let pr = gh_json(&[
        "pr",
        "view",
        &number.to_string(),
        "-R",
        &repo_slug,
        "--json",
        "title,author,headRefName,headRefOid,isDraft,isCrossRepository,headRepositoryOwner,files",
    ]);
    let pr_files: std::collections::BTreeSet<String> = pr["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();

    // ── The Review tab lists it, correctly, without blocking. ──
    let db_file = NamedTempFile::new().unwrap();
    let mut app = App::new_for_test(
        store_with_repo(clone.clone(), crate::project::ProjectStatus::Active),
        Box::new(MockTmuxOps::new()), // any tmux call fails the test
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    app.selection = Selection::Project(0);
    let features_before = app.store.projects[0].features.len();
    app.open_pr_review();
    assert_eq!(
        review_list(&app).load,
        PrReviewListLoad::Loading,
        "never blocks"
    );
    settle_slowly(
        &mut app,
        |a| a.pr_review_work.review_list_pending(),
        App::poll_pr_review_list_bg,
    );
    let row = match &review_list(&app).load {
        PrReviewListLoad::Loaded(prs) => prs
            .iter()
            .position(|p| p.number == number)
            .expect("the PR is listed"),
        other => panic!("list failed: {other:?}"),
    };
    {
        let PrReviewListLoad::Loaded(prs) = &review_list(&app).load else {
            unreachable!()
        };
        let listed = &prs[row];
        assert_eq!(listed.title, pr["title"].as_str().unwrap());
        assert_eq!(listed.author, pr["author"]["login"].as_str().unwrap());
        assert_eq!(listed.is_draft, pr["isDraft"].as_bool().unwrap());
        let branch = pr["headRefName"].as_str().unwrap();
        let expected_label = if pr["isCrossRepository"].as_bool().unwrap() {
            format!(
                "{}:{branch}",
                pr["headRepositoryOwner"]["login"].as_str().unwrap()
            )
        } else {
            branch.to_string()
        };
        assert_eq!(listed.branch_label(), expected_label);
    }
    if let AppMode::PrReviewList(state) = &mut app.mode {
        state.selected = row;
    }

    // ── Enter shows exactly the PR's merge-base → head files. ──
    let open = |app: &mut App| {
        press(app, KeyCode::Enter);
        settle_slowly(
            app,
            |a| a.pr_review_work.review_open_pending(),
            App::poll_pr_review_open_bg,
        );
        if let AppMode::PrReviewList(list) = &app.mode {
            panic!("could not open: {:?}", list.open_error);
        }
        app.complete_diff_viewer_loading();
        assert_eq!(viewer(app).error, None);
    };
    open(&mut app);
    let shown: std::collections::BTreeSet<String> =
        viewer(&app).files.iter().map(|f| f.path.clone()).collect();
    assert_eq!(shown, pr_files, "exactly the PR's files");
    assert!(!shown.contains("local-only-untracked.txt"));

    // ── Comment, approve, pause, return, reopen: all restored. ──
    let (commented, comment_line) = viewer(&app)
        .files
        .iter()
        .find_map(|file| {
            file.hunks
                .iter()
                .flat_map(|hunk| &hunk.lines)
                .find(|line| {
                    line.kind == crate::diff::DiffLineKind::Added
                        && line.text.trim_start_matches('+').trim().len() > 4
                })
                .map(|line| {
                    (
                        file.path.clone(),
                        line.text.trim_start_matches('+').to_string(),
                    )
                })
        })
        .expect("a file with an added line");
    select_file(&mut app, &commented);
    comment_on_line_containing(&mut app, &comment_line, "AMF acceptance: inline note");
    for path in pr_files.iter() {
        select_file(&mut app, path);
        press(&mut app, KeyCode::Char('a'));
    }
    select_file(&mut app, &commented);
    let position = viewer(&app).selected_file;
    press(&mut app, KeyCode::Esc);
    assert!(
        app.message
            .as_deref()
            .is_some_and(|m| m.contains("draft saved"))
    );
    open(&mut app);
    assert_eq!(line_comment_texts(&app), ["AMF acceptance: inline note"]);
    assert_eq!(viewer(&app).decisions.len(), pr_files.len());
    assert_eq!(viewer(&app).selected_file, position, "position restored");

    // ── The user's checkouts are untouched; no agent, feature, or session. ──
    assert_eq!(checkout_snapshot(&clone), clone_before);
    assert_eq!(checkout_snapshot(&feature), feature_before);
    assert_eq!(app.store.projects[0].features.len(), features_before);
    assert!(
        app.store.projects[0]
            .features
            .iter()
            .all(|f| f.sessions.is_empty())
    );

    // ── A new commit on the PR flags only the file it changed. ──
    press(&mut app, KeyCode::Esc);
    let touched = pr_files
        .iter()
        .find(|path| **path != commented && !path.ends_with(".rs"))
        .or_else(|| pr_files.iter().find(|path| **path != commented))
        .expect("a second file in the PR")
        .clone();
    let pusher = root.path().join("pusher");
    git_out(root.path(), &["clone", "--quiet", &repo_url, "pusher"]);
    git_out(&pusher, &["config", "user.name", "AMF Acceptance"]);
    git_out(&pusher, &["config", "user.email", "amf@example.com"]);
    let branch = pr["headRefName"].as_str().unwrap();
    git_out(&pusher, &["checkout", "--quiet", branch]);
    let touched_path = pusher.join(&touched);
    let before = std::fs::read_to_string(&touched_path).unwrap_or_default();
    std::fs::write(
        &touched_path,
        format!(
            "{before}\nAMF acceptance push {}\n",
            chrono::Utc::now().to_rfc3339()
        ),
    )
    .unwrap();
    git_out(
        &pusher,
        &["commit", "--quiet", "-am", "test: AMF acceptance push"],
    );
    git_out(&pusher, &["push", "--quiet", "origin", branch]);
    let new_head = git_out(&pusher, &["rev-parse", "HEAD"]);

    press(&mut app, KeyCode::Char('r'));
    settle_slowly(
        &mut app,
        |a| a.pr_review_work.review_list_pending(),
        App::poll_pr_review_list_bg,
    );
    open(&mut app);
    let state = viewer(&app);
    let DiffScope::PullRequest(target) = &state.scope else {
        unreachable!()
    };
    assert_eq!(target.pr.head_oid, new_head, "reviewing the new head");
    assert_eq!(
        state.changed_since_last,
        std::collections::HashSet::from([touched.clone()]),
        "only the pushed file is flagged"
    );
    assert!(
        !state.decisions.contains_key(&touched),
        "its approval is cleared"
    );
    assert_eq!(
        state.decisions.len(),
        pr_files.len() - 1,
        "the rest keep theirs"
    );
    assert_eq!(line_comment_texts(&app), ["AMF acceptance: inline note"]);

    // ── Posting a Comment pins the inline comment to the reviewed head. ──
    press(&mut app, KeyCode::Char('q'));
    let marker = format!("[AMF acceptance test {}]", chrono::Utc::now().to_rfc3339());
    write_summary(&mut app, &marker);
    press(&mut app, KeyCode::Enter);
    settle_slowly(
        &mut app,
        |a| a.pr_review_work.review_post_pending(),
        App::poll_pr_review_post_bg,
    );
    assert!(
        matches!(app.mode, AppMode::PrReviewList(_)),
        "posted and back on the list: {:?}",
        app.message
    );
    let reviews = gh_json(&["api", &format!("repos/{repo_slug}/pulls/{number}/reviews")]);
    let review = reviews
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|r| r["body"].as_str().is_some_and(|b| b.contains(&marker)))
        .expect("the review is on GitHub");
    assert_eq!(review["state"], "COMMENTED");
    assert_eq!(review["commit_id"].as_str().unwrap(), new_head);
    let inline = gh_json(&[
        "api",
        &format!(
            "repos/{repo_slug}/pulls/{number}/reviews/{}/comments",
            review["id"]
        ),
    ]);
    let inline = inline.as_array().unwrap();
    assert_eq!(inline.len(), 1);
    assert_eq!(inline[0]["path"].as_str().unwrap(), commented);
    assert!(
        inline[0]["body"]
            .as_str()
            .unwrap()
            .contains("AMF acceptance: inline note")
    );

    // ── The review's refs are gone; the checkouts are still untouched. ──
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !git_out(&clone, &["for-each-ref", "refs/amf/review/"]).is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "review refs left behind"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(checkout_snapshot(&clone), clone_before);
    assert_eq!(checkout_snapshot(&feature), feature_before);
    assert!(
        app.store.projects[0]
            .features
            .iter()
            .all(|f| f.sessions.is_empty())
    );
}
