use super::*;

#[test]
fn ai_review_finding_fix_costs_correlate_to_resolved_batched_comments() {
    let db_file = tempfile::NamedTempFile::new().unwrap();
    let mut app = pr_review_test_app();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());

    // A cached review with two code comments (ids 1 and 2, on src/file1.rs:1
    // and src/file2.rs:2) to correlate findings against.
    let review = pr_review_with_comments(2);
    app.db
        .as_ref()
        .unwrap()
        .save_pr_review_cache(&review)
        .unwrap();

    // Comment 1 was fixed in a combined batch and resolved; comment 2 wasn't
    // touched.
    let db = app.db.as_ref().unwrap();
    db.save_pr_comment_triage(
        7,
        "sha",
        1,
        crate::app::pr_review::TriageState::Fixing,
        None,
        Some("batch-z"),
    )
    .unwrap();
    db.save_pr_comment_triage(
        7,
        "sha",
        1,
        crate::app::pr_review::TriageState::Done,
        None,
        None,
    )
    .unwrap();
    // Comment 2 is in the same batch but was never resolved — the partial-batch
    // rule means it must contribute no cost line.
    db.save_pr_comment_triage(
        7,
        "sha",
        2,
        crate::app::pr_review::TriageState::Fixing,
        None,
        Some("batch-z"),
    )
    .unwrap();
    db.set_pr_comment_batch_fix_cost(7, "batch-z", "$0.09")
        .unwrap();

    let pr = review.pr.clone();
    let mut state = sample_ai_review_state(std::path::PathBuf::from("/tmp/wd"), pr);
    // Finding 0 matches comment 1 (resolved, batched); finding 1 matches
    // comment 2 (batched but unresolved); finding 2 matches nothing.
    let mut f0 = sample_ai_review_finding("race");
    f0.path = Some("src/file1.rs".to_string());
    f0.line = Some(1);
    let mut f1 = sample_ai_review_finding("style");
    f1.path = Some("src/file2.rs".to_string());
    f1.line = Some(2);
    let mut f2 = sample_ai_review_finding("orphan");
    f2.path = Some("src/other.rs".to_string());
    f2.line = Some(9);
    state.findings = vec![f0, f1, f2];
    app.mode = AppMode::AiReview(state);

    let costs = app.ai_review_finding_fix_costs();
    assert_eq!(
        costs,
        vec![
            Some("Fix cost (est.): $0.09 · combined (2)".to_string()),
            None,
            None,
        ]
    );

    // The result is memoized: a second call with the pane's findings unchanged
    // answers from `ai_review_fix_cost_cache` without re-hitting the DB. Prove
    // it by dropping the DB and confirming the same answer still comes back.
    assert!(app.ai_review_fix_cost_cache.is_some());
    app.db = None;
    assert_eq!(app.ai_review_finding_fix_costs(), costs);

    // Clearing the memo (what pane (re)open and a landed `A` run do) forces a
    // recompute — now with no DB, so every finding falls back to `None`.
    app.ai_review_fix_cost_cache = None;
    assert_eq!(app.ai_review_finding_fix_costs(), vec![None, None, None]);
}

#[test]
fn ai_review_model_picker_opens_for_pi() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Pi);
    }

    app.start_ai_pr_review();

    match &app.mode {
        AppMode::AiReview(state) => {
            // Pi's headless CLI takes `--model`, so it must get the picker
            // rather than being force-skipped to the harness default.
            assert!(!state.model_picked);
            let pick = state
                .model_pick
                .as_ref()
                .expect("Pi should open the model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
        }
        _ => panic!("expected AI Review pane"),
    }
    assert!(
        !app.ai_review_run.is_pending(),
        "review should not have started"
    );
}

#[test]
fn ai_review_model_picker_backs_through_custom_editor_and_rebuilds_for_new_harness() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .times(1)
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.config.review_model = Some("sonnet".to_string());
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Claude);
        state.model = Some("sonnet".to_string());
        state.model_pick = Some(AiModelPickState {
            rows: vec![
                ModelPickRow::Default,
                ModelPickRow::Preset("sonnet".to_string()),
                ModelPickRow::Custom,
            ],
            selected: 2,
            custom_input: "claude-custom".to_string(),
            editing_custom: true,
            reasoning_levels: &[],
            reasoning: None,
        });
    }

    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::AiReview(state) => {
            let pick = state.model_pick.as_ref().expect("model list should remain");
            assert!(!pick.editing_custom);
            assert_eq!(pick.custom_input, "claude-custom");
            assert!(state.harness_pick.is_none());
        }
        _ => panic!("expected AI Review pane"),
    }

    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::AiReview(state) => {
            let pick = state
                .harness_pick
                .as_ref()
                .expect("model list should return to harness picker");
            assert_eq!(pick.agents[pick.selected], AgentKind::Claude);
            assert!(state.harness.is_none());
            assert!(state.model.is_none());
            assert!(!state.model_picked);
            assert!(state.model_pick.is_none());
        }
        _ => panic!("expected AI Review pane"),
    }

    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();

    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            assert!(state.harness_pick.is_none());
            assert!(state.model.is_none());
            assert!(!state.model_picked);
            let pick = state
                .model_pick
                .as_ref()
                .expect("new harness should open a rebuilt model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
            assert_eq!(pick.selected, 0);
            assert!(pick.custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }
}

/// Regression for the gap `ai_review_model_picker_backs_through_custom_editor_and_rebuilds_for_new_harness`
/// doesn't cover: backing out *again* after switching harness once, and
/// reconfirming the *same* (already-switched-to) harness a second time.
/// `harness_changed` used to compare only against the immediately preceding
/// screen, so this second confirm looked unchanged and fell through to
/// `start_ai_pr_review`'s `AppConfig::review_model` fallback — reseeding the
/// Claude-only "sonnet" preset as a custom Opencode model, exactly what the
/// first switch had correctly cleared.
#[test]
fn ai_review_harness_pick_reconfirm_after_backing_out_does_not_reseed_stale_model() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .times(2)
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.config.review_model = Some("sonnet".to_string());
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Claude);
        state.model_pick = Some(AiModelPickState {
            rows: vec![
                ModelPickRow::Default,
                ModelPickRow::Preset("sonnet".to_string()),
                ModelPickRow::Custom,
            ],
            selected: 0,
            custom_input: String::new(),
            editing_custom: false,
            reasoning_levels: &[],
            reasoning: None,
        });
    }

    // Back out of the model picker (Claude -> harness picker) and switch to
    // Opencode: this is the already-covered case, so the model resets to
    // fresh defaults instead of "sonnet".
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            let pick = state.model_pick.as_ref().expect("rebuilt model picker");
            assert!(pick.custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Back out a second time without picking a model, and reconfirm the same
    // (already-switched-to) Opencode harness.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    match &app.mode {
        AppMode::AiReview(state) => {
            let pick = state
                .harness_pick
                .as_ref()
                .expect("should return to harness picker");
            assert_eq!(pick.agents[pick.selected], AgentKind::Opencode);
        }
        _ => panic!("expected AI Review pane"),
    }
    app.accept_selected_ai_review_harness_for_test();

    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            let pick = state
                .model_pick
                .as_ref()
                .expect("reconfirming should still open a fresh model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
            assert_eq!(pick.selected, 0);
            assert!(
                pick.custom_input.is_empty(),
                "must not reseed the Claude-only \"sonnet\" default as an Opencode custom model"
            );
        }
        _ => panic!("expected AI Review pane"),
    }
}

/// Broader regression covering the reviewer's general "repeated navigation"
/// concern rather than just a single back-and-forth: hops through three
/// harnesses (Claude -> Opencode -> Codex -> Opencode -> Claude), backing out
/// to the harness picker between every hop. Every switch away from the
/// original Claude harness must land on fresh `Default`/empty model defaults
/// (never a stale non-Claude value), and switching *back* to the original
/// Claude harness must still restore the meaningful `AppConfig::review_model`
/// default ("sonnet") rather than being needlessly cleared just because a
/// picker round trip happened.
#[test]
fn ai_review_harness_pick_survives_multi_hop_navigation() {
    let store = store_with_feature(ProjectStatus::Idle);
    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .times(4)
        .returning(|_| Ok(std::path::PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    app.config.review_model = Some("sonnet".to_string());
    enter_ai_review_for_feature(&mut app);
    if let AppMode::AiReview(state) = &mut app.mode {
        state.harness = Some(AgentKind::Claude);
        state.model_pick = Some(AiModelPickState {
            rows: vec![
                ModelPickRow::Default,
                ModelPickRow::Preset("sonnet".to_string()),
                ModelPickRow::Custom,
            ],
            selected: 1,
            custom_input: String::new(),
            editing_custom: false,
            reasoning_levels: &[],
            reasoning: None,
        });
    }

    // Claude -> Opencode: first switch away from the original harness.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            assert!(state.model_pick.as_ref().unwrap().custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Opencode -> Codex: still away from the original, still fresh defaults.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Down)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Codex));
            let pick = state.model_pick.as_ref().expect("rebuilt model picker");
            assert_eq!(pick.rows, vec![ModelPickRow::Default, ModelPickRow::Custom]);
            assert!(pick.custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Codex -> Opencode: back to a previously-visited harness, still not the
    // original — must not resurrect anything stale.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Up)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Opencode));
            assert!(state.model_pick.as_ref().unwrap().custom_input.is_empty());
        }
        _ => panic!("expected AI Review pane"),
    }

    // Opencode -> Claude: switching back to the *original* harness should
    // restore the configured "sonnet" default, not force an empty Custom.
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Esc)).unwrap();
    crate::handlers::handle_ai_review_key(&mut app, ke(KeyCode::Up)).unwrap();
    app.accept_selected_ai_review_harness_for_test();
    match &app.mode {
        AppMode::AiReview(state) => {
            assert_eq!(state.harness, Some(AgentKind::Claude));
            let pick = state.model_pick.as_ref().expect("rebuilt model picker");
            assert_eq!(
                pick.rows.get(pick.selected),
                Some(&ModelPickRow::Preset("sonnet".to_string())),
                "returning to the original harness should still honor the configured default"
            );
        }
        _ => panic!("expected AI Review pane"),
    }
}
