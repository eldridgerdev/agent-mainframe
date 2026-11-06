use super::*;

pub(super) fn sample_ai_review_finding(body: &str) -> crate::app::ai_review::AiReviewFinding {
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

pub(super) fn reply_editor_text(app: &App) -> String {
    match &app.mode {
        AppMode::PrReview(state) => state.reply.as_ref().unwrap().editor.text().to_string(),
        _ => unreachable!(),
    }
}

pub(super) fn pr_review_selected(app: &App) -> usize {
    match &app.mode {
        AppMode::PrReview(state) => state.selected,
        _ => panic!("not in PrReview mode"),
    }
}

/// An app whose single feature lives at `/tmp/test-workdir`, with a worktree
/// mock that resolves the repo root — enough for the fix-target picker and the
/// triage-feature setup overlay, neither of which touches git.
pub(super) fn reviewable(number: u32, title: &str) -> crate::github::ReviewablePr {
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
pub(super) const TEST_REPO_KEY: &str = "github.com/acme/widgets";

pub(super) fn two_open_prs(_: &std::path::Path, _: bool) -> ReviewListLoaded {
    ReviewListLoaded {
        prs: Ok(vec![reviewable(7, "first"), reviewable(9, "second")]),
        current_user: Some("me".to_string()),
        repo_key: Some(TEST_REPO_KEY.to_string()),
    }
}

pub(super) fn gh_not_signed_in(_: &std::path::Path, _: bool) -> ReviewListLoaded {
    ReviewListLoaded {
        prs: Err(anyhow::anyhow!(
            "`gh pr list` failed: To get started with GitHub CLI, please run:  gh auth login"
        )),
        current_user: None,
        repo_key: None,
    }
}

pub(super) fn press(app: &mut App, code: KeyCode) {
    crate::handlers::handle_key(app, KeyEvent::from(code), 40).unwrap();
}

/// Poll until the Review tab's worker result has been applied.
pub(super) fn settle_review_list(app: &mut App) {
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

pub(super) fn review_list(app: &App) -> &PrReviewListState {
    match &app.mode {
        AppMode::PrReviewList(state) => state,
        _ => panic!("not on the Review tab"),
    }
}

// ── Opening a PR for review from the Review tab ─────────────────────────────

pub(super) fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
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
pub(super) fn review_tab_with_db(repo: &std::path::Path, db_file: &NamedTempFile) -> App {
    let mut app = review_tab_on(repo);
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    app.pr_review_work
        .set_review_opener_for_test(open_teammate_pr);
    // Reload so the badges read the database that is now attached.
    press(&mut app, KeyCode::Char('r'));
    settle_review_list(&mut app);
    app
}

pub(super) fn open_selected_pr(app: &mut App) {
    press(app, KeyCode::Enter);
    settle_review_open(app);
    app.complete_diff_viewer_loading();
    assert!(viewer(app).is_pr_review());
}

/// Comment on the first commentable line of the current file.
pub(super) fn comment_first_line(app: &mut App, text: &str) {
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
pub(super) fn row_shows_updated_badge(app: &App, number: u32) -> bool {
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

pub(super) fn saved_draft(app: &App) -> Option<crate::db::pr_review_drafts::PrReviewDraft> {
    app.db
        .as_ref()
        .unwrap()
        .load_pr_review_draft(TEST_REPO_KEY, 9)
        .unwrap()
}

pub(super) fn line_comment_texts(app: &App) -> Vec<String> {
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
pub(super) fn repo_with_two_file_pr() -> TempDir {
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
pub(super) fn push_to_pr(dir: &std::path::Path, path: &str, content: &str) {
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

pub(super) fn select_file(app: &mut App, path: &str) {
    let AppMode::DiffViewer(state) = &mut app.mode else {
        panic!("not in the viewer");
    };
    state.selected_file = state.files.iter().position(|f| f.path == path).unwrap();
    state.on_file_changed();
}

/// Comment (with the real keys) on the current file's line containing `needle`.
pub(super) fn comment_on_line_containing(app: &mut App, needle: &str, text: &str) {
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

pub(super) fn comment<'a>(app: &'a App, path: &str, text: &str) -> &'a LineComment {
    viewer(app).line_comments[path]
        .iter()
        .find(|c| c.text == text)
        .unwrap_or_else(|| panic!("no comment {text:?} on {path}"))
}

/// What the stub posters were asked to post, by the test's repo path, so
/// parallel tests never read each other's requests.
pub(super) fn posted_requests()
-> &'static std::sync::Mutex<HashMap<std::path::PathBuf, Vec<PrPostRequest>>> {
    static POSTED: std::sync::OnceLock<
        std::sync::Mutex<HashMap<std::path::PathBuf, Vec<PrPostRequest>>>,
    > = std::sync::OnceLock::new();
    POSTED.get_or_init(Default::default)
}

pub(super) fn record(request: &PrPostRequest) {
    posted_requests()
        .lock()
        .unwrap()
        .entry(request.workdir.clone())
        .or_default()
        .push(request.clone());
}

pub(super) fn requests_for(dir: &std::path::Path) -> Vec<PrPostRequest> {
    posted_requests()
        .lock()
        .unwrap()
        .get(dir)
        .cloned()
        .unwrap_or_default()
}

pub(super) fn github_accepts(request: &PrPostRequest) -> PrPostOutcome {
    record(request);
    PrPostOutcome::Posted {
        file_comment_failures: vec![],
    }
}

pub(super) fn github_is_down(request: &PrPostRequest) -> PrPostOutcome {
    record(request);
    PrPostOutcome::Failed("`gh api` (create review) failed: HTTP 502".to_string())
}

pub(super) fn pr_has_moved(request: &PrPostRequest) -> PrPostOutcome {
    record(request);
    PrPostOutcome::HeadMoved {
        current_head: "feedface00000000000000000000000000000000".to_string(),
    }
}

pub(super) fn settle_review_post(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.pr_review_work.review_post_pending() {
        app.poll_pr_review_post_bg();
        assert!(std::time::Instant::now() < deadline, "post never answered");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

pub(super) fn submit_status(app: &App) -> PrSubmitStatus {
    viewer(app)
        .pr_submit
        .as_ref()
        .expect("the submit dialog is open")
        .status
        .clone()
}

/// Type into the open general-feedback (summary) editor and save it.
pub(super) fn write_summary(app: &mut App, text: &str) {
    press(app, KeyCode::Char('e'));
    for ch in text.chars() {
        press(app, KeyCode::Char(ch));
    }
    press(app, KeyCode::Tab);
}

/// The two-file PR, opened for review, with a single-line comment on `edit.rs`.
pub(super) fn reviewed_two_file_pr(
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
