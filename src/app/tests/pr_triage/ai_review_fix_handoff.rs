use super::*;

fn ai_review_over_triage(app: &mut App, n: usize) {
    enter_pr_review(app, 2);
    app.open_ai_review_from_triage();
    if let AppMode::AiReview(state) = &mut app.mode {
        state.findings = (0..n)
            .map(|i| sample_ai_review_finding(&format!("finding {i}")))
            .collect();
    }
}

#[test]
fn ai_review_fix_hands_the_finding_to_triage_without_posting() {
    let mut app = ai_review_test_app();
    ai_review_over_triage(&mut app, 2);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.selected = 1;
    }

    app.ai_review_fix_selected();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR Triage after handing off a finding");
    };
    let comment = state.selected_comment().unwrap();
    assert!(comment.is_local_finding());
    assert!(comment.body.contains("finding 1"));
    let confirm = state.fix_confirm.as_ref().expect("fix dialog should open");
    assert!(confirm.batch.is_none());
    assert!(confirm.editor.text().contains("finding 1"));
    // Nothing to reply to on GitHub, so no reply-draft handoff.
    assert!(!confirm.editor.text().contains("amf reply-draft"));
    assert_eq!(confirm.reply_draft_requests[0].comment_id, comment.id);
    // The stashed pane was restored, not replaced by a fresh one.
    assert!(app.ai_review_return_to.is_none());
    assert_eq!(state.review.comments.len(), 3);
}

#[test]
fn ai_review_batch_fix_combines_marked_findings() {
    let mut app = ai_review_test_app();
    ai_review_over_triage(&mut app, 3);
    app.ai_review_toggle_mark();
    app.ai_review_select_next();
    app.ai_review_select_next();
    app.ai_review_toggle_mark();

    app.ai_review_fix_marked();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR Triage");
    };
    let confirm = state
        .fix_confirm
        .as_ref()
        .expect("batch dialog should open");
    assert_eq!(confirm.batch.as_ref().map(Vec::len), Some(2));
    assert!(confirm.editor.text().contains("finding 0"));
    assert!(confirm.editor.text().contains("finding 2"));
    assert!(!confirm.editor.text().contains("finding 1"));
    assert!(!confirm.editor.text().contains("amf reply-draft"));
}

#[test]
fn ai_review_fix_refuses_posted_and_skipped_findings() {
    let mut app = ai_review_test_app();
    ai_review_over_triage(&mut app, 2);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.findings[0].published = true;
        state.findings[1].skipped = true;
    }

    app.ai_review_fix_selected();
    assert!(matches!(app.mode, AppMode::AiReview(_)));
    assert!(app.message.as_deref().unwrap().contains("Already posted"));

    app.ai_review_select_next();
    app.ai_review_toggle_mark();
    assert!(app.message.as_deref().unwrap().contains("Skipped"));
    app.ai_review_fix_marked();
    assert!(matches!(app.mode, AppMode::AiReview(_)));
}

#[test]
fn a_local_finding_cannot_be_replied_to_on_github() {
    let mut app = ai_review_test_app();
    ai_review_over_triage(&mut app, 1);
    app.ai_review_fix_selected();
    app.pr_review_cancel_fix();

    app.pr_review_open_reply_pick();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR Triage");
    };
    assert!(state.reply_kind_pick.is_none());
    assert!(app.message.as_deref().unwrap().contains("not posted"));
}

// ── PR picker Review tab ────────────────────────────────────────────────────
