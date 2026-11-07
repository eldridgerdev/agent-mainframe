use super::*;

/// A repo whose `teammate-pr` branch stands in for PR #9, checked out on an
/// unrelated feature branch with a dirty tracked file and an untracked one.
pub(super) fn repo_with_teammate_pr() -> TempDir {
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
pub(super) fn open_teammate_pr(
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

/// [`two_open_prs`], but PR #9's row reports `teammate-pr`'s real head, as
/// `gh` would once the author pushes, so the list can tell a moved PR apart.
pub(super) fn two_open_prs_at_real_heads(
    workdir: &std::path::Path,
    closed: bool,
) -> ReviewListLoaded {
    let mut loaded = two_open_prs(workdir, closed);
    if let Ok(prs) = &mut loaded.prs {
        for pr in prs.iter_mut().filter(|pr| pr.number == 9) {
            pr.head_oid = git_out(workdir, &["rev-parse", "teammate-pr"]);
        }
    }
    loaded
}

pub(super) fn refuse_to_open(
    _: &std::path::Path,
    _: &crate::github::ReviewablePr,
) -> anyhow::Result<PrDiffTarget> {
    Err(anyhow::anyhow!(
        "`git fetch` for PR #9 failed: network down"
    ))
}

pub(super) fn settle_review_open(app: &mut App) {
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
pub(super) fn review_tab_on(repo: &std::path::Path) -> App {
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

pub(super) fn viewer(app: &App) -> &DiffViewerState {
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
