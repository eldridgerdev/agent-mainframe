use super::*;

fn setup() -> (
    tempfile::TempDir,
    AmfDb,
    ConsultationOrigin,
    ConsultationRequest,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = AmfDb::open(&dir.path().join("test.db")).unwrap();
    let origin = ConsultationOrigin {
        project_id: "project".into(),
        feature_id: "feature".into(),
        session_id: "session".into(),
        launch_generation: "generation".into(),
        provider_session_id: None,
        workdir: dir.path().to_path_buf(),
        repository_identity: "repo".into(),
        tmux_server_identity: "server".into(),
        tmux_session_id: "$1".into(),
        tmux_window_id: "@1".into(),
        tmux_pane_id: "%1".into(),
    };
    let request = ConsultationRequest {
        question: "Why does this check fail?".into(),
        acceptance_criteria: vec!["Explain the failure".into()],
        attempted_fixes: vec![],
        evidence: vec![],
        profile: ExpertProfile {
            harness: AgentKind::Claude,
            binary: "claude".into(),
            model: "configured-expert".into(),
            policy: HeadlessExecutionPolicy::PacketOnly,
            limits: Default::default(),
        },
        prompt_id: "expert_assist.consult".into(),
        template_source: "builtin".into(),
        rendered_prompt: "Question and evidence".into(),
    };
    (dir, db, origin, request)
}

fn owner() -> ConsultationOwner {
    ConsultationOwner::current().unwrap()
}

fn outcome(status: HeadlessJobStatus) -> ConsultationOutcome {
    ConsultationOutcome {
        status,
        response: (status == HeadlessJobStatus::Completed).then(|| "advice".into()),
        error: (status != HeadlessJobStatus::Completed).then(|| "failed work".into()),
        usage: HeadlessUsage {
            input_tokens: Some(12),
            ..Default::default()
        },
        usage_complete: false,
        elapsed_millis: 1234,
    }
}

fn current(db: &AmfDb, id: &str) -> Consultation {
    db.expert_consultation(id).unwrap().unwrap()
}

fn completed(db: &AmfDb, origin: &ConsultationOrigin, request: &ConsultationRequest) -> String {
    let id = db.create_expert_consultation(origin, request).unwrap();
    let owner = owner();
    let attempt = db.begin_expert_attempt(&id, 0, &owner).unwrap().unwrap();
    assert!(
        db.finish_expert_attempt(&id, &attempt, &owner, outcome(HeadlessJobStatus::Completed))
            .unwrap()
    );
    id
}

#[test]
fn request_revisions_are_immutable_and_stale_edits_do_not_overwrite() {
    let (_dir, db, origin, mut request) = setup();
    let original = request.question.clone();
    let id = db.create_expert_consultation(&origin, &request).unwrap();
    request.question = "new question".into();
    assert!(db.revise_expert_request(&id, 0, &request).unwrap());
    assert!(!db.revise_expert_request(&id, 0, &request).unwrap());
    assert_eq!(db.expert_request(&id, 1).unwrap().question, original);
    assert_eq!(db.expert_request(&id, 2).unwrap().question, "new question");
    assert_eq!(current(&db, &id).request_revision, 2);
}

#[test]
fn attempts_preserve_failed_cost_and_reject_wrong_or_duplicate_completion() {
    let (_dir, db, origin, request) = setup();
    let id = db.create_expert_consultation(&origin, &request).unwrap();
    let owner = owner();
    let mut wrong = owner.clone();
    wrong.instance_id = "different-instance".into();
    let first = db.begin_expert_attempt(&id, 0, &owner).unwrap().unwrap();
    assert!(
        !db.finish_expert_attempt(&id, &first, &wrong, outcome(HeadlessJobStatus::Completed))
            .unwrap()
    );
    assert!(
        db.finish_expert_attempt(&id, &first, &owner, outcome(HeadlessJobStatus::Failed))
            .unwrap()
    );
    assert!(
        !db.finish_expert_attempt(&id, &first, &owner, outcome(HeadlessJobStatus::Completed))
            .unwrap()
    );
    let second = db
        .begin_expert_attempt(&id, current(&db, &id).revision, &owner)
        .unwrap()
        .unwrap();
    assert!(
        db.finish_expert_attempt(&id, &second, &owner, outcome(HeadlessJobStatus::Completed))
            .unwrap()
    );
    let outcomes = db.expert_attempt_outcomes(&id).unwrap();
    assert_eq!(outcomes.len(), 2);
    assert_eq!(
        outcomes[0].1.as_ref().unwrap().status,
        HeadlessJobStatus::Failed
    );
    assert_eq!(outcomes[0].1.as_ref().unwrap().usage.input_tokens, Some(12));
    assert_eq!(outcomes[1].1.as_ref().unwrap().usage.output_tokens, None);
    assert_eq!(
        current(&db, &id).handoff_state,
        "none",
        "raw completion cannot deliver or stage a handoff"
    );
}

#[test]
fn persisted_cancellation_wins_over_late_success() {
    let (_dir, db, origin, request) = setup();
    let id = db.create_expert_consultation(&origin, &request).unwrap();
    let owner = owner();
    let attempt = db.begin_expert_attempt(&id, 0, &owner).unwrap().unwrap();
    assert!(db.cancel_expert_attempt(&id, 1).unwrap());
    assert!(
        db.finish_expert_attempt(&id, &attempt, &owner, outcome(HeadlessJobStatus::Completed))
            .unwrap()
    );
    assert_eq!(current(&db, &id).state, "cancelled");
    let stored = db
        .expert_attempt_outcomes(&id)
        .unwrap()
        .remove(0)
        .1
        .unwrap();
    assert!(stored.response.is_none());
    assert_eq!(stored.usage.input_tokens, Some(12));
}

#[test]
fn full_replace_store_save_and_database_reopen_preserve_consultations() {
    let (dir, db, mut origin, request) = setup();
    let mut store = crate::project::ProjectStore::empty();
    let mut project = crate::project::Project::new(
        "test".into(),
        dir.path().to_path_buf(),
        true,
        AgentKind::Claude,
    );
    let mut feature = crate::project::Feature::new(
        "feature".into(),
        "branch".into(),
        dir.path().to_path_buf(),
        false,
        crate::project::VibeMode::Vibe,
        false,
        false,
        AgentKind::Claude,
        false,
        false,
    );
    let session = feature.add_session(crate::project::SessionKind::Claude);
    origin.session_id = session.id.clone();
    origin.feature_id = feature.id.clone();
    origin.project_id = project.id.clone();
    project.features.push(feature);
    store.projects.push(project);
    db.save_store(&store).unwrap();
    let id = completed(&db, &origin, &request);
    assert!(
        db.stage_expert_handoff(&id, current(&db, &id).revision, "original advice")
            .unwrap()
    );
    assert!(
        db.stage_expert_handoff(&id, current(&db, &id).revision, "edited advice")
            .unwrap()
    );
    db.save_store(&store).unwrap();
    drop(db);
    let db = AmfDb::open(&dir.path().join("test.db")).unwrap();
    assert_eq!(current(&db, &id).handoff_state, "ready");
    assert_eq!(
        db.expert_handoff_body(&id).unwrap().as_deref(),
        Some("edited advice")
    );
    assert_eq!(db.expert_attempt_outcomes(&id).unwrap().len(), 1);
    assert_eq!(
        db.expert_consultations_for_session(&origin.session_id)
            .unwrap(),
        [id]
    );
}

#[test]
fn separate_instances_cannot_claim_the_same_attempt_or_delivery() {
    let (dir, db, origin, request) = setup();
    let other = AmfDb::open(&dir.path().join("test.db")).unwrap();
    let id = db.create_expert_consultation(&origin, &request).unwrap();
    let owner = owner();
    let attempt = db.begin_expert_attempt(&id, 0, &owner).unwrap().unwrap();
    assert!(
        other
            .begin_expert_attempt(&id, 0, &owner)
            .unwrap()
            .is_none()
    );
    db.finish_expert_attempt(&id, &attempt, &owner, outcome(HeadlessJobStatus::Completed))
        .unwrap();
    db.stage_expert_handoff(&id, current(&db, &id).revision, "advice")
        .unwrap();
    let revision = current(&db, &id).revision;
    let delivery = db
        .claim_expert_delivery(&id, revision, &owner)
        .unwrap()
        .unwrap();
    assert!(
        other
            .claim_expert_delivery(&id, revision, &owner)
            .unwrap()
            .is_none()
    );
    assert!(
        db.finish_expert_delivery(&id, &delivery, &owner, true)
            .unwrap()
    );
    assert!(
        !db.finish_expert_delivery(&id, &delivery, &owner, true)
            .unwrap()
    );
    assert!(
        !db.stage_expert_handoff(&id, current(&db, &id).revision, "duplicate")
            .unwrap()
    );
}

#[test]
fn recovery_preserves_live_unknown_and_future_schema_owners() {
    let (_dir, db, origin, request) = setup();
    let id = db.create_expert_consultation(&origin, &request).unwrap();
    db.begin_expert_attempt(&id, 0, &owner()).unwrap();
    for liveness in [OwnerLiveness::Alive, OwnerLiveness::Unknown] {
        assert_eq!(db.recover_expert_with(i64::MAX, |_| liveness).unwrap(), 0);
        assert_eq!(current(&db, &id).state, "running");
    }
    db.conn
        .execute(
            "UPDATE expert_consultations SET schema_version=999 WHERE id=?1",
            [&id],
        )
        .unwrap();
    assert_eq!(
        db.recover_expert_with(i64::MAX, |_| OwnerLiveness::Dead)
            .unwrap(),
        0
    );
    assert!(db.expert_consultation(&id).is_err());
    assert!(!db.revise_expert_request(&id, 1, &request).unwrap());
}

#[test]
fn abandoned_run_is_interrupted_without_relaunch_and_late_result_is_rejected() {
    let (_dir, db, origin, request) = setup();
    let id = db.create_expert_consultation(&origin, &request).unwrap();
    let owner = owner();
    let attempt = db.begin_expert_attempt(&id, 0, &owner).unwrap().unwrap();
    assert_eq!(
        db.recover_expert_with(i64::MAX, |_| OwnerLiveness::Dead)
            .unwrap(),
        1
    );
    assert_eq!(current(&db, &id).state, "interrupted");
    assert!(current(&db, &id).active_attempt_id.is_none());
    assert!(
        !db.finish_expert_attempt(&id, &attempt, &owner, outcome(HeadlessJobStatus::Completed))
            .unwrap()
    );
    assert_eq!(db.expert_attempt_outcomes(&id).unwrap().len(), 1);
}

#[test]
fn crash_during_send_becomes_unknown_and_never_auto_retries() {
    let (_dir, db, origin, request) = setup();
    let id = completed(&db, &origin, &request);
    db.stage_expert_handoff(&id, current(&db, &id).revision, "edited handoff")
        .unwrap();
    let owner = owner();
    let delivery = db
        .claim_expert_delivery(&id, current(&db, &id).revision, &owner)
        .unwrap()
        .unwrap();
    assert_eq!(
        db.recover_expert_with(i64::MAX, |_| OwnerLiveness::Dead)
            .unwrap(),
        1
    );
    let row = current(&db, &id);
    assert_eq!(row.state, "completed");
    assert_eq!(row.handoff_state, "delivery_unknown");
    assert!(
        db.claim_expert_delivery(&id, row.revision, &owner)
            .unwrap()
            .is_none()
    );
    assert!(
        !db.finish_expert_delivery(&id, &delivery, &owner, true)
            .unwrap()
    );
    assert_eq!(
        db.expert_handoff_body(&id).unwrap().as_deref(),
        Some("edited handoff")
    );
}

#[test]
fn heartbeat_update_during_recovery_invalidates_the_recovery_snapshot() {
    let (_dir, db, origin, request) = setup();
    let id = db.create_expert_consultation(&origin, &request).unwrap();
    db.begin_expert_attempt(&id, 0, &owner()).unwrap();
    assert_eq!(
        db.recover_expert_with(i64::MAX, |_| {
            db.conn
                .execute(
                    "UPDATE expert_consultations SET heartbeat_at=heartbeat_at+1 WHERE id=?1",
                    [&id],
                )
                .unwrap();
            OwnerLiveness::Dead
        })
        .unwrap(),
        0
    );
    assert_eq!(current(&db, &id).state, "running");
}

#[test]
fn explicit_deletion_cascades_only_consultation_owned_records() {
    let (_dir, db, origin, request) = setup();
    let id = completed(&db, &origin, &request);
    db.stage_expert_handoff(&id, current(&db, &id).revision, "advice")
        .unwrap();
    let another = db.create_expert_consultation(&origin, &request).unwrap();
    db.begin_expert_attempt(&another, 0, &owner()).unwrap();
    assert!(!db.delete_expert_consultation(&another).unwrap());
    assert!(db.delete_expert_consultation(&id).unwrap());
    for table in [
        "expert_requests",
        "expert_attempts",
        "expert_handoffs",
        "expert_deliveries",
    ] {
        let count: i64 = db
            .conn
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE consultation_id=?1"),
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }
    assert!(db.expert_consultation(&another).unwrap().is_some());
}

#[test]
fn request_validation_and_failed_completion_are_atomic() {
    let (_dir, db, origin, mut request) = setup();
    request.profile.harness = AgentKind::Codex;
    assert!(db.create_expert_consultation(&origin, &request).is_err());
    request.profile.harness = AgentKind::Claude;
    request.profile.limits.stdout_bytes = 0;
    assert!(db.create_expert_consultation(&origin, &request).is_err());
    request.profile.limits = Default::default();
    let id = db.create_expert_consultation(&origin, &request).unwrap();
    let owner = owner();
    let attempt = db.begin_expert_attempt(&id, 0, &owner).unwrap().unwrap();
    let mut bad = outcome(HeadlessJobStatus::Completed);
    bad.response = None;
    assert!(
        db.finish_expert_attempt(&id, &attempt, &owner, bad)
            .is_err()
    );
    assert_eq!(current(&db, &id).state, "running");
    assert!(db.expert_attempt_outcomes(&id).unwrap()[0].1.is_none());
}

#[test]
fn evaluation_export_keeps_failed_attempts_and_unknown_costs_explicit() {
    let (_dir, db, origin, request) = setup();
    let id = db.create_expert_consultation(&origin, &request).unwrap();
    let owner = owner();
    let attempt = db.begin_expert_attempt(&id, 0, &owner).unwrap().unwrap();
    assert!(
        db.finish_expert_attempt(&id, &attempt, &owner, outcome(HeadlessJobStatus::Failed))
            .unwrap()
    );
    let json = db.export_expert_evaluation(&id).unwrap();
    let export: super::ExpertEvaluationExport = serde_json::from_str(&json).unwrap();
    assert_eq!(export.attempts.len(), 1);
    assert_eq!(export.attempts[0].status, "failed");
    assert_eq!(export.attempts[0].usage.input_tokens, Some(12));
    assert!(!export.attempts[0].usage_complete);
    assert!(export.attempts[0].cost.is_none());
    assert!(export.attempts[0].billing_basis.is_none());
}

#[test]
fn migration_36_rolls_back_partial_schema_and_version_on_failure() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch("CREATE TABLE schema_version(version INTEGER PRIMARY KEY,applied_at TEXT NOT NULL,description TEXT NOT NULL); INSERT INTO schema_version VALUES(35,'now','fixture'); CREATE TABLE expert_handoffs(conflict TEXT);").unwrap();
    assert!(super::super::migrations::run(&conn).is_err());
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='expert_consultations'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "partial migration was not rolled back");
    let version: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 35);
    conn.execute_batch("DROP TABLE expert_handoffs").unwrap();
    super::super::migrations::run(&conn).unwrap();
    super::super::migrations::run(&conn).unwrap();
    let version: i64 = conn
        .query_row("SELECT MAX(version) FROM schema_version", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(version, 36);
}
