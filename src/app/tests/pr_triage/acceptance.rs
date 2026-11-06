use super::*;

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
