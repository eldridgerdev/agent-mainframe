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

fn reply_editor_text(app: &App) -> String {
    match &app.mode {
        AppMode::PrReview(state) => state.reply.as_ref().unwrap().editor.text().to_string(),
        _ => unreachable!(),
    }
}

fn pr_review_selected(app: &App) -> usize {
    match &app.mode {
        AppMode::PrReview(state) => state.selected,
        _ => panic!("not in PrReview mode"),
    }
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

#[test]
fn pr_review_open_fix_confirm_without_comments_shows_message() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 0);

    app.pr_review_open_fix_confirm();
    assert_eq!(app.pr_review_fix_editing(), None);
    assert!(app.message.is_some());
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

use crate::app::pr_review::runtime::ReviewListLoaded;

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

/// Whether the Review tab's row for PR `number` carries the `↻ updated`
/// badge: a saved draft whose head is not the row's current one.
fn row_shows_updated_badge(app: &App, number: u32) -> bool {
    let list = review_list(app);
    let PrReviewListLoad::Loaded(prs) = &list.load else {
        panic!("the list is not loaded");
    };
    let row = prs
        .iter()
        .find(|pr| pr.number == number)
        .expect("PR listed");
    list.drafts
        .get(&number)
        .is_some_and(|draft| draft.head_oid != row.head_oid)
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
    app.pr_review_work
        .set_review_list_loader_for_test(two_open_prs_at_real_heads);
    open_selected_pr(&mut app);
    comment_first_line(&mut app, "keep me");
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Esc);
    // Before the author pushes, the row and the draft agree: no badge.
    press(&mut app, KeyCode::Char('r'));
    settle_review_list(&mut app);
    assert!(!row_shows_updated_badge(&app, 9));

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
    press(&mut app, KeyCode::Char('r'));
    settle_review_list(&mut app);
    assert!(row_shows_updated_badge(&app, 9));

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

#[test]
fn tui_reply_and_resolve_write_through_the_shared_github_boundary() {
    use crate::app::pr_review::github_access::fake::FakeGithub;
    let fake = FakeGithub::default();
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_github_for_test(std::sync::Arc::new(fake.clone()));
    enter_pr_review(&mut app, 2);

    // A failed write keeps the TUI's existing behaviour: back to the
    // dashboard with the error, nothing recorded as posted.
    fake.state().fail_writes = true;
    app.pr_review_open_reply_not_needed();
    if let AppMode::PrReview(state) = &mut app.mode {
        state.reply.as_mut().unwrap().editor =
            crate::editor::TextEditor::new("Already handled upstream.".into());
    }
    app.pr_review_post_reply().unwrap();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(fake.state().writes.is_empty());

    fake.state().fail_writes = false;
    enter_pr_review(&mut app, 2);
    app.pr_review_open_reply_not_needed();
    if let AppMode::PrReview(state) = &mut app.mode {
        state.reply.as_mut().unwrap().editor =
            crate::editor::TextEditor::new("Already handled upstream.".into());
        state.review.comments[1].thread_id = Some("THREAD".into());
    }
    app.pr_review_post_reply().unwrap();
    assert_eq!(
        fake.state().writes,
        ["reply 1: Already handled upstream.\n\n— posted via AMF"]
    );
    app.pr_review_select_next();
    app.pr_review_toggle_resolve();
    assert_eq!(fake.state().writes.len(), 2);
    assert_eq!(fake.state().writes[1], "resolve THREAD: true");
    let AppMode::PrReview(state) = &app.mode else {
        panic!("triage stays open after a successful write");
    };
    assert!(state.review.comments[1].is_resolved);
}

#[test]
fn tui_comment_fetch_failure_still_returns_to_the_dashboard() {
    use crate::app::pr_review::github_access::fake::FakeGithub;
    let fake = FakeGithub::default();
    fake.state().fail_fetch = Some("HTTP 502".into());
    let mut app = pr_review_test_app();
    app.pr_review_work
        .set_github_for_test(std::sync::Arc::new(fake.clone()));
    let pr = fake.pr_ref(9);
    app.start_pr_review_fetch(std::path::PathBuf::from("/tmp/wd"), pr);
    assert!(app.pr_review_loading());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !app.poll_pr_review_bg() {
        assert!(std::time::Instant::now() < until, "fetch never finished");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(matches!(app.mode, AppMode::Normal));
    assert_eq!(fake.state().fetches, 1);
}

mod navigation;

mod investigations;

mod fix_handoff;

mod review_memory;

mod session_activity;

mod fix_batch;

mod fix_editor;

mod review_sort;

mod replies;

mod memory_workflows;

mod ai_review_picker;

mod ai_review_workflow;

mod companion_feature;

mod ai_review_fix_handoff;

mod review_list;

mod open_pr;
use open_pr::{
    open_teammate_pr, repo_with_teammate_pr, review_tab_on, settle_review_open,
    two_open_prs_at_real_heads, viewer,
};
