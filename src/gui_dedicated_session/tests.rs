//! Dedicated triage session: private git repository, mocked tmux (any launch
//! call would panic the mock, so every test also proves nothing is started) and
//! no GitHub access at all.
use super::*;
use crate::github::PrRef;
use crate::gui_contract::GuiErrorKind;
use crate::project::ProjectStatus;
use crate::traits::MockTmuxOps;

fn pr(number: u32, head: &str) -> PrRef {
    PrRef {
        number,
        head_sha: head.into(),
        url: format!("https://example.test/pull/{number}"),
        owner: "fixture".into(),
        repo: "repo".into(),
        head_ref: "feature".into(),
    }
}

fn quiet_tmux() -> MockTmuxOps {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_check_harness_available().returning(|_| Ok(()));
    tmux.expect_session_exists().return_const(false);
    tmux.expect_window_exists().return_const(false);
    tmux.expect_list_panes().returning(Vec::new);
    tmux
}

fn fixture() -> (
    tempfile::TempDir,
    GuiHandle,
    PrDestinationWorkflow,
    PrIdentity,
) {
    let (dir, mut gui, target) = crate::gui_diff::tests::fixture();
    let app = gui.app_for_workflow();
    app.config.low_memory_warn_mb = 0;
    app.config.max_concurrent_agents = 0;
    let id = crate::gui_pr_workspace::begin(&mut gui, target).unwrap();
    gui.pr_workspace_context.as_mut().unwrap().pr = Some(pr(42, "head-1"));
    let workflow = PrDestinationWorkflow::Workspace { workflow_id: id };
    let identity = catalog(&mut gui, &workflow).unwrap().identity;
    (dir, gui, workflow, identity)
}

fn choice(agent: AgentKind, name: Option<&str>) -> PrDestinationChoice {
    PrDestinationChoice::DedicatedSession {
        agent,
        name: name.map(Into::into),
    }
}

fn request(
    workflow: &PrDestinationWorkflow,
    identity: &PrIdentity,
    choice: PrDestinationChoice,
) -> PrDestinationRequest {
    PrDestinationRequest {
        workflow: workflow.clone(),
        identity: identity.clone(),
        choice,
    }
}

fn prepare_for(
    gui: &mut GuiHandle,
    req: &PrDestinationRequest,
    revision: &str,
    approved: bool,
) -> GuiResult<PrDestinationPrepared> {
    prepare(
        gui,
        PrDestinationPrepareRequest {
            workflow: req.workflow.clone(),
            identity: req.identity.clone(),
            choice: req.choice.clone(),
            preview_revision: revision.into(),
            approved,
        },
        "op-test",
    )
}

fn sessions(gui: &mut GuiHandle) -> Vec<FeatureSession> {
    gui.app_for_workflow().store.projects[0].features[0]
        .sessions
        .clone()
}

/// A session created behind the GUI's back (the TUI, another window).
fn add_elsewhere(gui: &mut GuiHandle, kind: SessionKind, label: &str) -> String {
    let app = gui.app_for_workflow();
    let session = app.store.projects[0].features[0].add_session_named(kind, label.into());
    let id = session.id.clone();
    app.db.as_ref().unwrap().save_store(&app.store).unwrap();
    app.store_version = None;
    id
}

fn error(result: GuiResult<impl std::fmt::Debug>) -> GuiError {
    result.unwrap_err()
}

#[test]
fn catalog_lists_harnesses_the_default_name_and_the_pr_identity() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let view = catalog(&mut gui, &workflow).unwrap();
    assert_eq!(view.default_name, "PR Triage");
    assert_eq!(view.feature_name, "Feature");
    assert_eq!(view.identity, identity);
    assert_eq!(
        (view.identity.pr_number, view.identity.head_sha.as_str()),
        (42, "head-1")
    );
    assert!(
        view.agents
            .iter()
            .any(|a| a.agent == AgentKind::Claude && a.unavailable.is_none())
    );
    assert_eq!(view.preferred, AgentKind::Claude);
}

#[test]
fn an_unavailable_harness_is_listed_with_its_reason_and_refused() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_check_harness_available().returning(|agent| {
        if *agent == AgentKind::Pi {
            anyhow::bail!("pi is not installed")
        }
        Ok(())
    });
    gui.app_for_workflow().tmux = Box::new(tmux);
    let view = catalog(&mut gui, &workflow).unwrap();
    let pi = view.agents.iter().find(|a| a.agent == AgentKind::Pi);
    assert_eq!(
        pi.and_then(|a| a.unavailable.as_deref()),
        Some("pi is not installed")
    );
    let req = request(&workflow, &identity, choice(AgentKind::Pi, None));
    assert!(
        error(preview(&mut gui, &req))
            .message
            .contains("pi is not installed")
    );
    assert!(sessions(&mut gui).is_empty());
}

#[test]
fn preview_is_read_only_and_names_the_consequences() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    assert!(shown.creates_session && !shown.reuses_existing && shown.stopped);
    assert_eq!(shown.label, "PR Triage");
    assert!(shown.resource_warning.is_none());
    let text = shown.consequences.join(" ");
    assert!(text.contains("created stopped"), "{text}");
    assert!(text.contains("Start it from the session tab"), "{text}");
    assert!(text.contains("Nothing is posted to GitHub"), "{text}");
    assert!(sessions(&mut gui).is_empty());
    // Repeating it is stable.
    assert_eq!(preview(&mut gui, &req).unwrap().revision, shown.revision);
}

#[test]
fn prepare_creates_the_session_stopped_without_starting_or_sending_anything() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    let made = prepare_for(&mut gui, &req, &shown.revision, false).unwrap();
    assert_eq!(made.status, PrepareStatus::Created);
    assert!(made.stopped);
    let target = made.created_target.clone().unwrap();
    let rows = sessions(&mut gui);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, target.session_id);
    assert_eq!(rows[0].label, "PR Triage");
    assert_eq!(rows[0].kind, SessionKind::Claude);
    assert!(rows[0].stopped);
    assert!(made.message.contains("stopped"));
    // Persisted, not only in memory.
    let stored = gui
        .app_for_workflow()
        .db
        .as_ref()
        .unwrap()
        .load_store()
        .unwrap();
    assert_eq!(stored.projects[0].features[0].sessions.len(), 1);
    // The next preview now reuses it.
    let again = preview(&mut gui, &req).unwrap();
    assert!(again.reuses_existing && !again.creates_session);
    assert_eq!(
        again.existing_target.map(|t| t.session_id),
        made.created_target.map(|t| t.session_id)
    );
}

#[test]
fn the_created_session_is_a_fix_target_with_the_returned_revision() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let req = request(&workflow, &identity, choice(AgentKind::Codex, None));
    let shown = preview(&mut gui, &req).unwrap();
    let made = prepare_for(&mut gui, &req, &shown.revision, false).unwrap();
    let PrDestinationWorkflow::Workspace { workflow_id } = &workflow else {
        unreachable!()
    };
    let rows = crate::gui_pr_workspace::fix_targets(&mut gui, workflow_id).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        Some(rows[0].target.session_id.clone()),
        made.created_target.map(|t| t.session_id)
    );
    assert_eq!(
        Some(rows[0].destination_revision.clone()),
        made.destination_revision
    );
    assert!(rows[0].stopped);
    assert_eq!(
        rows[0].destination_kind,
        crate::gui_pr_workspace::DestinationKind::OpenedFeature
    );
    assert_eq!(rows[0].harness, AgentKind::Codex);
}

#[test]
fn a_custom_name_is_the_exact_identity_and_blank_means_the_default() {
    let (_dir, mut gui, workflow, identity) = fixture();
    add_elsewhere(&mut gui, SessionKind::Claude, "PR Triage");
    let named = request(
        &workflow,
        &identity,
        choice(AgentKind::Claude, Some("  Hotfix triage ")),
    );
    let shown = preview(&mut gui, &named).unwrap();
    // The default-labelled session does not stand in for the custom name.
    assert!(shown.creates_session && shown.label == "Hotfix triage");
    prepare_for(&mut gui, &named, &shown.revision, false).unwrap();
    assert_eq!(sessions(&mut gui).len(), 2);
    let blank = request(&workflow, &identity, choice(AgentKind::Claude, Some("   ")));
    let shown = preview(&mut gui, &blank).unwrap();
    assert!(shown.reuses_existing && shown.label == "PR Triage");
    for bad in ["x".repeat(61), "two\nlines".to_string()] {
        let req = request(&workflow, &identity, choice(AgentKind::Claude, Some(&bad)));
        assert_eq!(error(preview(&mut gui, &req)).kind, GuiErrorKind::Conflict);
    }
}

#[test]
fn an_existing_labelled_session_is_reused_without_creating_or_asking() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let existing = add_elsewhere(&mut gui, SessionKind::Claude, "PR Triage");
    // Even a tripped resource gate does not apply: nothing is created.
    gui.app_for_workflow().config.low_memory_warn_mb = u64::MAX;
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    assert!(shown.reuses_existing && !shown.creates_session);
    assert!(shown.resource_warning.is_none());
    assert!(shown.consequences.join(" ").contains("Reuses the existing"));
    let made = prepare_for(&mut gui, &req, &shown.revision, false).unwrap();
    assert_eq!(made.status, PrepareStatus::Reused);
    assert_eq!(made.created_target.unwrap().session_id, existing);
    assert_eq!(sessions(&mut gui).len(), 1);
}

#[test]
fn the_legacy_default_label_is_reused_but_never_for_a_custom_name() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let legacy = add_elsewhere(&mut gui, SessionKind::Claude, "PR Review");
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    assert_eq!(
        preview(&mut gui, &req)
            .unwrap()
            .existing_target
            .unwrap()
            .session_id,
        legacy
    );
    let custom = request(
        &workflow,
        &identity,
        choice(AgentKind::Claude, Some("PR Review 2")),
    );
    assert!(preview(&mut gui, &custom).unwrap().creates_session);
}

#[test]
fn a_label_used_by_another_harness_is_a_conflict_not_a_reuse() {
    let (_dir, mut gui, workflow, identity) = fixture();
    add_elsewhere(&mut gui, SessionKind::Claude, "PR Triage");
    let req = request(&workflow, &identity, choice(AgentKind::Codex, None));
    let failure = error(preview(&mut gui, &req));
    assert_eq!(failure.kind, GuiErrorKind::Conflict);
    assert!(
        failure.message.contains("already runs Claude"),
        "{}",
        failure.message
    );
    let failure = error(prepare_for(&mut gui, &req, "anything", true));
    assert!(failure.message.contains("already runs Claude"));
    assert_eq!(sessions(&mut gui).len(), 1);
}

#[test]
fn two_sessions_with_the_label_are_refused_instead_of_guessed_between() {
    let (_dir, mut gui, workflow, identity) = fixture();
    add_elsewhere(&mut gui, SessionKind::Claude, "PR Triage");
    add_elsewhere(&mut gui, SessionKind::Claude, "PR Triage");
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let failure = error(preview(&mut gui, &req));
    assert_eq!(failure.kind, GuiErrorKind::Conflict);
    assert!(failure.message.contains("guess"), "{}", failure.message);
    assert_eq!(sessions(&mut gui).len(), 2);
}

#[test]
fn a_session_created_between_preview_and_prepare_makes_the_preview_stale() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    add_elsewhere(&mut gui, SessionKind::Claude, "PR Triage");
    let failure = error(prepare_for(&mut gui, &req, &shown.revision, true));
    assert_eq!(failure.kind, GuiErrorKind::Conflict);
    assert!(
        failure.message.contains("changed since the preview"),
        "{}",
        failure.message
    );
    // Nothing was added on top of the TUI's session.
    assert_eq!(sessions(&mut gui).len(), 1);
    // Previewing again shows the reuse, and that confirms.
    let fresh = preview(&mut gui, &req).unwrap();
    assert!(fresh.reuses_existing);
    assert_eq!(
        prepare_for(&mut gui, &req, &fresh.revision, false)
            .unwrap()
            .status,
        PrepareStatus::Reused
    );
}

#[test]
fn a_changed_feature_or_a_different_choice_makes_the_preview_stale() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    // Another harness or name is a different preview.
    for other in [
        choice(AgentKind::Codex, None),
        choice(AgentKind::Claude, Some("Other")),
    ] {
        let swapped = request(&workflow, &identity, other);
        assert!(prepare_for(&mut gui, &swapped, &shown.revision, true).is_err());
    }
    // The feature's recorded branch changes.
    gui.app_for_workflow().store.projects[0].features[0].branch = "renamed".into();
    assert!(prepare_for(&mut gui, &req, &shown.revision, true).is_err());
    assert!(sessions(&mut gui).is_empty());
}

#[test]
fn pr_identity_must_be_echoed_exactly() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    // The PR head moves.
    gui.pr_workspace_context.as_mut().unwrap().pr = Some(pr(42, "head-2"));
    for stale in [
        preview(&mut gui, &req).map(|_| ()),
        prepare_for(&mut gui, &req, &shown.revision, true).map(|_| ()),
    ] {
        let failure = stale.unwrap_err();
        assert_eq!(failure.kind, GuiErrorKind::Conflict);
        assert!(
            failure.message.contains("new commits"),
            "{}",
            failure.message
        );
    }
    // A different PR number.
    gui.pr_workspace_context.as_mut().unwrap().pr = Some(pr(43, "head-1"));
    let failure = error(preview(&mut gui, &req));
    assert!(
        failure.message.contains("#42 to #43"),
        "{}",
        failure.message
    );
    // The caller claims a comment/finding set the workflow does not hold.
    gui.pr_workspace_context.as_mut().unwrap().pr = Some(pr(42, "head-1"));
    let mut claimed = req.clone();
    claimed.identity.set_digest = Some("not-the-live-set".into());
    let failure = error(preview(&mut gui, &claimed));
    assert!(
        failure.message.contains("comments or AI findings"),
        "{}",
        failure.message
    );
    assert!(sessions(&mut gui).is_empty());
    // And the original identity works again once restored.
    assert!(preview(&mut gui, &req).is_ok());
}

#[test]
fn the_resource_gate_needs_explicit_approval_and_then_creates() {
    let (_dir, mut gui, workflow, identity) = fixture();
    gui.app_for_workflow().config.low_memory_warn_mb = u64::MAX;
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    assert!(shown.resource_warning.is_some());
    let failure = error(prepare_for(&mut gui, &req, &shown.revision, false));
    assert_eq!(failure.kind, GuiErrorKind::NeedsApproval);
    assert!(sessions(&mut gui).is_empty());
    let made = prepare_for(&mut gui, &req, &shown.revision, true).unwrap();
    assert_eq!(made.status, PrepareStatus::Created);
    assert_eq!(sessions(&mut gui).len(), 1);
}

#[test]
fn the_gate_tripping_after_the_preview_makes_it_stale_rather_than_unasked() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    gui.app_for_workflow().config.low_memory_warn_mb = u64::MAX;
    assert!(prepare_for(&mut gui, &req, &shown.revision, true).is_err());
    assert!(sessions(&mut gui).is_empty());
}

#[test]
fn a_feature_sharing_a_tmux_session_is_refused() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let app = gui.app_for_workflow();
    let mut other = app.store.projects[0].features[0].clone();
    other.id = "other-feature".into();
    other.name = "other".into();
    app.store.projects[0].features.push(other);
    app.db.as_ref().unwrap().save_store(&app.store).unwrap();
    app.store_version = None;
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let failure = error(preview(&mut gui, &req));
    assert_eq!(failure.kind, GuiErrorKind::Conflict);
    assert!(failure.message.contains("shares a tmux session"));
}

#[test]
fn a_deleted_feature_or_closed_workflow_is_refused() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let app = gui.app_for_workflow();
    app.store.projects[0].features.clear();
    app.db.as_ref().unwrap().save_store(&app.store).unwrap();
    app.store_version = None;
    assert!(preview(&mut gui, &req).is_err());
    let closed = PrDestinationWorkflow::Workspace {
        workflow_id: "no-such-workflow".into(),
    };
    let req = request(&closed, &identity, choice(AgentKind::Claude, None));
    assert_eq!(error(preview(&mut gui, &req)).kind, GuiErrorKind::Conflict);
}

#[test]
fn another_writers_change_is_adopted_not_overwritten() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    // Another process saves after the preview; the bind refreshes first, so the
    // new row is added on top of its write instead of replacing it.
    let app = gui.app_for_workflow();
    let mut other = app.store.clone();
    other.projects[0].name = "Renamed elsewhere".into();
    app.db.as_ref().unwrap().save_store(&other).unwrap();
    let made = prepare_for(&mut gui, &req, &shown.revision, true).unwrap();
    assert_eq!(made.status, PrepareStatus::Created);
    let stored = gui
        .app_for_workflow()
        .db
        .as_ref()
        .unwrap()
        .load_store()
        .unwrap();
    assert_eq!(stored.projects[0].name, "Renamed elsewhere");
    assert_eq!(stored.projects[0].features[0].sessions.len(), 1);
}

#[test]
fn a_stopped_feature_stays_stopped() {
    let (_dir, mut gui, workflow, identity) = fixture();
    assert_eq!(
        gui.app_for_workflow().store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    prepare_for(&mut gui, &req, &shown.revision, false).unwrap();
    assert_eq!(
        gui.app_for_workflow().store.projects[0].features[0].status,
        ProjectStatus::Stopped
    );
}

#[test]
fn a_running_feature_gets_a_stopped_row_and_no_new_window() {
    let (_dir, mut gui, workflow, identity) = fixture();
    // Any window creation on this mock would panic.
    gui.app_for_workflow().tmux = Box::new(quiet_tmux());
    gui.app_for_workflow().store.projects[0].features[0].status = ProjectStatus::Idle;
    let req = request(&workflow, &identity, choice(AgentKind::Opencode, None));
    let shown = preview(&mut gui, &req).unwrap();
    let made = prepare_for(&mut gui, &req, &shown.revision, false).unwrap();
    assert!(made.stopped);
    let rows = sessions(&mut gui);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].stopped && rows[0].kind == SessionKind::Opencode);
}

#[test]
fn a_workspace_ai_result_change_is_a_conflict() {
    let (dir, mut gui, workflow, _) = fixture();
    let mut worktree = crate::traits::MockWorktreeOps::new();
    worktree
        .expect_repo_root()
        .returning(crate::worktree::WorktreeManager::repo_root);
    let app = gui.app_for_workflow();
    app.worktree = std::sync::Arc::new(worktree);
    app.config.global_review_memory_path =
        Some(dir.path().join("absent-memory.md").display().to_string());
    let PrDestinationWorkflow::Workspace { workflow_id } = &workflow else {
        unreachable!()
    };
    let workdir = gui.pr_workspace_context.as_ref().unwrap().workdir.clone();
    gui.app_for_workflow()
        .open_ai_review_for_pr(workdir, pr(42, "head-1"));
    gui.pr_workspace_context.as_mut().unwrap().ai_active = true;
    let identity = catalog(&mut gui, &workflow).unwrap().identity;
    assert!(identity.set_digest.is_some(), "AI result revision is bound");
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    let crate::app::AppMode::AiReview(state) = &mut gui.app_for_workflow().mode else {
        panic!("AI mode")
    };
    state.summary = Some("A different result".into());
    let failure = error(prepare_for(&mut gui, &req, &shown.revision, true));
    assert!(
        failure.message.contains("comments or AI findings"),
        "{}",
        failure.message
    );
    assert!(sessions(&mut gui).is_empty());
    let _ = workflow_id;
}

#[test]
fn one_operation_per_workflow_and_a_duplicate_gets_the_running_one() {
    let a = PrDestinationWorkflow::Triage {
        workflow_id: "op-a".into(),
    };
    let b = PrDestinationWorkflow::Workspace {
        workflow_id: "op-a".into(),
    };
    let OperationStart::Started(first) = begin_operation(&a) else {
        panic!("first claims the slot")
    };
    let OperationStart::Existing(duplicate) = begin_operation(&a) else {
        panic!("duplicate must not start")
    };
    assert_eq!(duplicate.status, PrepareStatus::InProgress);
    assert_eq!(duplicate.operation_id, first.operation_id);
    assert!(duplicate.created_target.is_none());
    // A different workflow (even with the same id text) is independent.
    let OperationStart::Started(second) = begin_operation(&b) else {
        panic!("other workflow is independent")
    };
    drop(second);
    let id = first.operation_id.clone();
    drop(first);
    let OperationStart::Started(again) = begin_operation(&a) else {
        panic!("the slot is released")
    };
    assert_ne!(again.operation_id, id);
}

#[test]
fn a_panicking_operation_releases_its_slot() {
    let flow = PrDestinationWorkflow::Triage {
        workflow_id: "op-panic".into(),
    };
    let flow2 = flow.clone();
    let result = std::thread::spawn(move || {
        let OperationStart::Started(_guard) = begin_operation(&flow2) else {
            panic!("claims")
        };
        panic!("boom");
    })
    .join();
    assert!(result.is_err());
    assert!(matches!(begin_operation(&flow), OperationStart::Started(_)));
}

fn settings_path(gui: &mut GuiHandle) -> std::path::PathBuf {
    gui.app_for_workflow().store.projects[0].features[0]
        .workdir
        .join(".claude")
        .join("settings.local.json")
}

#[test]
fn a_successful_prepare_writes_hooks_and_the_review_instructions_after_the_save() {
    let (_dir, mut gui, workflow, identity) = fixture();
    {
        let feature = &mut gui.app_for_workflow().store.projects[0].features[0];
        feature.review = true;
        feature.collapsed = true;
    }
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    assert!(
        shown
            .consequences
            .iter()
            .any(|line| line.contains("review enabled")),
        "{:?}",
        shown.consequences
    );
    assert!(!settings_path(&mut gui).exists(), "preview writes nothing");
    prepare_for(&mut gui, &req, &shown.revision, false).unwrap();
    assert!(settings_path(&mut gui).exists());
    let workdir = gui.app_for_workflow().store.projects[0].features[0]
        .workdir
        .clone();
    let claude_md = std::fs::read_to_string(workdir.join("CLAUDE.local.md")).unwrap();
    assert!(claude_md.contains("REVIEW MODE"), "{claude_md}");
    let stored = gui
        .app_for_workflow()
        .db
        .as_ref()
        .unwrap()
        .load_store()
        .unwrap();
    assert!(!stored.projects[0].features[0].collapsed);
}

#[test]
fn a_feature_without_review_gets_no_review_instructions() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    prepare_for(&mut gui, &req, &shown.revision, false).unwrap();
    let workdir = gui.app_for_workflow().store.projects[0].features[0]
        .workdir
        .clone();
    let md = std::fs::read_to_string(workdir.join("CLAUDE.local.md")).unwrap_or_default();
    assert!(!md.contains("REVIEW MODE"));
}

/// A tmux mock whose harness probe runs `hook` once armed, i.e. after the
/// preview, between prepare's bind and its save.
fn tmux_with_hook(
    armed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    hook: impl Fn() + Send + 'static,
) -> MockTmuxOps {
    let mut tmux = MockTmuxOps::new();
    tmux.expect_check_harness_available().returning(move |_| {
        if armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            hook();
        }
        Ok(())
    });
    tmux.expect_session_exists().return_const(false);
    tmux.expect_window_exists().return_const(false);
    tmux.expect_list_panes().returning(Vec::new);
    tmux
}

#[test]
fn an_unrelated_concurrent_write_is_reapplied_not_a_conflict() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let db_path = gui.app_for_workflow().db.as_ref().unwrap().path.clone();
    let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    gui.app_for_workflow().tmux = Box::new(tmux_with_hook(armed.clone(), move || {
        let db = crate::db::AmfDb::open(&db_path).unwrap();
        let mut other = db.load_store().unwrap();
        other.projects[0].name = "Renamed mid-prepare".into();
        db.save_store(&other).unwrap();
    }));
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let made = prepare_for(&mut gui, &req, &shown.revision, true).unwrap();
    assert_eq!(made.status, PrepareStatus::Created);
    let stored = gui
        .app_for_workflow()
        .db
        .as_ref()
        .unwrap()
        .load_store()
        .unwrap();
    assert_eq!(stored.projects[0].name, "Renamed mid-prepare");
    assert_eq!(stored.projects[0].features[0].sessions.len(), 1);
    assert!(settings_path(&mut gui).exists());
}

#[test]
fn a_feature_deleted_mid_prepare_leaves_no_row_and_no_hook_files() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let db_path = gui.app_for_workflow().db.as_ref().unwrap().path.clone();
    let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    gui.app_for_workflow().tmux = Box::new(tmux_with_hook(armed.clone(), move || {
        let db = crate::db::AmfDb::open(&db_path).unwrap();
        let mut other = db.load_store().unwrap();
        other.projects[0].features.clear();
        db.save_store(&other).unwrap();
    }));
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let failure = error(prepare_for(&mut gui, &req, &shown.revision, true));
    assert_eq!(failure.kind, GuiErrorKind::NotFound, "{}", failure.message);
    assert!(gui.app_for_workflow().store.projects[0].features.is_empty());
}

#[test]
fn a_failed_save_rolls_the_row_back_and_leaves_no_hook_files() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let db_path = gui.app_for_workflow().db.as_ref().unwrap().path.clone();
    let holder: std::sync::Arc<std::sync::Mutex<Option<rusqlite::Connection>>> = Default::default();
    let held = holder.clone();
    let armed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    gui.app_for_workflow().tmux = Box::new(tmux_with_hook(armed.clone(), move || {
        // Hold the write lock so the save fails with "database is locked".
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        *held.lock().unwrap() = Some(conn);
    }));
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let shown = preview(&mut gui, &req).unwrap();
    armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let failure = error(prepare_for(&mut gui, &req, &shown.revision, true));
    assert_ne!(failure.kind, GuiErrorKind::Conflict, "{}", failure.message);
    assert!(holder.lock().unwrap().is_some(), "the lock was taken");
    assert!(sessions(&mut gui).is_empty(), "the in-memory row is undone");
    assert!(!settings_path(&mut gui).exists());
    holder.lock().unwrap().take();
}

#[test]
fn a_same_named_session_of_any_kind_blocks_the_name_case_insensitively() {
    let (_dir, mut gui, workflow, identity) = fixture();
    add_elsewhere(&mut gui, SessionKind::Terminal, "pr triage");
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let failure = error(preview(&mut gui, &req));
    assert_eq!(failure.kind, GuiErrorKind::Conflict);
    assert!(
        failure.message.contains("choose another session name"),
        "{}",
        failure.message
    );
    assert_eq!(sessions(&mut gui).len(), 1);
}

#[test]
fn the_slot_is_released_after_a_failed_prepare() {
    let (_dir, mut gui, workflow, identity) = fixture();
    let req = request(&workflow, &identity, choice(AgentKind::Claude, None));
    let OperationStart::Started(guard) = begin_operation(&workflow) else {
        panic!("claims")
    };
    let result = prepare_for(&mut gui, &req, "stale-revision", false);
    assert!(result.is_err());
    assert!(matches!(
        begin_operation(&workflow),
        OperationStart::Existing(_)
    ));
    drop(guard);
    assert!(matches!(
        begin_operation(&workflow),
        OperationStart::Started(_)
    ));
}
