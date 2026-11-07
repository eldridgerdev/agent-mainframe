use super::*;

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
