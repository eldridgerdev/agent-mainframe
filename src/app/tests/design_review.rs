//! Design Review Phase 1: instruction injection and turn-end outbox ingest.

use super::support::*;
use crate::app::setup::{
    DESIGN_REVIEW_BLOCK, DESIGN_REVIEW_BLOCK_MAX_BYTES, change_harness_setup,
    ensure_design_review_instructions, ensure_feature_review_instructions,
};
use crate::app::*;
use crate::design_review::outbox;
use crate::design_review::record::{DecisionKind, DecisionRecord, OutboxLine};
use crate::project::AgentKind;
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use std::io::Write;
use tempfile::{NamedTempFile, TempDir};

const BEGIN: &str = "<!-- AMF:design-review:begin -->";

fn claude_local(dir: &TempDir) -> Option<String> {
    std::fs::read_to_string(dir.path().join("CLAUDE.local.md")).ok()
}

// ── Instruction injection ─────────────────────────────────

#[test]
fn design_review_block_stays_within_its_byte_cap() {
    assert!(
        DESIGN_REVIEW_BLOCK.len() <= DESIGN_REVIEW_BLOCK_MAX_BYTES,
        "the always-on instruction is {} bytes; the cap is {}",
        DESIGN_REVIEW_BLOCK.len(),
        DESIGN_REVIEW_BLOCK_MAX_BYTES
    );
    assert!(DESIGN_REVIEW_BLOCK.contains("amf decision add"));
}

#[test]
fn claude_gets_the_block_once_and_disabling_strips_it() {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("CLAUDE.local.md"), "# Mine\n").unwrap();

    ensure_design_review_instructions(dir.path(), &AgentKind::Claude, true);
    ensure_design_review_instructions(dir.path(), &AgentKind::Claude, true);

    let text = claude_local(&dir).unwrap();
    assert!(text.starts_with("# Mine\n"));
    assert_eq!(text.matches(BEGIN).count(), 1);
    let ignore = std::fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|line| line == "CLAUDE.local.md"));

    ensure_design_review_instructions(dir.path(), &AgentKind::Claude, false);
    assert_eq!(claude_local(&dir).unwrap(), "# Mine\n");
}

#[test]
fn disabling_removes_a_file_that_only_held_the_block() {
    let dir = TempDir::new().unwrap();
    ensure_design_review_instructions(dir.path(), &AgentKind::Claude, true);
    ensure_design_review_instructions(dir.path(), &AgentKind::Claude, false);
    assert_eq!(claude_local(&dir), None);
}

#[test]
fn non_claude_harnesses_get_no_block_and_lose_a_stale_one() {
    for agent in [AgentKind::Codex, AgentKind::Opencode, AgentKind::Pi] {
        let dir = TempDir::new().unwrap();
        ensure_design_review_instructions(dir.path(), &agent, true);
        assert_eq!(claude_local(&dir), None, "{agent:?}");
        assert!(!dir.path().join("AGENTS.md").exists(), "{agent:?}");

        // A block left from when the feature ran Claude is stripped.
        ensure_design_review_instructions(dir.path(), &AgentKind::Claude, true);
        ensure_design_review_instructions(dir.path(), &agent, true);
        assert_eq!(claude_local(&dir), None, "{agent:?}");
    }
}

#[test]
fn design_review_suppresses_review_mode_notes_block() {
    let dir = TempDir::new().unwrap();
    let store = store_with_repo(dir.path().to_path_buf(), ProjectStatus::Stopped);
    let mut feature = store.projects[0].features[0].clone();
    feature.set_review_mode(true);
    ensure_feature_review_instructions(dir.path(), &feature, false);
    let text = claude_local(&dir).unwrap();
    assert!(text.contains("AMF:review-instructions:begin"));
    assert!(!text.contains(BEGIN));

    feature.set_design_review(true);
    assert!(
        !feature.review,
        "turning Design Review on clears Review Mode"
    );
    ensure_feature_review_instructions(dir.path(), &feature, false);
    let text = claude_local(&dir).unwrap();
    assert!(!text.contains("AMF:review-instructions:begin"));
    assert!(text.contains(BEGIN));

    // Both flags set (a hand-edited store): Design Review wins.
    feature.review = true;
    assert!(!feature.review_notes_enabled());
    ensure_feature_review_instructions(dir.path(), &feature, false);
    let text = claude_local(&dir).unwrap();
    assert!(!text.contains("AMF:review-instructions:begin"));

    feature.set_review_mode(true);
    assert!(
        !feature.design_review,
        "turning Review Mode on clears Design Review"
    );
}

#[test]
fn unsupported_feature_gets_no_block() {
    let dir = TempDir::new().unwrap();
    let store = store_with_repo(dir.path().to_path_buf(), ProjectStatus::Stopped);
    let mut feature = store.projects[0].features[0].clone();
    feature.set_design_review(true);

    // Containerized project.
    ensure_feature_review_instructions(dir.path(), &feature, true);
    assert_eq!(claude_local(&dir), None);

    // Non-Claude harness.
    feature.agent = AgentKind::Codex;
    ensure_feature_review_instructions(dir.path(), &feature, false);
    assert_eq!(claude_local(&dir), None);
}

#[test]
fn harness_change_away_from_claude_strips_the_block() {
    let dir = TempDir::new().unwrap();
    let repo = TempDir::new().unwrap();
    let store = store_with_repo(dir.path().to_path_buf(), ProjectStatus::Stopped);
    let mut feature = store.projects[0].features[0].clone();
    feature.set_design_review(true);
    ensure_feature_review_instructions(dir.path(), &feature, false);
    assert!(claude_local(&dir).unwrap().contains(BEGIN));

    change_harness_setup(
        &feature,
        repo.path(),
        false,
        &[AgentKind::Claude],
        &AgentKind::Codex,
    )
    .unwrap();
    assert!(!claude_local(&dir).unwrap_or_default().contains(BEGIN));
}

// ── Turn-end ingest ───────────────────────────────────────

fn record(uid: &str, seq: u32) -> OutboxLine {
    OutboxLine::Decision(DecisionRecord {
        uid: uid.into(),
        seq,
        kind: DecisionKind::Architecture,
        title: format!("decision {seq}"),
        why: "because".into(),
        rejected: vec!["db write: contention".into()],
        ask: Some("fresh enough?".into()),
        supersedes: None,
        paths: vec!["src/a.rs".into()],
        dirty: Default::default(),
        head: Some("abc123".into()),
        session_id: Some("sess-1".into()),
        harness: "claude".into(),
        ts: "2026-11-13T00:00:00Z".into(),
    })
}

fn app_with_db(dir: &TempDir, design_review: bool) -> (NamedTempFile, App) {
    let mut store = store_with_repo(dir.path().to_path_buf(), ProjectStatus::Idle);
    store.projects[0].features[0].design_review = design_review;
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    (db_file, app)
}

fn turn_end(app: &mut App, dir: &TempDir) {
    app.handle_ipc_message_value(serde_json::json!({
        "type": "stop",
        "session_id": "amf-my-feat",
        "cwd": dir.path().display().to_string(),
    }));
}

fn decisions(app: &App) -> Vec<crate::db::design_review::DesignDecision> {
    app.db
        .as_ref()
        .unwrap()
        .design_decisions_for_feature("feat-1")
        .unwrap()
}

#[test]
fn turn_end_ingests_new_outbox_lines_once() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app) = app_with_db(&dir, true);
    let path = outbox::outbox_path(dir.path());
    outbox::append(&path, &record("u1", 1)).unwrap();

    turn_end(&mut app, &dir);
    let rows = decisions(&app);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].uid, "u1");
    assert_eq!(rows[0].project_id, "proj-1");
    assert_eq!(rows[0].kind, "architecture");
    assert_eq!(rows[0].rejected, ["db write: contention"]);
    assert_eq!(rows[0].source, "agent");
    assert_eq!(rows[0].status, "open");

    // A malformed line and a partial line are skipped; the next turn picks up
    // the completed line.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"garbage\n").unwrap();
    outbox::append(&path, &record("u2", 2)).unwrap();
    let partial = serde_json::to_string(&record("u3", 3)).unwrap();
    let (head, tail) = partial.split_at(12);
    file.write_all(head.as_bytes()).unwrap();
    turn_end(&mut app, &dir);
    assert_eq!(decisions(&app).len(), 2);

    file.write_all(format!("{tail}\n").as_bytes()).unwrap();
    turn_end(&mut app, &dir);
    turn_end(&mut app, &dir);
    let uids: Vec<_> = decisions(&app).into_iter().map(|d| d.uid).collect();
    assert_eq!(uids, ["u1", "u2", "u3"]);

    // A rewritten outbox restarts at 0 without duplicating known uids.
    std::fs::remove_file(&path).unwrap();
    outbox::append(&path, &record("u1", 1)).unwrap();
    turn_end(&mut app, &dir);
    assert_eq!(decisions(&app).len(), 3);
}

#[test]
fn turn_end_skips_features_without_design_review() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app) = app_with_db(&dir, false);
    outbox::append(&outbox::outbox_path(dir.path()), &record("u1", 1)).unwrap();
    turn_end(&mut app, &dir);
    assert!(decisions(&app).is_empty());

    // Review-open ingests on demand regardless of the turn-end filter.
    assert_eq!(app.ingest_design_review_outbox(0, 0), 1);
    assert_eq!(decisions(&app).len(), 1);
}
