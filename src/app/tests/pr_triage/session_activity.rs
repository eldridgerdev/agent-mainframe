use super::*;

#[test]
fn pr_review_fix_session_usage_reads_the_target_sessions_tokens() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    session.token_usage = Some(SessionTokenUsage {
        source: TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "s1".to_string(),
        },
        input_tokens: 1000,
        output_tokens: 500,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1500,
    });

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    let usage = app.pr_review_fix_session_usage();
    assert_eq!(usage.map(|u| u.total_tokens), Some(1500));
}

#[test]
fn pr_review_fix_session_usage_none_before_session_exists() {
    // Before the first `f`, the dedicated review session doesn't exist yet —
    // the header shouldn't show a stale or fabricated number.
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert!(app.pr_review_fix_session_usage().is_none());
}

#[test]
fn pr_review_dedicated_session_status_is_scoped_to_its_session() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let other_session_id = store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Working session".to_string())
        .id
        .clone();
    let dedicated_session_id = store.projects[0].features[0]
        .add_session_named(
            SessionKind::Claude,
            crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
        )
        .id
        .clone();
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), Some(false));

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": tmux_session,
        "amf_feature_session_id": other_session_id,
    }));
    assert_eq!(
        app.pr_review_dedicated_session_working(),
        Some(false),
        "another agent window in the feature must not light the badge"
    );

    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));

    // Either activity signal independently keeps the exact session working.
    app.handle_ipc_message_value(serde_json::json!({
        "type": "tool-start",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-stop",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));

    app.handle_ipc_message_value(serde_json::json!({
        "type": "tool-stop",
        "session_id": tmux_session,
        "amf_feature_session_id": dedicated_session_id,
    }));
    assert_eq!(app.pr_review_dedicated_session_working(), Some(false));
}

#[test]
fn pr_review_dedicated_opencode_session_uses_sidebar_activity() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Opencode,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    session.set_token_usage_source_exact(TokenUsageSource {
        provider: TokenUsageProvider::Opencode,
        id: "opencode-triage".to_string(),
    });
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.opencode_sidebar_cache.insert(
        tmux_session,
        crate::app::opencode_storage::OpencodeSidebarData {
            session_id: "opencode-triage".to_string(),
            status: Some("busy".to_string()),
            ..Default::default()
        },
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));
}

#[test]
fn pr_review_schedules_sidebar_load_for_dedicated_opencode_session() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Opencode,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    session.set_token_usage_source_exact(TokenUsageSource {
        provider: TokenUsageProvider::Opencode,
        id: "opencode-triage".to_string(),
    });
    let si = store.projects[0].features[0].sessions.len() - 1;
    let expected_signature = SidebarLoadRequest::from_opencode_session(
        &store.projects[0].features[0],
        &store.projects[0].features[0].sessions[si],
    )
    .signature();
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    app.schedule_sidebar_load_for_feature(0, 0);
    for _ in 0..20 {
        app.poll_sidebar_load_results();
        if !app
            .sidebar_runtime
            .pending_sidebar_loads
            .contains(&tmux_session)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }

    assert_eq!(
        app.sidebar_runtime
            .sidebar_load_signatures
            .get(&tmux_session),
        Some(&expected_signature)
    );
}

#[test]
fn pr_review_dedicated_pi_session_uses_thinking_marker() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(
        SessionKind::Pi,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    let tmux_session = format!("amf-pi-triage-test-{}", uuid::Uuid::new_v4());
    store.projects[0].features[0].tmux_session = tmux_session.clone();
    let marker = PathBuf::from("/tmp/amf-thinking").join(&tmux_session);
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, "").unwrap();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), Some(true));

    std::fs::remove_file(marker).unwrap();
}

#[test]
fn pr_review_dedicated_session_status_is_absent_before_creation() {
    let store = store_with_feature(ProjectStatus::Active);
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);

    assert_eq!(app.pr_review_dedicated_session_working(), None);
}

#[test]
fn pr_review_dedicated_session_status_is_absent_for_existing_live_target() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Working session".to_string());
    let session_id = session.id.clone();
    let tmux_session = store.projects[0].features[0].tmux_session.clone();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 1);
    if let AppMode::PrReview(state) = &mut app.mode {
        state.fix_target = crate::app::pr_review::FixTarget::ExistingLive;
        state.fix_target_picked = true;
    }
    app.handle_ipc_message_value(serde_json::json!({
        "type": "thinking-start",
        "session_id": tmux_session,
        "amf_feature_session_id": session_id,
    }));

    assert_eq!(
        app.pr_review_dedicated_session_working(),
        None,
        "existing-live activity must not be rendered as dedicated activity"
    );
}

#[test]
fn pr_review_triage_session_usage_reports_only_growth_since_baseline() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    let source = TokenUsageSource {
        provider: TokenUsageProvider::Claude,
        id: "triage-session".to_string(),
    };
    session.token_usage = Some(SessionTokenUsage {
        source: source.clone(),
        input_tokens: 1400,
        output_tokens: 300,
        cache_read_tokens: 200,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1900,
    });

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);
    let AppMode::PrReview(state) = &mut app.mode else {
        panic!("expected PR review pane");
    };
    state.usage_baselines.insert(
        source.clone(),
        SessionTokenUsage {
            source,
            input_tokens: 1000,
            output_tokens: 200,
            cache_read_tokens: 100,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            total_tokens: 1300,
        },
    );

    let usage = app.pr_review_triage_session_usage().unwrap();
    assert_eq!(usage.input_tokens, 400);
    assert_eq!(usage.output_tokens, 100);
    assert_eq!(usage.cache_read_tokens, 100);
    assert_eq!(usage.total_tokens, 600);
}

#[test]
fn pr_review_agent_draft_captures_harness_model_usage_and_cost() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    let source = TokenUsageSource {
        provider: TokenUsageProvider::Claude,
        id: "triage-session".to_string(),
    };
    let baseline = SessionTokenUsage {
        source: source.clone(),
        input_tokens: 1_000,
        output_tokens: 200,
        cache_read_tokens: 100,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1_300,
    };
    session.token_usage = Some(baseline.clone());

    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    let tmux_session = app.store.projects[0].features[0].tmux_session.clone();
    app.sidebar_model_cache
        .insert(tmux_session, "Model: claude-sonnet-4-6".to_string());
    let session_id = app.store.projects[0].features[0].sessions[0].id.clone();
    let usage = app.store.projects[0].features[0].sessions[0]
        .token_usage
        .as_mut()
        .unwrap();
    usage.input_tokens += 400;
    usage.output_tokens += 100;
    usage.cache_read_tokens += 100;
    usage.total_tokens += 600;
    // What `pr_review_inject_fix` records at injection time.
    persist_draft_with_provenance(
        &app,
        Some(&crate::app::pr_review::ReplyDraftProvenance {
            harness: "Claude".to_string(),
            session_id,
            model: None,
            usage_baseline: Some(baseline),
        }),
    );

    app.pr_review_open_reply_done();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    let metadata = state
        .reply
        .as_ref()
        .and_then(|reply| reply.generation_metadata.as_ref())
        .expect("captured drafts should disclose their generation details");
    assert_eq!(metadata.harness.as_deref(), Some("Claude"));
    assert_eq!(metadata.model.as_deref(), Some("claude-sonnet-4-6"));
    // Only the spend since the fix was injected, not the session's whole life.
    assert_eq!(metadata.estimated_tokens, Some(600));
    assert_eq!(metadata.estimated_cost.as_deref(), Some("<$0.01"));
    let _ = source;
}

/// Write a captured draft for PR 7 / comment 1 straight into the DB, with the
/// provenance a fix injection would have recorded alongside it.
fn persist_draft_with_provenance(
    app: &App,
    provenance: Option<&crate::app::pr_review::ReplyDraftProvenance>,
) {
    let encoded = provenance.map(|p| serde_json::to_string(p).unwrap());
    let db = app.db.as_ref().unwrap();
    db.begin_pr_comment_reply_draft(7, 1, "request-1", "sha", encoded.as_deref())
        .unwrap();
    assert!(
        db.capture_pr_comment_reply_draft(7, 1, "request-1", "Fixed the guard.")
            .unwrap()
    );
}

/// The reviewer's case: PR Triage is re-opened (which resets `fix_target` to the
/// default) and a *different* agent session is now what that target resolves to.
/// The disclosure has to keep naming the session that wrote the draft.
#[test]
fn pr_review_agent_draft_metadata_ignores_a_changed_fix_target() {
    let mut store = store_with_feature(ProjectStatus::Active);
    // The session the fix actually went to, and a second one the reset fix
    // target lands on instead.
    let drafting = store.projects[0].features[0].add_session_named(
        SessionKind::Codex,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    drafting.token_usage = Some(SessionTokenUsage {
        source: TokenUsageSource {
            provider: TokenUsageProvider::Codex,
            id: "drafting".to_string(),
        },
        input_tokens: 400,
        output_tokens: 100,
        cache_read_tokens: 100,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 600,
    });
    let drafting_id = drafting.id.clone();

    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    persist_draft_with_provenance(
        &app,
        Some(&crate::app::pr_review::ReplyDraftProvenance {
            harness: "Codex".to_string(),
            session_id: drafting_id,
            model: Some("gpt-5.5".to_string()),
            usage_baseline: None,
        }),
    );
    // Whatever the pane now points at is irrelevant: swap the session out for a
    // Claude one under the same triage label.
    app.store.projects[0].features[0].sessions[0].kind = SessionKind::Claude;
    app.store.projects[0].features[0].sessions[0].id = "some-other-session".to_string();

    app.pr_review_open_reply_done();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    let metadata = state
        .reply
        .as_ref()
        .and_then(|reply| reply.generation_metadata.as_ref())
        .expect("captured drafts should disclose their generation details");
    assert_eq!(metadata.harness.as_deref(), Some("Codex"));
    assert_eq!(metadata.model.as_deref(), Some("gpt-5.5"));
    // The drafting session is gone, so its usage is unknown — never the
    // replacement session's numbers.
    assert_eq!(metadata.estimated_tokens, None);
    assert_eq!(metadata.estimated_cost, None);
}

/// A draft persisted before AMF recorded provenance still discloses that it was
/// AI-generated; the details it cannot vouch for read as unreported.
#[test]
fn pr_review_agent_draft_without_provenance_discloses_unknown_details() {
    let mut store = store_with_feature(ProjectStatus::Active);
    store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );

    let db_dir = TempDir::new().unwrap();
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    app.db = Some(crate::db::AmfDb::open(&db_dir.path().join("amf.db")).unwrap());
    enter_pr_review_for_feature(&mut app, 1);
    persist_draft_with_provenance(&app, None);

    app.pr_review_open_reply_done();

    let AppMode::PrReview(state) = &app.mode else {
        panic!("expected PR review pane");
    };
    let reply = state.reply.as_ref().expect("expected a reply dialog");
    assert!(reply.agent_drafted);
    let metadata = reply
        .generation_metadata
        .as_ref()
        .expect("an agent draft always discloses, even with nothing to report");
    assert_eq!(metadata.harness, None);
    assert_eq!(
        metadata.source_disclosure(),
        "AI generation: harness unreported · model unreported"
    );
    assert_eq!(
        metadata.usage_disclosure(),
        "estimated tokens unavailable · Fix cost (est.): unavailable"
    );
}

#[test]
fn pr_review_triage_session_usage_hides_an_unchanged_baseline() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let session = store.projects[0].features[0].add_session_named(
        SessionKind::Claude,
        crate::app::pr_review::TRIAGE_SESSION_LABEL.to_string(),
    );
    let usage = SessionTokenUsage {
        source: TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "unchanged".to_string(),
        },
        input_tokens: 1000,
        output_tokens: 200,
        cache_read_tokens: 100,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1300,
    };
    session.token_usage = Some(usage.clone());

    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    enter_pr_review_for_feature(&mut app, 2);
    let AppMode::PrReview(state) = &mut app.mode else {
        panic!("expected PR review pane");
    };
    state.usage_baselines.insert(usage.source.clone(), usage);

    assert!(app.pr_review_triage_session_usage().is_none());
}

#[test]
fn pr_review_switching_to_live_target_ignores_its_earlier_usage() {
    let mut store = store_with_feature(ProjectStatus::Active);
    let usage = SessionTokenUsage {
        source: TokenUsageSource {
            provider: TokenUsageProvider::Claude,
            id: "live-session".to_string(),
        },
        input_tokens: 800,
        output_tokens: 200,
        cache_read_tokens: 0,
        cache_write_tokens: 0,
        reasoning_tokens: 0,
        total_tokens: 1000,
    };
    store.projects[0].features[0]
        .add_session_named(SessionKind::Claude, "Working".to_string())
        .token_usage = Some(usage.clone());

    let mut worktree = MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(|_| Ok(PathBuf::from("/tmp/test-repo")));
    let mut app = App::new_for_test(store, Box::new(MockTmuxOps::new()), Box::new(worktree));
    enter_pr_review_for_feature(&mut app, 2);

    // Pick "existing live session" from the fix-target picker (replaces the
    // old standalone `t` toggle).
    app.pr_review_open_fix_confirm();
    if let AppMode::PrReview(state) = &mut app.mode {
        let pick = state
            .harness_pick
            .as_mut()
            .expect("fix-target picker should be open");
        pick.selected = pick
            .rows
            .iter()
            .position(|r| matches!(r, crate::app::pr_review::FixTargetPickRow::ExistingLive(_)))
            .expect("existing-live row should be present");
    }
    app.pr_review_harness_pick_confirm();

    assert!(app.pr_review_triage_session_usage().is_none());

    let session = &mut app.store.projects[0].features[0].sessions[0];
    session.token_usage.as_mut().unwrap().input_tokens += 250;
    session.token_usage.as_mut().unwrap().total_tokens += 250;
    let delta = app.pr_review_triage_session_usage().unwrap();
    assert_eq!(delta.input_tokens, 250);
    assert_eq!(delta.total_tokens, 250);
}
