use super::*;

fn enter_pr_review_with_authors(app: &mut App, entries: &[(u64, &str, &str, bool)]) {
    let comments: Vec<crate::github::ReviewComment> = entries
        .iter()
        .map(|&(id, path, author, is_bot)| crate::github::ReviewComment {
            id,
            path: Some(path.to_string()),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("comment {id}"),
            user: crate::github::GhUser {
                login: author.to_string(),
                kind: if is_bot {
                    "Bot".to_string()
                } else {
                    "User".to_string()
                },
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
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
    let review = crate::app::pr_review::normalize(pr, comments, vec![], vec![], vec![]);
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

/// Enter the review pane with a mix of inline (code-anchored) comments and
/// top-level conversation comments, in fetch order: every id in `inline_ids`
/// first, then every id in `conversation_ids` — matching how `normalize`
/// orders review comments ahead of issue comments. For exercising
/// `PrSortMode::Conversations`.
fn enter_pr_review_with_conversation(app: &mut App, inline_ids: &[u64], conversation_ids: &[u64]) {
    let review_comments: Vec<crate::github::ReviewComment> = inline_ids
        .iter()
        .map(|&id| crate::github::ReviewComment {
            id,
            path: Some(format!("f{id}.rs")),
            line: Some(id as u32),
            original_line: Some(id as u32),
            side: Some("RIGHT".into()),
            subject_type: None,
            diff_hunk: Some("@@".to_string()),
            body: format!("inline {id}"),
            user: crate::github::GhUser {
                login: "alice".into(),
                kind: "User".into(),
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        })
        .collect();
    let issue_comments: Vec<crate::github::IssueComment> = conversation_ids
        .iter()
        .map(|&id| crate::github::IssueComment {
            id,
            body: format!("conversation {id}"),
            user: crate::github::GhUser {
                login: "bob".into(),
                kind: "User".into(),
            },
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
    let review =
        crate::app::pr_review::normalize(pr, review_comments, vec![], issue_comments, vec![]);
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

fn pr_review_ids_in_visible_order(app: &App) -> Vec<u64> {
    match &app.mode {
        AppMode::PrReview(state) => state
            .visible_indices()
            .iter()
            .map(|&i| state.review.comments[i].id)
            .collect(),
        _ => panic!("not in PrReview mode"),
    }
}

fn install_amf_outbound_comment_mix(app: &mut App) {
    use crate::app::pr_review::{
        AI_REVIEW_ATTRIBUTION_FOOTER, AMF_ATTRIBUTION_FOOTER, CommentKind,
    };

    enter_pr_review_with_conversation(app, &[], &[]);
    if let AppMode::PrReview(state) = &mut app.mode {
        let mut root = pr_comment_of_kind(1, CommentKind::Inline);
        root.author = "reviewer".into();

        let mut amf_reply = pr_comment_of_kind(2, CommentKind::Inline);
        amf_reply.author = "author".into();
        amf_reply.in_reply_to = Some(1);
        amf_reply.body = format!("Done in `abc123`.\n\n{AMF_ATTRIBUTION_FOOTER}");

        let mut ai_finding = pr_comment_of_kind(3, CommentKind::Inline);
        ai_finding.author = "author".into();
        ai_finding.body = format!("AI finding.\n\n{AI_REVIEW_ATTRIBUTION_FOOTER}");

        let mut human_reply = pr_comment_of_kind(4, CommentKind::Inline);
        human_reply.author = "second-reviewer".into();
        human_reply.in_reply_to = Some(1);
        human_reply.body = "This still needs a regression test.".into();

        let mut orphaned_amf_reply = pr_comment_of_kind(5, CommentKind::Inline);
        orphaned_amf_reply.author = "author".into();
        orphaned_amf_reply.in_reply_to = Some(999);
        orphaned_amf_reply.body = format!("Done elsewhere.\n\n{AMF_ATTRIBUTION_FOOTER}");

        state.review.comments = vec![root, amf_reply, ai_finding, human_reply, orphaned_amf_reply];
    }
}

#[test]
fn pr_review_collates_amf_followup_but_keeps_standalone_findings_actionable() {
    let mut app = pr_review_test_app();
    install_amf_outbound_comment_mix(&mut app);

    // The attributed inline reply is represented under comment 1's Replies
    // section, not as a duplicate row. A top-level AI finding stays fully
    // actionable; an orphaned follow-up stays visible for context; and the
    // unrelated human reply remains actionable.
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![1, 3, 4, 5]);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("not in PrReview mode");
    };
    assert_eq!(state.review.open_count(), 3);
    assert!(state.review.comments[3].is_actionable());
    assert!(state.review.comments[2].is_actionable());
    assert!(!state.review.comments[4].is_actionable());
}

#[test]
fn pr_review_fixes_standalone_amf_findings_but_not_followup_replies() {
    let mut app = pr_review_test_app();
    install_amf_outbound_comment_mix(&mut app);

    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = 2; // top-level AI Review finding
    }
    app.pr_review_open_fix_confirm();
    match &app.mode {
        AppMode::PrReview(state) => {
            let confirm = state
                .fix_confirm
                .as_ref()
                .expect("standalone AI Review finding is fixable");
            assert!(confirm.editor.text().contains("AI finding."));
        }
        _ => panic!("not in PrReview mode"),
    }

    // Even a stale mark set assembled before refresh filters the orphaned
    // AMF follow-up out of the combined prompt, but retains the finding.
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_confirm = None;
        state.marked.extend([3, 4, 5]);
    }
    app.pr_review_open_batch_confirm();
    let AppMode::PrReview(state) = &app.mode else {
        panic!("not in PrReview mode");
    };
    let confirm = state
        .fix_confirm
        .as_ref()
        .expect("standalone finding and human reply are batchable");
    assert_eq!(confirm.batch.as_deref(), Some(&[3, 4][..]));
    assert!(
        confirm
            .editor
            .text()
            .contains("This still needs a regression test.")
    );
    assert!(confirm.editor.text().contains("AI finding."));
    assert!(!confirm.editor.text().contains("Done elsewhere."));
}

#[test]
fn pr_review_cycle_sort_wraps_through_all_modes() {
    let mut app = pr_review_test_app();
    enter_pr_review(&mut app, 2);

    use crate::app::pr_review::PrSortMode;
    let sort_mode = |app: &App| match &app.mode {
        AppMode::PrReview(state) => state.sort_mode,
        _ => panic!("not in PrReview mode"),
    };

    assert_eq!(sort_mode(&app), PrSortMode::FetchOrder);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::ByFile);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::ByAuthor);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::HumansFirst);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::Conversations);
    app.pr_review_cycle_sort();
    assert_eq!(sort_mode(&app), PrSortMode::FetchOrder);
}

#[test]
fn pr_review_sort_by_file_groups_and_orders_paths() {
    let mut app = pr_review_test_app();
    // Fetch order: 1 (z.rs), 2 (a.rs), 3 (m.rs).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "z.rs", "alice", false),
            (2, "a.rs", "alice", false),
            (3, "m.rs", "alice", false),
        ],
    );
    app.pr_review_cycle_sort(); // FetchOrder -> ByFile
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 3, 1]);
}

#[test]
fn pr_review_sort_by_author_is_stable_within_ties() {
    let mut app = pr_review_test_app();
    // Fetch order: 1 (bob), 2 (alice), 3 (alice).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "a.rs", "bob", false),
            (2, "b.rs", "alice", false),
            (3, "c.rs", "alice", false),
        ],
    );
    app.pr_review_cycle_sort(); // FetchOrder -> ByFile
    app.pr_review_cycle_sort(); // ByFile -> ByAuthor
    // alice's two comments keep their fetch-order relative to each other.
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 3, 1]);
}

#[test]
fn pr_review_sort_humans_first_keeps_bots_last() {
    let mut app = pr_review_test_app();
    // Fetch order: 1 (bot), 2 (human), 3 (bot), 4 (human).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "a.rs", "coderabbit", true),
            (2, "b.rs", "alice", false),
            (3, "c.rs", "copilot", true),
            (4, "d.rs", "bob", false),
        ],
    );
    for _ in 0..3 {
        app.pr_review_cycle_sort(); // FetchOrder -> ByFile -> ByAuthor -> HumansFirst
    }
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 4, 1, 3]);
}

#[test]
fn pr_review_sort_conversations_groups_them_after_code_anchored_comments() {
    let mut app = pr_review_test_app();
    // Fetch order: inline 1, 2 then conversation 3, 4 — already matches the
    // desired grouping, so this proves the mode is a no-op here, not just
    // coincidentally correct because fetch order already separates them
    // (the interleaved case below is the real test).
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[3, 4]);
    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> ByFile -> ByAuthor -> HumansFirst -> Conversations
    }
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![1, 2, 3, 4]);
}

fn pr_comment_of_kind(
    id: u64,
    kind: crate::app::pr_review::CommentKind,
) -> crate::app::pr_review::PrComment {
    crate::app::pr_review::PrComment {
        id,
        kind,
        author: "someone".into(),
        is_bot: false,
        path: None,
        line: None,
        side: None,
        outdated: false,
        file_level: false,
        diff_hunk: None,
        body: format!("comment {id}"),
        snippet: format!("comment {id}"),
        in_reply_to: None,
        thread_id: None,
        is_resolved: false,
        triage: crate::app::pr_review::TriageState::Untriaged,
        local_note: None,
        batch_id: None,
        github_id: None,
        github_review_id: None,
    }
}

#[test]
fn pr_review_sort_conversations_reorders_interleaved_comments() {
    use crate::app::pr_review::CommentKind;

    let mut app = pr_review_test_app();
    enter_pr_review_with_conversation(&mut app, &[], &[]);
    if let AppMode::PrReview(state) = &mut app.mode {
        // Fetch order: 1 (inline), 3 (conversation), 2 (inline), 4 (conversation).
        state.review.comments = vec![
            pr_comment_of_kind(1, CommentKind::Inline),
            pr_comment_of_kind(3, CommentKind::Conversation),
            pr_comment_of_kind(2, CommentKind::Inline),
            pr_comment_of_kind(4, CommentKind::Conversation),
        ];
    }

    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> ByFile -> ByAuthor -> HumansFirst -> Conversations
    }
    // Every conversation comment moves after every inline one; relative order
    // within each group is preserved (stable sort).
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![1, 2, 3, 4]);
}

#[test]
fn pr_review_conversation_section_start_marks_where_the_group_begins() {
    let mut app = pr_review_test_app();
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[3, 4]);
    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> Conversations
    }
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PrReview");
    };
    assert_eq!(state.conversation_section_start(), Some(2));
}

#[test]
fn pr_review_conversation_section_start_is_none_outside_conversations_mode() {
    let mut app = pr_review_test_app();
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[3, 4]);
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PrReview");
    };
    assert_eq!(state.conversation_section_start(), None);
}

#[test]
fn pr_review_conversation_section_start_is_none_without_both_groups() {
    let mut app = pr_review_test_app();
    // All inline, no conversation comments — nothing to separate.
    enter_pr_review_with_conversation(&mut app, &[1, 2], &[]);
    for _ in 0..4 {
        app.pr_review_cycle_sort(); // -> Conversations
    }
    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PrReview");
    };
    assert_eq!(state.conversation_section_start(), None);
}

#[test]
fn pr_review_toggle_resolved_snaps_using_current_sort_order() {
    let mut app = pr_review_test_app();
    // Fetch order 1,2,3 by id; sorted by file: 2 (a.rs), 3 (m.rs), 1 (z.rs).
    // Resolve comment 3 (the middle one in file-sort order).
    enter_pr_review_with_authors(
        &mut app,
        &[
            (1, "z.rs", "alice", false),
            (2, "a.rs", "alice", false),
            (3, "m.rs", "alice", false),
        ],
    );
    if let AppMode::PrReview(state) = &mut app.mode {
        // Mark comment 3 resolved directly on the model.
        state
            .review
            .comments
            .iter_mut()
            .find(|c| c.id == 3)
            .unwrap()
            .is_resolved = true;
    }
    app.pr_review_cycle_sort(); // FetchOrder -> ByFile: order is [2, 3, 1]
    if let AppMode::PrReview(state) = &mut app.mode {
        state.selected = state
            .review
            .comments
            .iter()
            .position(|c| c.id == 3)
            .unwrap();
    }

    app.pr_review_toggle_resolved();
    // Comment 3 is hidden; the visible file-sort order is now [2, 1].
    assert_eq!(pr_review_ids_in_visible_order(&app), vec![2, 1]);
    match &app.mode {
        AppMode::PrReview(state) => {
            let selected_id = state.review.comments[state.selected].id;
            assert_eq!(selected_id, 1);
        }
        _ => panic!("not in PrReview mode"),
    }
}
