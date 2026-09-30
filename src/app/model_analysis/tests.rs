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
        result: Ok(Outcome::Advice(vec![])),
    })
    .unwrap();
    assert!(!app.poll_model_analysis());
    assert!(app.model_analysis_work.pending());
    tx.send(Completion {
        generation,
        target,
        allowed: vec![AgentKind::Codex],
        result: Ok(Outcome::Advice(vec![])),
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
    app.retry_model_analysis();
    poll(&mut app);
    assert_eq!(app.store.projects[0].features.len(), 1);
    assert_eq!(app.store.projects[0].features[0].id, id);
    assert_eq!(count.load(Ordering::Relaxed), 2);
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
