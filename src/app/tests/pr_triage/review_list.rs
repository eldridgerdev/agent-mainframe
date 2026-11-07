use super::*;

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
