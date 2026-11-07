use super::*;

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
