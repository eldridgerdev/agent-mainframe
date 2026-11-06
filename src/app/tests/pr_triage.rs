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
use crate::app::pr_review::runtime::{PrPostOutcome, PrPostRequest};

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

mod draft_persistence;

mod pr_updates;

mod review_posting;

mod acceptance;

mod review_pane;
