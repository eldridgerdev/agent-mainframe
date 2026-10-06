use super::*;
use crate::{
    app::PlanInterviewState,
    db::AmfDb,
    model_options::{Availability, ModelCapability},
    project::{Feature, Project, ProjectStore, SessionKind, VibeMode},
    traits::{MockTmuxOps, MockWorktreeOps},
};
use std::time::Instant;

fn discover(_: &Path, _: &[AgentKind], _: &AtomicBool) -> Result<Vec<HarnessCapability>> {
    Ok(vec![HarnessCapability {
        harness: AgentKind::Codex,
        availability: Availability::Available,
        model_flag: true,
        reasoning_flag: true,
        models: vec![ModelCapability {
            model: "test-model".into(),
            availability: Availability::Available,
            reasoning_levels: Some(vec!["low".into(), "high".into()]),
        }],
    }])
}
fn discover_only_current_harness(
    workdir: &Path,
    allowed: &[AgentKind],
    cancelled: &AtomicBool,
) -> Result<Vec<HarnessCapability>> {
    assert_eq!(allowed, &[AgentKind::Codex]);
    discover(workdir, allowed, cancelled)
}
fn answer(input: &RunInput) -> Result<String> {
    // Mocks still exercise effective prompt and required-context validation.
    render_analysis_prompt(&input.templates[0].1, &input.context)?;
    let choices: serde_json::Value =
        serde_json::from_str(input.context.get("eligible_options").unwrap())?;
    let choice = choices
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["reasoning"] == "low")
        .unwrap();
    let high = choices
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["reasoning"] == "high")
        .unwrap();
    Ok(serde_json::json!({"status":"qualified","choices":[{"option_id":choice["option_id"],"evidence_ids":choice["evidence_ids"],"priority":"speed"},{"option_id":high["option_id"],"evidence_ids":high["evidence_ids"],"priority":"depth"}]}).to_string())
}

fn discover_claude(
    _: &Path,
    allowed: &[AgentKind],
    _: &AtomicBool,
) -> Result<Vec<HarnessCapability>> {
    assert_eq!(allowed, &[AgentKind::Claude]);
    Ok(vec![HarnessCapability {
        harness: AgentKind::Claude,
        availability: Availability::Available,
        model_flag: true,
        reasoning_flag: true,
        models: vec![ModelCapability {
            model: "claude-sonnet-5-5".into(),
            availability: Availability::Available,
            reasoning_levels: Some(vec!["low".into(), "high".into()]),
        }],
    }])
}

fn configure_claude(app: &mut App) {
    app.store.available_harnesses = vec![AgentKind::Claude];
    app.model_analysis_work.discover = discover_claude;
    app.model_analysis_work.now = || "2026-09-30T12:00:00Z".parse().unwrap();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        let prepared = state.pending_launch.as_mut().unwrap();
        prepared.agent = AgentKind::Claude;
        prepared.session_name = "Claude 1".into();
    }
}
fn fixture(tmux: MockTmuxOps) -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    let mut store = ProjectStore::empty();
    store.projects.push(Project::new(
        "project".into(),
        repo.clone(),
        false,
        AgentKind::Codex,
    ));
    store.available_harnesses = vec![AgentKind::Codex];
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.store_path = dir.path().join("store.json");
    app.db = Some(AmfDb::open(&dir.path().join("db")).unwrap());
    app.config.max_concurrent_agents = 0;
    app.config.low_memory_warn_mb = 0;
    let prepared = PreparedFeatureLaunch {
        model_selection: None,
        project_name: "project".into(),
        feature_name: None,
        branch: "feature".into(),
        workdir: repo,
        is_worktree: false,
        mode: VibeMode::Vibe,
        review: false,
        plan_mode: true,
        quick_plan: false,
        agent: AgentKind::Codex,
        create_terminal: false,
        session_name: "Codex 1".into(),
        enable_chrome: false,
        remote_control: false,
        steering_enabled: false,
        hook_succeeded: None,
        startup_prompt: None,
        todo_origin: None,
        issue_source: None,
    };
    let mut state = PlanInterviewState::for_feature_creation(prepared, vec![]);
    state.apply_synthesis("Implement a small deterministic parser and verify its behavior.".into());
    app.mode = AppMode::PlanInterview(state);
    app.model_analysis_work.now = || "2026-09-29T12:00:00Z".parse().unwrap();
    app.model_analysis_work.discover = discover;
    app.model_analysis_work.runner = answer;
    (app, dir)
}
fn session_fixture(kind: SessionKind) -> (App, tempfile::TempDir) {
    let (mut app, dir) = fixture(MockTmuxOps::new());
    let repo = app.store.projects[0].repo.clone();
    let mut feature = Feature::new_for_project(
        "project",
        "parser".into(),
        "feature/parser".into(),
        repo,
        false,
        VibeMode::Vibe,
        false,
        false,
        AgentKind::Codex,
        false,
        false,
    );
    feature.summary = Some("Handle malformed input without panicking".into());
    feature.add_session(kind);
    app.store.projects[0].features.push(feature);
    app.selection = DashboardSelection::Session(0, 0, 0);
    app.mode = AppMode::Normal;
    (app, dir)
}
fn session_answer(input: &RunInput) -> Result<String> {
    assert_eq!(
        input.context.get("task_phase"),
        Some("existing agent session")
    );
    let context = input.context.get("task_context").unwrap();
    assert!(context.contains("Current feature: parser"));
    assert!(context.contains("Handle malformed input"));
    assert!(context.contains("Current plan:"));
    assert!(context.contains("Validate Unicode boundaries"));
    answer(input)
}
fn change_plan_during_session_analysis(input: &RunInput) -> Result<String> {
    std::fs::write(input.workdir.join("AMF_PLAN.md"), "Revised task")?;
    answer(input)
}
fn poll(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while app.model_analysis_work.pending() {
        assert!(Instant::now() < deadline);
        app.poll_model_analysis();
        std::thread::sleep(Duration::from_millis(2));
    }
}

struct PreparedSession(session_control::Request);
impl session_control::Prepared for PreparedSession {
    fn commit(self: Box<Self>, _: &AtomicBool) -> Result<()> {
        std::fs::write(
            self.0.workdir.join("applied-setting"),
            format!(
                "{}:{}",
                self.0.choice.model(),
                self.0.choice.reasoning().unwrap()
            ),
        )?;
        Ok(())
    }
}
fn prepare_session(
    request: &session_control::Request,
    _: &AtomicBool,
) -> Result<Box<dyn session_control::Prepared>> {
    assert_eq!(request.thread_id, "exact-thread");
    std::fs::write(request.workdir.join("prepared-setting"), "ready")?;
    Ok(Box::new(PreparedSession(request.clone())))
}
fn live_session_fixture() -> (App, tempfile::TempDir) {
    let (mut app, dir) = session_fixture(SessionKind::Codex);
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(true);
    tmux.expect_window_exists().return_const(true);
    app.tmux = Box::new(tmux);
    app.store.projects[0].features[0].sessions[0].set_token_usage_source_exact(
        crate::token_tracking::TokenUsageSource {
            provider: crate::token_tracking::TokenUsageProvider::Codex,
            id: "exact-thread".into(),
        },
    );
    app.model_analysis_work.prepare_session = prepare_session;
    app.open_model_analysis().unwrap();
    poll(&mut app);
    (app, dir)
}
fn wait_for_prepared(app: &App) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while !app.store.projects[0].repo.join("prepared-setting").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn live_session_application_revalidates_and_applies_once_without_launching() {
    let (mut app, _dir) = live_session_fixture();
    let session_id = app.store.projects[0].features[0].sessions[0].id.clone();
    assert!(matches!(&app.mode, AppMode::ModelAnalysis(s) if s.can_apply_session()));
    app.apply_model_analysis().unwrap();
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(
        app.message
            .as_ref()
            .unwrap()
            .contains("verified for subsequent turns")
    );
    assert_eq!(
        std::fs::read_to_string(app.store.projects[0].repo.join("applied-setting")).unwrap(),
        "test-model:low"
    );
    assert_eq!(app.store.projects[0].features[0].sessions.len(), 1);
    assert_eq!(app.store.projects[0].features[0].sessions[0].id, session_id);
    assert!(app.model_analysis_work.launch_args.is_none());
}

#[test]
fn cancelling_prepared_live_application_drops_control_without_mutation() {
    let (mut app, _dir) = live_session_fixture();
    app.apply_model_analysis().unwrap();
    wait_for_prepared(&app);
    app.cancel_model_analysis();
    assert!(matches!(app.mode, AppMode::Normal));
    assert!(!app.store.projects[0].repo.join("applied-setting").exists());
    assert!(!app.poll_model_analysis());
}

#[test]
fn changed_task_conversation_or_deleted_target_prevents_prepared_update() {
    for change in ["plan", "conversation", "deleted", "configuration"] {
        let (mut app, _dir) = live_session_fixture();
        app.apply_model_analysis().unwrap();
        wait_for_prepared(&app);
        match change {
            "plan" => std::fs::write(
                app.store.projects[0].repo.join("AMF_PLAN.md"),
                "Changed task",
            )
            .unwrap(),
            "conversation" => {
                app.store.projects[0].features[0].sessions[0]
                    .token_usage_source
                    .as_mut()
                    .unwrap()
                    .id = "another-thread".into()
            }
            "deleted" => app.store.projects[0].features.clear(),
            "configuration" => app.store.available_harnesses.clear(),
            _ => unreachable!(),
        }
        poll(&mut app);
        assert!(
            matches!(&app.mode, AppMode::ModelAnalysis(s) if matches!(s.status, Status::Error(_))),
            "{change}"
        );
        assert!(!app.store.projects[0].repo.join("applied-setting").exists());
    }
}

#[test]
fn stopped_window_and_inferred_conversation_are_advice_only() {
    let (mut app, _dir) = live_session_fixture();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_session_exists().return_const(false);
    app.tmux = Box::new(tmux);
    assert!(app.apply_model_analysis().is_err());
    assert!(!app.model_analysis_work.pending());
    app.cancel_model_analysis();
    app.store.projects[0].features[0].sessions[0].token_usage_source_match =
        Some(crate::project::TokenUsageSourceMatch::Inferred);
    app.open_model_analysis().unwrap();
    poll(&mut app);
    assert!(matches!(&app.mode, AppMode::ModelAnalysis(s) if !s.can_apply_session()));
    assert!(app.apply_model_analysis().is_err());
}

#[test]
fn live_control_error_retries_analysis_without_repeating_mutation() {
    struct FailedUpdate;
    impl session_control::Prepared for FailedUpdate {
        fn commit(self: Box<Self>, _: &AtomicBool) -> Result<()> {
            anyhow::bail!("Update could not be verified; inspect the harness settings");
        }
    }
    let (mut app, _dir) = live_session_fixture();
    app.model_analysis_work.prepare_session = |_, _| Ok(Box::new(FailedUpdate));
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    assert!(
        matches!(&app.mode, AppMode::ModelAnalysis(s) if !s.committing && s.session_apply.is_none() && matches!(s.status, Status::Error(_)))
    );
    app.model_analysis_work.prepare_session =
        |_, _| panic!("Retry must analyze, not repeat update");
    app.retry_model_analysis();
    poll(&mut app);
    assert!(matches!(&app.mode, AppMode::ModelAnalysis(s) if matches!(s.status, Status::Ready(_))));
}

#[test]
fn retry_during_live_preparation_analyzes_without_applying_the_pending_selection() {
    let (mut app, _dir) = live_session_fixture();
    app.apply_model_analysis().unwrap();
    wait_for_prepared(&app);
    app.model_analysis_work.prepare_session =
        |_, _| panic!("Retry must analyze, not prepare the pending selection again");
    app.retry_model_analysis();
    poll(&mut app);
    assert!(matches!(&app.mode, AppMode::ModelAnalysis(s)
        if s.session_apply.is_none() && matches!(s.status, Status::Ready(_))));
    assert!(!app.store.projects[0].repo.join("applied-setting").exists());
}

#[test]
fn failed_retry_setup_drops_the_prepared_selection_instead_of_replaying_it() {
    let (mut app, _dir) = live_session_fixture();
    app.apply_model_analysis().unwrap();
    wait_for_prepared(&app);
    let conversation = |app: &mut App, id: &str| {
        app.store.projects[0].features[0].sessions[0]
            .token_usage_source
            .as_mut()
            .unwrap()
            .id = id.into()
    };
    conversation(&mut app, "another-thread");
    app.retry_model_analysis();
    assert!(
        matches!(&app.mode, AppMode::ModelAnalysis(s) if s.session_apply.is_none() && !s.committing && !s.is_checking_setting() && matches!(s.status, Status::Error(_)))
    );
    conversation(&mut app, "exact-thread");
    app.model_analysis_work.prepare_session =
        |_, _| panic!("Retry must analyze, not replay the earlier selection");
    app.retry_model_analysis();
    poll(&mut app);
    assert!(matches!(&app.mode, AppMode::ModelAnalysis(s) if matches!(s.status, Status::Ready(_))));
    assert!(!app.store.projects[0].repo.join("applied-setting").exists());
}

#[test]
fn target_change_during_commit_reports_what_happened_to_the_conversation() {
    // Blocks until released, then reports the outcome named in `commit-outcome`.
    struct ScriptedUpdate(session_control::Request);
    impl session_control::Prepared for ScriptedUpdate {
        fn commit(self: Box<Self>, _: &AtomicBool) -> Result<()> {
            let dir = &self.0.workdir;
            std::fs::write(dir.join("committing-setting"), "pending")?;
            let deadline = Instant::now() + Duration::from_secs(3);
            while !dir.join("release-setting").exists() {
                ensure!(Instant::now() < deadline, "test did not release commit");
                std::thread::sleep(Duration::from_millis(2));
            }
            match std::fs::read_to_string(dir.join("commit-outcome"))?.as_str() {
                "applied" => Ok(()),
                "unchanged" => {
                    Err(anyhow::anyhow!("Session settings changed")
                        .context(session_control::NotSent))
                }
                _ => anyhow::bail!("Update was sent, but its result could not be verified"),
            }
        }
    }
    for (outcome, expected) in [
        ("applied", "were updated and verified"),
        (
            "unchanged",
            "Settings were not changed: Session settings changed",
        ),
        (
            "unknown",
            "inspect the conversation's settings (Update was sent",
        ),
    ] {
        let (mut app, _dir) = live_session_fixture();
        let repo = app.store.projects[0].repo.clone();
        std::fs::write(repo.join("commit-outcome"), outcome).unwrap();
        app.model_analysis_work.prepare_session =
            |request, _| Ok(Box::new(ScriptedUpdate(request.clone())));
        app.apply_model_analysis().unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !matches!(&app.mode, AppMode::ModelAnalysis(s) if s.committing) {
            assert!(Instant::now() < deadline);
            app.poll_model_analysis();
            std::thread::sleep(Duration::from_millis(2));
        }
        app.store.projects[0].features[0].sessions[0]
            .token_usage_source
            .as_mut()
            .unwrap()
            .id = "another-thread".into();
        std::fs::write(repo.join("release-setting"), "done").unwrap();
        poll(&mut app);
        assert!(
            matches!(&app.mode, AppMode::ModelAnalysis(s) if !s.committing && s.session_apply.is_none() && matches!(&s.status, Status::Error(e) if e.contains(expected))),
            "{outcome}"
        );
    }
}

#[test]
fn committed_update_cannot_be_cancelled_or_retried_while_verification_is_pending() {
    struct BlockingUpdate(session_control::Request);
    impl session_control::Prepared for BlockingUpdate {
        fn commit(self: Box<Self>, _: &AtomicBool) -> Result<()> {
            std::fs::write(self.0.workdir.join("committing-setting"), "pending")?;
            let deadline = Instant::now() + Duration::from_secs(3);
            while !self.0.workdir.join("release-setting").exists() {
                ensure!(
                    Instant::now() < deadline,
                    "test did not release settings verification"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            Ok(())
        }
    }
    let (mut app, _dir) = live_session_fixture();
    app.model_analysis_work.prepare_session =
        |request, _| Ok(Box::new(BlockingUpdate(request.clone())));
    app.apply_model_analysis().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !matches!(&app.mode, AppMode::ModelAnalysis(s) if s.committing) {
        assert!(Instant::now() < deadline);
        app.poll_model_analysis();
        std::thread::sleep(Duration::from_millis(2));
    }
    let generation = app.model_analysis_work.next;
    app.cancel_model_analysis();
    app.retry_model_analysis();
    app.apply_model_analysis().unwrap();
    assert!(matches!(&app.mode, AppMode::ModelAnalysis(s) if s.committing));
    assert_eq!(app.model_analysis_work.next, generation);
    std::fs::write(app.store.projects[0].repo.join("release-setting"), "done").unwrap();
    poll(&mut app);
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn worker_disconnect_after_committing_unlocks_navigation_without_claiming_success() {
    let (mut app, _dir) = live_session_fixture();
    let (tx, rx) = mpsc::channel();
    drop(tx);
    app.model_analysis_work.job = Some(Job {
        cancelled: Arc::new(AtomicBool::new(false)),
        rx,
    });
    if let AppMode::ModelAnalysis(s) = &mut app.mode {
        s.committing = true;
    }
    assert!(app.poll_model_analysis());
    assert!(
        matches!(&app.mode, AppMode::ModelAnalysis(s) if !s.committing && matches!(&s.status, Status::Error(e) if e.contains("inspect the harness settings")))
    );
    app.cancel_model_analysis();
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn plan_changes_beyond_prompt_limit_and_expired_research_prevent_live_application() {
    for expired in [false, true] {
        let (mut app, _dir) = live_session_fixture();
        if !expired {
            app.cancel_model_analysis();
            std::fs::write(
                app.store.projects[0].repo.join("AMF_PLAN.md"),
                "x".repeat(25_000),
            )
            .unwrap();
            app.open_model_analysis().unwrap();
            poll(&mut app);
        }
        app.apply_model_analysis().unwrap();
        wait_for_prepared(&app);
        if expired {
            app.model_analysis_work.now = || "2026-11-01T12:00:00Z".parse().unwrap();
        } else {
            std::fs::write(
                app.store.projects[0].repo.join("AMF_PLAN.md"),
                format!("{}changed", "x".repeat(25_000)),
            )
            .unwrap();
        }
        poll(&mut app);
        assert!(
            matches!(&app.mode, AppMode::ModelAnalysis(s) if matches!(s.status, Status::Error(_)))
        );
        assert!(!app.store.projects[0].repo.join("applied-setting").exists());
    }
}
#[test]
fn dashboard_session_advice_uses_effective_plan_and_cannot_change_running_harness() {
    let (mut app, _dir) = session_fixture(SessionKind::Codex);
    std::fs::write(
        app.store.projects[0].repo.join("AMF_PLAN.md"),
        "Validate Unicode boundaries before parsing.",
    )
    .unwrap();
    app.model_analysis_work.discover = discover_only_current_harness;
    app.model_analysis_work.runner = session_answer;
    crate::handlers::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('m'),
            crossterm::event::KeyModifiers::NONE,
        ),
        20,
    )
    .unwrap();
    poll(&mut app);
    assert!(
        matches!(&app.mode,AppMode::ModelAnalysis(s) if s.is_existing_session() && matches!(&s.status,Status::Ready(r) if r.len()==2))
    );
    assert!(app.apply_model_analysis().is_err());
    assert!(app.model_analysis_work.launch_args.is_none());
    app.cancel_model_analysis();
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn claude_session_advice_uses_its_own_sources_and_remains_view_only() {
    let (mut app, _dir) = session_fixture(SessionKind::Claude);
    configure_claude(&mut app);
    app.open_model_analysis().unwrap();
    poll(&mut app);
    let AppMode::ModelAnalysis(state) = &app.mode else {
        panic!()
    };
    let Status::Ready(choices) = &state.status else {
        panic!("{:?}", state.status)
    };
    assert_eq!(choices.len(), 2);
    assert!(
        choices
            .iter()
            .all(|r| *r.choice.harness() == AgentKind::Claude
                && r.evidence.iter().all(|n| n.source.contains("claude.com")))
    );
    assert!(app.apply_model_analysis().is_err());
    app.cancel_model_analysis();
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn claude_selection_is_rejected_when_its_effort_cap_changes_before_launch() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    configure_claude(&mut app);
    app.open_model_analysis().unwrap();
    poll(&mut app);
    if let AppMode::ModelAnalysis(state) = &mut app.mode {
        state.selected = 1; // high
    }
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    app.model_analysis_work.discover = |workdir, allowed, cancelled| {
        let mut caps = discover_claude(workdir, allowed, cancelled)?;
        caps[0].models[0].reasoning_levels = Some(vec!["low".into()]);
        Ok(caps)
    };
    app.complete_plan_interview_with_resource_approval(true)
        .unwrap();
    poll(&mut app);
    assert!(app.store.projects[0].features.is_empty());
    assert!(matches!(&app.mode, AppMode::ModelAnalysis(s) if matches!(s.status, Status::Error(_))));
}

#[test]
fn pane_session_advice_returns_to_pane_and_rejects_deleted_target() {
    let (mut app, _dir) = session_fixture(SessionKind::Codex);
    let feature = &app.store.projects[0].features[0];
    let session = &feature.sessions[0];
    app.mode = AppMode::Viewing(super::super::ViewState::new(
        "project".into(),
        feature.name.clone(),
        feature.tmux_session.clone(),
        session.tmux_window.clone(),
        session.label.clone(),
        session.kind.clone(),
        feature.mode.clone(),
        feature.review,
    ));
    app.activate_leader();
    crate::handlers::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('B'),
            crossterm::event::KeyModifiers::NONE,
        ),
        20,
    )
    .unwrap();
    assert!(matches!(app.mode, AppMode::ModelAnalysis(_)));
    app.store.projects[0].features[0].sessions.clear();
    poll(&mut app);
    assert!(matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(s.status,Status::Error(_))));
    app.cancel_model_analysis();
    assert!(matches!(app.mode, AppMode::Viewing(_)));
}

#[test]
fn non_agent_sessions_are_rejected_and_unverified_harnesses_are_insufficient() {
    let (mut app, _dir) = session_fixture(SessionKind::Terminal);
    assert!(app.open_model_analysis().is_err());
    app.store.projects[0].features[0].sessions[0].kind = SessionKind::Claude;
    app.open_model_analysis().unwrap();
    poll(&mut app);
    assert!(
        matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(s.status,Status::Insufficient))
    );
}

#[test]
fn session_plan_edit_during_analysis_rejects_stale_advice() {
    let (mut app, _dir) = session_fixture(SessionKind::Codex);
    std::fs::write(
        app.store.projects[0].repo.join("AMF_PLAN.md"),
        "Original task",
    )
    .unwrap();
    app.model_analysis_work.runner = change_plan_during_session_analysis;
    app.open_model_analysis().unwrap();
    poll(&mut app);
    assert!(
        matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(&s.status,Status::Error(e) if e.contains("task context changed")))
    );
}
#[test]
fn cancel_restores_review_without_launch_or_late_result() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    app.open_model_analysis().unwrap();
    let flag = app
        .model_analysis_work
        .job
        .as_ref()
        .unwrap()
        .cancelled
        .clone();
    app.cancel_model_analysis();
    assert!(flag.load(Ordering::Relaxed));
    assert!(matches!(app.mode, AppMode::PlanInterview(_)));
    assert!(!app.poll_model_analysis());
    assert!(app.store.projects[0].features.is_empty());
}
#[test]
fn failure_retry_insufficient_and_navigation() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    app.model_analysis_work.runner = |_| anyhow::bail!("mock runner failed");
    app.open_model_analysis().unwrap();
    poll(&mut app);
    assert!(
        matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(&s.status,Status::Error(e) if e.contains("mock runner failed")))
    );
    app.model_analysis_work.runner = answer;
    app.retry_model_analysis();
    poll(&mut app);
    assert!(matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(&s.status,Status::Ready(_))));
    crate::handlers::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Down,
            crossterm::event::KeyModifiers::NONE,
        ),
        20,
    )
    .unwrap();
    assert!(matches!(&app.mode,AppMode::ModelAnalysis(s) if s.selected==1));
    crate::handlers::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Up,
            crossterm::event::KeyModifiers::NONE,
        ),
        20,
    )
    .unwrap();
    assert!(matches!(&app.mode,AppMode::ModelAnalysis(s) if s.selected==0));
    for key in [
        crossterm::event::KeyCode::Char('s'),
        crossterm::event::KeyCode::PageDown,
    ] {
        crate::handlers::handle_key(
            &mut app,
            crossterm::event::KeyEvent::new(key, crossterm::event::KeyModifiers::NONE),
            20,
        )
        .unwrap();
    }
    assert!(matches!(&app.mode,AppMode::ModelAnalysis(s) if s.show_sources && s.scroll==8));
    crate::handlers::handle_key(
        &mut app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('s'),
            crossterm::event::KeyModifiers::NONE,
        ),
        20,
    )
    .unwrap();
    assert!(matches!(&app.mode,AppMode::ModelAnalysis(s) if !s.show_sources && s.scroll==0));
    app.model_analysis_work.discover = |_, _, _| Ok(vec![]);
    app.retry_model_analysis();
    poll(&mut app);
    assert!(
        matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(&s.status,Status::Insufficient))
    );
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    assert!(app.store.projects[0].features.is_empty());
}
#[test]
fn changed_plan_deleted_project_and_config_reject_results() {
    for change in 0..3 {
        let (mut app, _dir) = fixture(MockTmuxOps::new());
        app.open_model_analysis().unwrap();
        match change {
            0 => {
                if let AppMode::ModelAnalysis(s) = &mut app.mode
                    && let AppMode::PlanInterview(p) = &mut *s.origin
                {
                    p.synthesized_plan = Some("changed".into());
                }
            }
            1 => app.store.projects.clear(),
            _ => app.store.available_harnesses = vec![AgentKind::Pi],
        }
        poll(&mut app);
        assert!(
            matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(&s.status,Status::Error(_)))
        );
    }
}

#[test]
fn application_checks_availability_and_plan_edits_invalidate_selection() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    app.open_model_analysis().unwrap();
    poll(&mut app);
    app.model_analysis_work.discover = |_, _, _| Ok(vec![]);
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    assert!(matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(s.status,Status::Error(_))));
    app.cancel_model_analysis();
    app.model_analysis_work.discover = discover;
    app.open_model_analysis().unwrap();
    poll(&mut app);
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    if let AppMode::PlanInterview(s) = &mut app.mode {
        s.synthesized_plan = Some("changed plan".into());
    }
    assert!(
        app.complete_plan_interview_with_resource_approval(true)
            .is_err()
    );
    assert!(app.store.projects[0].features.is_empty());
}

#[test]
fn disconnected_and_old_generations_cannot_replace_current_request() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    app.open_model_analysis().unwrap();
    app.model_analysis_work.cancel();
    let (generation, target) = match &app.mode {
        AppMode::ModelAnalysis(s) => (s.generation, s.target.clone()),
        _ => unreachable!(),
    };
    let (tx, rx) = mpsc::channel();
    app.model_analysis_work.job = Some(Job {
        cancelled: Arc::new(AtomicBool::new(false)),
        rx,
    });
    tx.send(Completion {
        generation: generation + 1,
        target: target.clone(),
        allowed: vec![AgentKind::Codex],
        result: Ok(Outcome::Advice(vec![], None)),
    })
    .unwrap();
    assert!(!app.poll_model_analysis());
    assert!(app.model_analysis_work.pending());
    tx.send(Completion {
        generation,
        target,
        allowed: vec![AgentKind::Codex],
        result: Ok(Outcome::Advice(vec![], None)),
    })
    .unwrap();
    assert!(app.poll_model_analysis());
    assert!(
        matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(s.status,Status::Insufficient))
    );
    let (tx, rx) = mpsc::channel();
    drop(tx);
    app.model_analysis_work.job = Some(Job {
        cancelled: Arc::new(AtomicBool::new(false)),
        rx,
    });
    assert!(app.poll_model_analysis());
    assert!(matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(s.status,Status::Error(_))));
}

#[test]
fn analyzer_prompt_obeys_all_override_layers_and_validates_winning_template() {
    use crate::db::prompt_overrides::OverrideScope;
    use crate::prompts::PromptSource;
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    let target = app.model_target(&app.mode).unwrap();
    let id = PromptId::ModelAnalysis;
    let resolve = |app: &App| {
        app.resolve_headless_template(id, &AgentKind::Codex, &target.repo, &target.workdir)
    };
    assert_eq!(resolve(&app).1, PromptSource::BuiltIn);
    let db = app.db.as_ref().unwrap();
    db.upsert_prompt_override(id.as_str(), &OverrideScope::Global, None, "global")
        .unwrap();
    assert_eq!(resolve(&app), ("global".into(), PromptSource::Global));
    std::fs::write(
        target.repo.join("amf.json"),
        serde_json::json!({"prompt_overrides":{id.as_str():{"template":"project"}}}).to_string(),
    )
    .unwrap();
    assert_eq!(resolve(&app), ("project".into(), PromptSource::Project));
    db.upsert_prompt_override(
        id.as_str(),
        &OverrideScope::Feature {
            workdir: target.workdir.to_string_lossy().into_owned(),
        },
        None,
        "feature with missing context",
    )
    .unwrap();
    assert_eq!(resolve(&app).1, PromptSource::Feature);
    app.open_model_analysis().unwrap();
    poll(&mut app);
    assert!(
        matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(&s.status,Status::Error(e) if e.contains("omits required context")))
    );
}

#[test]
fn resource_confirmation_restores_review_before_fresh_eligibility_check() {
    use crate::app::{PendingStart, ResourceConfirmState};
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    app.open_model_analysis().unwrap();
    poll(&mut app);
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    let AppMode::PlanInterview(state) = std::mem::replace(&mut app.mode, AppMode::Normal) else {
        panic!()
    };
    let pending = PendingPlanLaunch {
        prepared: state.pending_launch.clone().unwrap(),
        interview_key: state.interview_key.clone(),
        plan: state.synthesized_plan.clone().unwrap(),
    };
    app.mode = AppMode::ConfirmResourceStart(Box::new(ResourceConfirmState {
        pending: PendingStart::PlannedFeature(Box::new(pending)),
        plan_interview: Some(state),
        from_view: None,
        over_limit: None,
        low_memory: None,
        open_editors: vec![],
    }));
    app.model_analysis_work.discover = |_, _, _| Ok(vec![]);
    app.confirm_pending_start().unwrap();
    poll(&mut app);
    assert!(matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(s.status,Status::Error(_))));
    app.cancel_model_analysis();
    assert!(matches!(app.mode, AppMode::PlanInterview(_)));
    assert!(app.store.projects[0].features.is_empty());
}
#[test]
fn selected_setting_reaches_execution_and_confirm_is_idempotent() {
    selected_launch(false);
}

#[test]
fn todo_selected_setting_reaches_execution_once_and_retains_todo_links() {
    selected_launch(true);
}

fn selected_launch(from_todo: bool) {
    let mut tmux = MockTmuxOps::new();
    let alive = Arc::new(AtomicBool::new(false));
    let exists = alive.clone();
    let created = alive.clone();
    tmux.expect_session_exists()
        .returning(move |_| exists.load(Ordering::Relaxed));
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(move |_, _, _| {
            created.store(true, Ordering::Relaxed);
            Ok(())
        });
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_launch_codex()
        .times(1)
        .withf(|_, _, _, _, args| {
            args.windows(2).any(|a| a == ["--model", "test-model"])
                && args.contains(&"model_reasoning_effort=\"low\"".to_string())
        })
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));
    let (mut app, _dir) = fixture(tmux);
    let todo = from_todo.then(|| attach_todo(&mut app, "source-feature"));
    // Advice may switch the originally prepared harness as well as settings.
    if let AppMode::PlanInterview(state) = &mut app.mode {
        let prepared = state.pending_launch.as_mut().unwrap();
        prepared.agent = AgentKind::Claude;
        prepared.session_name = "Claude 1".into();
    }
    app.open_model_analysis().unwrap();
    poll(&mut app);
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    assert!(app.store.projects[0].features.is_empty());
    app.complete_plan_interview_with_resource_approval(true)
        .unwrap();
    app.complete_plan_interview_with_resource_approval(true)
        .unwrap();
    poll(&mut app);
    assert_eq!(app.store.projects[0].features.len(), 1);
    assert_eq!(app.store.projects[0].features[0].agent, AgentKind::Codex);
    assert_eq!(
        app.store.projects[0].features[0].sessions[0].label,
        "Codex 1"
    );
    assert!(app.model_analysis_work.launch_args.is_none());
    if let Some(origin) = todo {
        let todo = app
            .db
            .as_ref()
            .unwrap()
            .find_todo_by_id(&origin.todo_id)
            .unwrap()
            .unwrap();
        let feature = &app.store.projects[0].features[0];
        assert_eq!(todo.linked_feature_id.as_deref(), Some(feature.id.as_str()));
        assert_eq!(
            todo.work.agent_session_id.as_deref(),
            Some(feature.sessions[0].id.as_str())
        );
        assert_eq!(todo.work.status, crate::db::todos::TodoStatus::InProgress);
        assert_eq!(
            feature.sessions[0].todo_reference.as_ref().unwrap().todo_id,
            origin.todo_id
        );
    }
}

#[test]
fn selected_claude_model_and_effort_reach_initial_launch_once() {
    let mut tmux = MockTmuxOps::new();
    let alive = Arc::new(AtomicBool::new(false));
    let exists = alive.clone();
    let created = alive.clone();
    tmux.expect_session_exists()
        .returning(move |_| exists.load(Ordering::Relaxed));
    tmux.expect_create_session_with_window()
        .times(1)
        .returning(move |_, _, _| {
            created.store(true, Ordering::Relaxed);
            Ok(())
        });
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    tmux.expect_launch_claude()
        .times(1)
        .withf(|_, _, _, _, args| {
            args.windows(2)
                .any(|a| a == ["--model", "claude-sonnet-5-5"])
                && args.windows(2).any(|a| a == ["--effort", "low"])
        })
        .returning(|_, _, _, _, _| Ok(()));
    tmux.expect_select_window().returning(|_, _| Ok(()));
    let (mut app, _dir) = fixture(tmux);
    configure_claude(&mut app);
    app.open_model_analysis().unwrap();
    poll(&mut app);
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    app.complete_plan_interview_with_resource_approval(true)
        .unwrap();
    app.complete_plan_interview_with_resource_approval(true)
        .unwrap();
    poll(&mut app);
    assert_eq!(app.store.projects[0].features.len(), 1);
    assert_eq!(app.store.projects[0].features[0].agent, AgentKind::Claude);
    assert_eq!(
        app.store.projects[0].features[0].sessions[0].label,
        "Claude 1"
    );
    assert!(app.model_analysis_work.launch_args.is_none());
}
#[test]
fn eligibility_is_rechecked_after_selection() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    app.open_model_analysis().unwrap();
    poll(&mut app);
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    app.model_analysis_work.discover = |_, _, _| Ok(vec![]);
    app.complete_plan_interview_with_resource_approval(true)
        .unwrap();
    poll(&mut app);
    assert!(app.store.projects[0].features.is_empty());
    assert!(matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(&s.status,Status::Error(_))));
}
#[test]
fn launch_failure_retries_same_feature_and_settings() {
    retry_failed_launch(false);
}

#[test]
fn todo_launch_failure_retains_reservation_and_retries_same_destination() {
    retry_failed_launch(true);
}

fn retry_failed_launch(from_todo: bool) {
    let mut tmux = MockTmuxOps::new();
    let alive = Arc::new(AtomicBool::new(false));
    let exists = alive.clone();
    let created = alive.clone();
    let killed = alive.clone();
    tmux.expect_session_exists()
        .returning(move |_| exists.load(Ordering::Relaxed));
    tmux.expect_create_session_with_window()
        .times(2)
        .returning(move |_, _, _| {
            created.store(true, Ordering::Relaxed);
            Ok(())
        });
    tmux.expect_set_session_env().returning(|_, _, _| Ok(()));
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count_run = count.clone();
    tmux.expect_launch_codex()
        .times(2)
        .returning(move |_, _, _, _, args| {
            assert!(args.contains(&"test-model".to_string()));
            if count_run.fetch_add(1, Ordering::Relaxed) == 0 {
                anyhow::bail!("mock launch failure")
            }
            Ok(())
        });
    tmux.expect_kill_session().times(1).returning(move |_| {
        killed.store(false, Ordering::Relaxed);
        Ok(())
    });
    tmux.expect_select_window().returning(|_, _| Ok(()));
    let (mut app, _dir) = fixture(tmux);
    let todo = from_todo.then(|| attach_todo(&mut app, "source"));
    app.open_model_analysis().unwrap();
    poll(&mut app);
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    app.complete_plan_interview_with_resource_approval(true)
        .unwrap();
    poll(&mut app);
    assert!(matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(&s.status,Status::Error(_))));
    assert_eq!(app.store.projects[0].features.len(), 1);
    let id = app.store.projects[0].features[0].id.clone();
    if let Some(origin) = &todo {
        let todo = app
            .db
            .as_ref()
            .unwrap()
            .find_todo_by_id(&origin.todo_id)
            .unwrap()
            .unwrap();
        assert_eq!(todo.work.status, crate::db::todos::TodoStatus::InProgress);
        assert!(todo.linked_feature_id.is_none());
        assert!(todo.work.agent_session_id.is_none());
    }
    app.retry_model_analysis();
    poll(&mut app);
    assert_eq!(app.store.projects[0].features.len(), 1);
    assert_eq!(app.store.projects[0].features[0].id, id);
    assert_eq!(count.load(Ordering::Relaxed), 2);
    if let Some(origin) = &todo {
        let todo = app
            .db
            .as_ref()
            .unwrap()
            .find_todo_by_id(&origin.todo_id)
            .unwrap()
            .unwrap();
        assert_eq!(todo.linked_feature_id.as_deref(), Some(id.as_str()));
    }
}

#[test]
fn options_that_cannot_launch_in_the_prepared_mode_are_excluded() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    if let AppMode::PlanInterview(s) = &mut app.mode {
        s.pending_launch.as_mut().unwrap().mode = VibeMode::Vibeless;
    }
    app.model_analysis_work.runner = |_| panic!("must not analyze inapplicable options");
    app.open_model_analysis().unwrap();
    poll(&mut app);
    assert!(
        matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(s.status,Status::Insufficient))
    );
}

#[test]
fn changed_plan_file_during_launch_validation_does_not_start_an_agent() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    app.open_model_analysis().unwrap();
    poll(&mut app);
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    let target = app.model_target(&app.mode).unwrap();
    app.complete_plan_interview_with_resource_approval(true)
        .unwrap();
    std::fs::write(
        target.workdir.join("AMF_PLAN.md"),
        "externally changed plan",
    )
    .unwrap();
    poll(&mut app);
    assert!(
        matches!(&app.mode,AppMode::ModelAnalysis(s) if matches!(&s.status,Status::Error(e) if e.contains("plan file changed")))
    );
    assert!(app.store.projects[0].features.is_empty());
}

fn attach_todo(app: &mut App, host: &str) -> TodoPlanOrigin {
    use crate::db::todos::{TodoPriority, TodoScope, TodoStatus};
    let db = app.db.as_ref().unwrap();
    // A global source deliberately differs from the resolved destination project.
    let list = db
        .load_or_create_todo_list(&TodoScope::Global, None)
        .unwrap();
    let mut todo = db
        .add_todo(
            &list.id,
            "Implement parser",
            Some("Check Unicode"),
            TodoPriority::Med,
        )
        .unwrap();
    todo.work.status = TodoStatus::InProgress;
    db.update_todo(&todo).unwrap();
    let origin = TodoPlanOrigin {
        todo_id: todo.id,
        list_id: list.id,
        todo_title: todo.title,
        host_feature_id: host.into(),
    };
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.todo_origin = Some(origin.clone());
        if let Some(prepared) = &mut state.pending_launch {
            prepared.todo_origin = Some(origin.clone());
        }
    }
    origin
}

fn reviewed_feature_fixture(quick: bool) -> (App, tempfile::TempDir) {
    let (mut app, dir) = session_fixture(SessionKind::Codex);
    let feature = &app.store.projects[0].features[0];
    let mut state = if quick {
        PlanInterviewState::for_feature_quick(
            feature.name.clone(),
            feature.id.clone(),
            feature.workdir.clone(),
            AgentKind::Claude,
        )
    } else {
        PlanInterviewState::for_feature(
            feature.name.clone(),
            feature.id.clone(),
            vec![],
            feature.workdir.clone(),
            AgentKind::Claude,
        )
    };
    // Planning runner preference is not the implementation harness identity.
    state.apply_synthesis("Reviewed task: handle Unicode parser boundaries.".into());
    app.mode = AppMode::PlanInterview(state);
    app.model_analysis_work.discover = discover_only_current_harness;
    app.model_analysis_work.runner = reviewed_plan_answer;
    (app, dir)
}

fn reviewed_plan_answer(input: &RunInput) -> Result<String> {
    assert_eq!(input.context.get("task_phase"), Some("implementation"));
    assert!(
        input
            .context
            .get("task_context")
            .unwrap()
            .contains("Reviewed task:")
    );
    assert_eq!(input.preferred, AgentKind::Codex);
    answer(input)
}

#[test]
fn existing_full_and_quick_plan_review_offer_view_only_implementation_advice() {
    for quick in [false, true] {
        let (mut app, _dir) = reviewed_feature_fixture(quick);
        assert!(app.model_analysis_available());
        crate::handlers::handle_key(
            &mut app,
            crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char('m'),
                crossterm::event::KeyModifiers::NONE,
            ),
            20,
        )
        .unwrap();
        poll(&mut app);
        assert!(
            matches!(&app.mode, AppMode::ModelAnalysis(s) if s.scope() == AdviceScope::ExistingPlan && matches!(s.status, Status::Ready(_)))
        );
        assert!(
            app.apply_model_analysis()
                .unwrap_err()
                .to_string()
                .contains("view-only")
        );
        app.cancel_model_analysis();
        assert!(
            matches!(&app.mode, AppMode::PlanInterview(s) if s.pending_launch.is_none() && s.synthesized_plan.as_ref().unwrap().contains("Reviewed task:"))
        );
        assert!(!app.store.projects[0].repo.join("AMF_PLAN.md").exists());
        assert_eq!(app.store.projects[0].features[0].sessions.len(), 1);
    }
}

#[test]
fn host_todo_plan_advice_keeps_the_existing_plan_and_reservation_untouched() {
    let (mut app, _dir) = reviewed_feature_fixture(false);
    let host = app.store.projects[0].features[0].id.clone();
    let origin = attach_todo(&mut app, &host);
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.interview_key = crate::plan_interview::todo_interview_key(&origin.todo_id);
    }
    let db = app.db.as_ref().unwrap();
    let before = db.find_todo_by_id(&origin.todo_id).unwrap().unwrap();
    let plan = app.store.projects[0].repo.join("AMF_PLAN.md");
    std::fs::write(&plan, "Existing feature plan").unwrap();
    app.open_model_analysis().unwrap();
    poll(&mut app);
    assert!(
        matches!(&app.mode, AppMode::ModelAnalysis(s) if s.scope() == AdviceScope::HostTodoPlan && matches!(s.status, Status::Ready(_)))
    );
    assert!(app.apply_model_analysis().is_err());
    app.cancel_model_analysis();
    let after = app
        .db
        .as_ref()
        .unwrap()
        .find_todo_by_id(&origin.todo_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(before).unwrap(),
        serde_json::to_value(after).unwrap()
    );
    assert_eq!(
        std::fs::read_to_string(plan).unwrap(),
        "Existing feature plan"
    );
    assert!(
        matches!(&app.mode, AppMode::PlanInterview(s) if s.todo_origin.as_ref() == Some(&origin))
    );
}

#[test]
fn existing_plan_advice_rejects_changed_plan_feature_harness_and_destination() {
    for change in 0..6 {
        let (mut app, _dir) = reviewed_feature_fixture(false);
        app.open_model_analysis().unwrap();
        match change {
            0 => {
                if let AppMode::ModelAnalysis(s) = &mut app.mode
                    && let AppMode::PlanInterview(p) = s.origin.as_mut()
                {
                    p.apply_synthesis("Changed implementation task".into());
                }
            }
            1 => app.store.projects[0].features.clear(),
            2 => app.store.projects[0].features[0].agent = AgentKind::Claude,
            3 => app.store.projects[0].id = "other-project".into(),
            4 => app.store.projects[0].features[0].workdir = PathBuf::from("/removed"),
            _ => app.store.available_harnesses.clear(),
        }
        poll(&mut app);
        assert!(
            matches!(&app.mode, AppMode::ModelAnalysis(s) if matches!(s.status, Status::Error(_))),
            "change {change}"
        );
        assert!(app.model_analysis_work.launch_args.is_none());
    }
}

#[test]
fn todo_plan_advice_rejects_deleted_edited_completed_or_moved_source() {
    for change in 0..4 {
        let (mut app, _dir) = fixture(MockTmuxOps::new());
        let origin = attach_todo(&mut app, "source");
        app.open_model_analysis().unwrap();
        let db = app.db.as_ref().unwrap();
        if change == 0 {
            db.delete_todo(&origin.todo_id).unwrap();
        } else if change == 3 {
            let list = db
                .load_or_create_todo_list(
                    &crate::db::todos::TodoScope::Project {
                        project_id: app.store.projects[0].id.clone(),
                    },
                    None,
                )
                .unwrap();
            db.move_todo(&origin.todo_id, &list.id).unwrap();
        } else {
            let mut todo = db.find_todo_by_id(&origin.todo_id).unwrap().unwrap();
            if change == 1 {
                todo.body = Some("Different implementation".into());
            } else {
                todo.work.status = crate::db::todos::TodoStatus::Completed;
            }
            db.update_todo(&todo).unwrap();
        }
        poll(&mut app);
        assert!(
            matches!(&app.mode, AppMode::ModelAnalysis(s) if matches!(s.status, Status::Error(_))),
            "change {change}"
        );
        assert!(app.store.projects[0].features.is_empty());
    }
}

#[test]
fn todo_selection_is_invalidated_between_application_and_acceptance() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    let origin = attach_todo(&mut app, "source");
    app.open_model_analysis().unwrap();
    poll(&mut app);
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    let db = app.db.as_ref().unwrap();
    let mut todo = db.find_todo_by_id(&origin.todo_id).unwrap().unwrap();
    todo.body = Some("Changed after selection".into());
    db.update_todo(&todo).unwrap();
    assert!(
        app.complete_plan_interview_with_resource_approval(true)
            .is_err()
    );
    assert!(matches!(app.mode, AppMode::PlanInterview(_)));
    assert!(app.store.projects[0].features.is_empty());
}

#[test]
fn todo_target_ignores_list_churn_that_leaves_the_todo_unchanged() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    let origin = attach_todo(&mut app, "source");
    let before = app.model_target(&app.mode).unwrap();
    let db = app.db.as_ref().unwrap();
    db.add_todo(
        &origin.list_id,
        "Unrelated sibling",
        None,
        crate::db::todos::TodoPriority::Low,
    )
    .unwrap();
    db.set_todo_carry_over(&origin.list_id, Some("left off here"))
        .unwrap();
    let mut todo = db.find_todo_by_id(&origin.todo_id).unwrap().unwrap();
    todo.sort_order += 10;
    db.update_todo(&todo).unwrap();
    assert_eq!(app.model_target(&app.mode).unwrap(), before);
    todo.body = Some("Changed after selection".into());
    db.update_todo(&todo).unwrap();
    assert_ne!(app.model_target(&app.mode).unwrap(), before);
}

#[test]
fn plan_review_hides_model_advice_where_it_cannot_open() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    assert!(app.model_analysis_available());
    attach_todo(&mut app, "source");
    assert!(app.model_analysis_available());
    app.db = None;
    assert!(!app.model_analysis_available());
    advice_key(&mut app, crossterm::event::KeyCode::Char('m'));
    assert!(matches!(app.mode, AppMode::PlanInterview(_)));
    assert!(app.message.as_deref().unwrap().contains("TODO database"));

    let (mut app, _dir) = reviewed_feature_fixture(false);
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.synthesized_plan = Some("  ".into());
    }
    assert!(!app.model_analysis_available());
}

#[test]
fn cancelled_todo_advice_drops_late_results_without_changing_reservation() {
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    let origin = attach_todo(&mut app, "source");
    app.open_model_analysis().unwrap();
    let cancelled = app
        .model_analysis_work
        .job
        .as_ref()
        .unwrap()
        .cancelled
        .clone();
    app.cancel_model_analysis();
    assert!(cancelled.load(Ordering::Relaxed));
    assert!(!app.poll_model_analysis());
    assert!(
        matches!(&app.mode, AppMode::PlanInterview(s) if s.todo_origin.as_ref() == Some(&origin))
    );
    let todo = app
        .db
        .as_ref()
        .unwrap()
        .find_todo_by_id(&origin.todo_id)
        .unwrap()
        .unwrap();
    assert_eq!(todo.work.status, crate::db::todos::TodoStatus::InProgress);
    assert!(todo.work.agent_session_id.is_none());
    assert!(todo.linked_feature_id.is_none());
}

#[test]
fn todo_advice_uses_destination_project_research_instead_of_source_project() {
    use crate::db::todos::TodoScope;
    let (mut app, dir) = fixture(MockTmuxOps::new());
    let origin = attach_todo(&mut app, "source");
    let source_repo = dir.path().join("source-repo");
    std::fs::create_dir(&source_repo).unwrap();
    let source = Project::new(
        "source-project".into(),
        source_repo.clone(),
        false,
        AgentKind::Claude,
    );
    let source_id = source.id.clone();
    let destination_id = app.store.projects[0].id.clone();
    let destination_repo = app.store.projects[0].repo.clone();
    let db = app.db.as_ref().unwrap();
    let source_list = db
        .load_or_create_todo_list(
            &TodoScope::Project {
                project_id: source_id.clone(),
            },
            None,
        )
        .unwrap();
    db.move_todo(&origin.todo_id, &source_list.id).unwrap();
    if let AppMode::PlanInterview(s) = &mut app.mode {
        s.todo_origin.as_mut().unwrap().list_id = source_list.id.clone();
        s.pending_launch
            .as_mut()
            .unwrap()
            .todo_origin
            .as_mut()
            .unwrap()
            .list_id = source_list.id;
    }
    app.store.projects.push(source);
    app.open_model_analysis().unwrap();
    poll(&mut app);
    assert!(
        matches!(&app.mode, AppMode::ModelAnalysis(s) if s.target.project_id == destination_id && s.target.repo == destination_repo && matches!(s.status, Status::Ready(_)))
    );
    let db = app.db.as_ref().unwrap();
    assert!(
        !db.load_model_research(&destination_id, &destination_repo)
            .unwrap()
            .is_empty()
    );
    assert!(
        db.load_model_research(&source_id, &source_repo)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn todo_changed_while_resource_confirmation_is_open_cannot_launch() {
    use crate::app::{PendingStart, ResourceConfirmState};
    let (mut app, _dir) = fixture(MockTmuxOps::new());
    let origin = attach_todo(&mut app, "source");
    app.open_model_analysis().unwrap();
    poll(&mut app);
    app.apply_model_analysis().unwrap();
    poll(&mut app);
    let AppMode::PlanInterview(state) = std::mem::replace(&mut app.mode, AppMode::Normal) else {
        panic!()
    };
    let pending = PendingPlanLaunch {
        prepared: state.pending_launch.clone().unwrap(),
        interview_key: state.interview_key.clone(),
        plan: state.synthesized_plan.clone().unwrap(),
    };
    app.mode = AppMode::ConfirmResourceStart(Box::new(ResourceConfirmState {
        pending: PendingStart::PlannedFeature(Box::new(pending)),
        plan_interview: Some(state),
        from_view: None,
        over_limit: None,
        low_memory: None,
        open_editors: vec![],
    }));
    app.db
        .as_ref()
        .unwrap()
        .delete_todo(&origin.todo_id)
        .unwrap();
    assert!(app.confirm_pending_start().is_err());
    assert!(matches!(app.mode, AppMode::PlanInterview(_)));
    assert!(app.store.projects[0].features.is_empty());
}

fn expert_fixture(existing: bool, quick: bool) -> (App, tempfile::TempDir) {
    use crate::{
        app::{AiModelPickState, ModelPickRow},
        headless::ReasoningLevel,
        plan_interview::{PlanQuestion, PlanQuestionKind, QuestionSource},
    };
    let (mut app, dir) = if existing {
        reviewed_feature_fixture(quick)
    } else {
        fixture(MockTmuxOps::new())
    };
    let workdir = app.store.projects[0].repo.clone();
    std::fs::write(
        workdir.join("README.md"),
        "Repository invariant: parse without panics.",
    )
    .unwrap();
    let reference = workdir.join("reference.md");
    std::fs::write(
        &reference,
        "Reference requirement: preserve Unicode boundaries.",
    )
    .unwrap();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.brief = "Check the Unicode parser design".into();
        state.questions = vec![PlanQuestion {
            id: "compatibility".into(),
            text: "Must existing callers work?".into(),
            kind: PlanQuestionKind::FreeText,
            source: QuestionSource::Builtin,
            optional: false,
        }];
        state.answers = vec![Some("Preserve all existing callers".into())];
        state.attached_docs = vec![reference];
        state.ai_harness = Some(Some(AgentKind::Codex));
        state.expert_model_pick = Some(AiModelPickState::new(
            &AgentKind::Codex,
            vec![
                ModelPickRow::Preset("test-model".into()),
                ModelPickRow::Custom,
            ],
            1,
            "my-review-model".into(),
            Some(ReasoningLevel::High),
        ));
    }
    app.model_analysis_work.discover = discover_only_current_harness;
    app.model_analysis_work.runner = expert_answer;
    (app, dir)
}

fn expert_answer(input: &RunInput) -> Result<String> {
    assert_eq!(input.context.get("task_phase"), Some("Expert plan review"));
    let task = input.context.get("task_context").unwrap();
    for expected in [
        "draft_plan",
        "Check the Unicode parser design",
        "Preserve all existing callers",
        "Repository invariant",
        "Reference requirement",
        "missing requirements, risks, correctness",
    ] {
        assert!(task.contains(expected), "missing {expected}");
    }
    answer(input)
}

fn advice_key(app: &mut App, code: crossterm::event::KeyCode) {
    crate::handlers::handle_key(
        app,
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE),
        20,
    )
    .unwrap();
}

#[test]
fn expert_advice_at_new_existing_quick_and_todo_plans_returns_to_unchanged_picker() {
    use crate::headless::ReasoningLevel;
    for (existing, quick, todo) in [
        (false, false, false),
        (true, false, false),
        (true, true, false),
        (false, false, true),
        (true, false, true),
    ] {
        let (mut app, _dir) = expert_fixture(existing, quick);
        if todo {
            let host = app.store.projects[0]
                .features
                .first()
                .map_or("source".into(), |f| f.id.clone());
            attach_todo(&mut app, &host);
        }
        advice_key(&mut app, crossterm::event::KeyCode::Char('m'));
        poll(&mut app);
        assert!(
            matches!(&app.mode, AppMode::ModelAnalysis(s) if s.scope() == AdviceScope::ExpertPlanReview
            && s.target.launch_path() == LaunchPath::Headless && matches!(s.status, Status::Ready(_)))
        );
        assert!(
            app.apply_model_analysis()
                .unwrap_err()
                .to_string()
                .contains("view-only")
        );
        advice_key(&mut app, crossterm::event::KeyCode::Enter);
        assert!(matches!(app.mode, AppMode::ModelAnalysis(_)));
        advice_key(&mut app, crossterm::event::KeyCode::Esc);
        let AppMode::PlanInterview(state) = &app.mode else {
            panic!()
        };
        let pick = state.expert_model_pick.as_ref().unwrap();
        assert_eq!(pick.selected, 1);
        assert_eq!(pick.custom_input, "my-review-model");
        assert_eq!(pick.reasoning, Some(ReasoningLevel::High));
        assert!(state.expert_model.is_none());
        assert_eq!(state.phase, PlanInterviewPhase::Review);
        assert!(
            state
                .pending_launch
                .as_ref()
                .is_none_or(|p| p.model_selection.is_none())
        );
        assert!(app.plan_interview_critique_bg.is_none());
        assert!(app.model_analysis_work.launch_args.is_none());
    }
}

#[test]
fn expert_advice_uses_resolved_reviewer_instead_of_implementation_harness() {
    let (mut app, _dir) = expert_fixture(false, false);
    app.store.available_harnesses = vec![AgentKind::Claude, AgentKind::Codex];
    app.model_analysis_work.discover = discover_claude;
    app.model_analysis_work.now = || "2026-09-30T12:00:00Z".parse().unwrap();
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.ai_harness = Some(Some(AgentKind::Claude));
    }
    app.open_model_analysis().unwrap();
    poll(&mut app);
    let AppMode::ModelAnalysis(state) = &app.mode else {
        panic!()
    };
    let Status::Ready(choices) = &state.status else {
        panic!("{:?}", state.status)
    };
    assert!(
        choices
            .iter()
            .all(|r| *r.choice.harness() == AgentKind::Claude)
    );
    app.cancel_model_analysis();
    assert!(
        matches!(&app.mode, AppMode::PlanInterview(s) if s.pending_launch.as_ref().unwrap().agent == AgentKind::Codex)
    );
}

#[test]
fn expert_advice_rejects_changed_interview_reviewer_and_destination() {
    for change in 0..8 {
        let (mut app, _dir) = expert_fixture(false, false);
        app.open_model_analysis().unwrap();
        if let AppMode::ModelAnalysis(state) = &mut app.mode {
            let AppMode::PlanInterview(interview) = state.origin.as_mut() else {
                panic!()
            };
            match change {
                0 => interview.brief.push_str("Changed"),
                1 => interview.answers[0] = Some("Different contract".into()),
                2 => interview.questions[0].text.push_str("Changed"),
                3 => interview.attached_docs.clear(),
                4 => interview.ai_harness = Some(Some(AgentKind::Claude)),
                5 => {
                    interview.expert_model_pick = None;
                }
                6 => interview.apply_synthesis("Different plan".into()),
                _ => {
                    app.store.projects[0].id = "another-project".into();
                }
            }
        }
        poll(&mut app);
        assert!(
            matches!(&app.mode, AppMode::ModelAnalysis(s) if matches!(s.status, Status::Error(_))),
            "change {change}"
        );
        assert!(app.plan_interview_critique_bg.is_none());
    }
}

#[test]
fn expert_advice_rejects_reference_and_repository_changes_during_analysis() {
    for change in 0..3 {
        let (mut app, _dir) = expert_fixture(false, false);
        app.model_analysis_work.runner = match change {
            0 => |input| {
                std::fs::write(input.workdir.join("reference.md"), "Changed reference")?;
                answer(input)
            },
            1 => |input| {
                std::fs::write(input.workdir.join("README.md"), "Changed repository")?;
                answer(input)
            },
            _ => |input| {
                std::fs::remove_file(input.workdir.join("reference.md"))?;
                answer(input)
            },
        };
        app.open_model_analysis().unwrap();
        poll(&mut app);
        assert!(
            matches!(&app.mode, AppMode::ModelAnalysis(s) if matches!(s.status, Status::Error(_))),
            "change {change}"
        );
    }
}

#[test]
fn expert_advice_hashes_reference_content_beyond_its_displayed_excerpt() {
    let (mut app, _dir) = expert_fixture(false, false);
    let reference = app.store.projects[0].repo.join("reference.md");
    std::fs::write(
        &reference,
        format!(
            "{}original",
            "x".repeat(crate::plan_interview::MODEL_INPUT_FIELD_MAX_CHARS)
        ),
    )
    .unwrap();
    app.model_analysis_work.runner = |input| {
        std::fs::write(
            input.workdir.join("reference.md"),
            format!(
                "{}changed",
                "x".repeat(crate::plan_interview::MODEL_INPUT_FIELD_MAX_CHARS)
            ),
        )?;
        answer(input)
    };
    app.open_model_analysis().unwrap();
    poll(&mut app);
    assert!(
        matches!(&app.mode, AppMode::ModelAnalysis(s) if matches!(&s.status, Status::Error(e) if e.contains("task context changed")))
    );
}

#[test]
fn expert_custom_model_typing_keeps_m_as_text_and_cancel_restores_picker() {
    let (mut app, _dir) = expert_fixture(false, false);
    if let AppMode::PlanInterview(state) = &mut app.mode {
        state.expert_model_pick.as_mut().unwrap().editing_custom = true;
    }
    advice_key(&mut app, crossterm::event::KeyCode::Char('m'));
    assert!(
        matches!(&app.mode, AppMode::PlanInterview(s) if s.expert_model_pick.as_ref().unwrap().custom_input == "my-review-modelm")
    );
    advice_key(&mut app, crossterm::event::KeyCode::Esc);
    advice_key(&mut app, crossterm::event::KeyCode::Char('m'));
    app.cancel_model_analysis();
    assert!(!app.model_analysis_work.pending());
    assert!(
        matches!(&app.mode, AppMode::PlanInterview(s) if s.expert_model_pick.is_some() && s.phase == PlanInterviewPhase::Review)
    );
    assert!(app.plan_interview_critique_bg.is_none());
}
