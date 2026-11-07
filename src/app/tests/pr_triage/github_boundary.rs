use super::*;

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
