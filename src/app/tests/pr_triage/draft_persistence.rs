use super::*;

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
