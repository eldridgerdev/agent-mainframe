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

// ── Creation surfaces ─────────────────────────────────────

mod creation {
    use super::super::feature_sessions::startup_prompt_overlay_launch;
    use super::*;
    use crate::automation::CreateFeatureRequest;
    use crate::extension::FeaturePreset;
    use crate::project::VibeMode;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;

    fn press(app: &mut App, code: KeyCode) {
        crate::handlers::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 40).unwrap();
    }

    fn form() -> App {
        let mut app = App::new_for_test(
            store_with_feature(ProjectStatus::Stopped),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        let mut state = CreateFeatureState::new(
            "my-project".into(),
            PathBuf::from("/tmp/test-repo"),
            Vec::new(),
            false,
        );
        state.branch = "feat".into();
        state.step = CreateFeatureStep::Mode;
        app.mode = AppMode::CreatingFeature(state);
        app
    }

    fn state(app: &App) -> &CreateFeatureState {
        match &app.mode {
            AppMode::CreatingFeature(state) => state,
            _ => panic!("expected the New Feature form"),
        }
    }

    fn state_mut(app: &mut App) -> &mut CreateFeatureState {
        match &mut app.mode {
            AppMode::CreatingFeature(state) => state,
            _ => panic!("expected the New Feature form"),
        }
    }

    fn focus_design_row(app: &mut App) {
        let state = state_mut(app);
        state.step = CreateFeatureStep::Mode;
        state.mode_focus = state.mode_max_focus();
    }

    #[test]
    fn dialog_row_toggles_and_is_exclusive_with_review() {
        let mut app = form();
        assert_eq!(state(&app).mode_max_focus(), 7, "Claude: last row");
        focus_design_row(&mut app);
        press(&mut app, KeyCode::Char(' '));
        assert!(state(&app).design_review);

        // Review Mode takes over: it clears Design Review.
        state_mut(&mut app).mode_focus = 2;
        press(&mut app, KeyCode::Char(' '));
        assert!(state(&app).review && !state(&app).design_review);

        // And Design Review clears Review Mode.
        focus_design_row(&mut app);
        press(&mut app, KeyCode::Char(' '));
        assert!(state(&app).design_review && !state(&app).review);
        press(&mut app, KeyCode::Char(' '));
        assert!(!state(&app).design_review);
    }

    #[test]
    fn dialog_row_is_disabled_for_other_harnesses_and_unchecks_on_change() {
        let mut app = form();
        focus_design_row(&mut app);
        press(&mut app, KeyCode::Char(' '));
        assert!(state(&app).design_review);

        let codex = state(&app)
            .allowed_agents
            .iter()
            .position(|agent| *agent == AgentKind::Codex)
            .unwrap();
        state_mut(&mut app).select_agent(codex);
        assert!(!state(&app).design_review, "unchecked on harness change");
        assert_eq!(
            state(&app).design_review_unavailable_reason(),
            Some("Claude only for now")
        );

        focus_design_row(&mut app);
        assert_eq!(state(&app).mode_max_focus(), 5, "non-Claude: last row");
        press(&mut app, KeyCode::Char(' '));
        assert!(!state(&app).design_review, "cannot be toggled");

        let mut containerized = form();
        state_mut(&mut containerized).containerized = true;
        focus_design_row(&mut containerized);
        press(&mut containerized, KeyCode::Char(' '));
        assert!(!state(&containerized).design_review);
        assert_eq!(
            state(&containerized).design_review_unavailable_reason(),
            Some("not available in container projects yet")
        );
    }

    #[test]
    fn dialog_renders_the_row_label_and_the_disabled_reason() {
        use ratatui::{Terminal, backend::TestBackend};
        let render = |app: &mut App| {
            let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
            terminal.draw(|frame| crate::ui::draw(frame, app)).unwrap();
            let buffer = terminal.backend().buffer().clone();
            (0..buffer.area.height)
                .map(|y| {
                    (0..buffer.area.width)
                        .map(|x| buffer[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let mut app = form();
        assert!(render(&mut app).contains("[ ] design review (Claude only for now)"));
        let codex = state(&app)
            .allowed_agents
            .iter()
            .position(|agent| *agent == AgentKind::Codex)
            .unwrap();
        state_mut(&mut app).select_agent(codex);
        assert!(render(&mut app).contains("Design Review: Claude only for now"));
    }

    fn preset_form(agent: AgentKind) -> App {
        let mut app = form();
        let state = state_mut(&mut app);
        state.step = CreateFeatureStep::SelectPreset;
        state.feature_presets = vec![FeaturePreset {
            name: "designed".into(),
            agent,
            mode: VibeMode::Vibe,
            design_review: true,
            ..FeaturePreset::default()
        }];
        app
    }

    #[test]
    fn preset_design_review_applies_on_claude() {
        let mut app = preset_form(AgentKind::Claude);
        press(&mut app, KeyCode::Enter);
        assert!(state(&app).design_review);
        assert!(app.toasts.is_empty());
    }

    #[test]
    fn preset_design_review_is_ignored_on_codex_with_a_message() {
        let mut app = preset_form(AgentKind::Codex);
        press(&mut app, KeyCode::Enter);
        assert_eq!(state(&app).agent, AgentKind::Codex);
        assert!(!state(&app).design_review);
        assert!(
            app.toasts
                .iter()
                .any(|toast| toast.message.contains("Design Review ignored")
                    && toast.message.contains("Claude only for now")),
            "{:?}",
            app.toasts.iter().map(|t| &t.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn preset_round_trips_design_review_and_old_presets_default_off() {
        let preset: FeaturePreset =
            serde_json::from_str(r#"{"name":"p","design_review":true}"#).unwrap();
        assert!(preset.design_review);
        let json = serde_json::to_string(&preset).unwrap();
        assert!(json.contains("\"design_review\":true"));
        let old: FeaturePreset = serde_json::from_str(r#"{"name":"p"}"#).unwrap();
        assert!(!old.design_review);
        assert!(
            !serde_json::to_string(&old)
                .unwrap()
                .contains("design_review")
        );
    }

    #[test]
    fn automation_request_round_trips_design_review() {
        let request: CreateFeatureRequest = serde_json::from_value(serde_json::json!({
            "project_name": "p", "branch": "b", "design_review": true
        }))
        .unwrap();
        assert!(request.design_review);
        assert_eq!(request.ipc_payload()["design_review"], true);
        let default: CreateFeatureRequest =
            serde_json::from_value(serde_json::json!({"project_name": "p"})).unwrap();
        assert!(!default.design_review);
    }

    fn automation_app(repo: &std::path::Path) -> App {
        App::new_for_test(
            store_with_empty_project(repo.to_path_buf(), true),
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        )
    }

    #[test]
    fn automation_refuses_design_review_on_other_harnesses_and_with_review() {
        let dir = TempDir::new().unwrap();
        let mut app = automation_app(dir.path());
        let base = CreateFeatureRequest {
            project_name: "automation-project".into(),
            branch: "feature-1".into(),
            design_review: true,
            use_worktree: Some(false),
            no_start: true,
            ..CreateFeatureRequest::default()
        };
        let err = app
            .create_feature_from_request(&CreateFeatureRequest {
                agent: AgentKind::Codex,
                mode: VibeMode::Vibe,
                ..base.clone()
            })
            .unwrap_err();
        assert!(err.to_string().contains("Claude only for now"), "{err}");
        let err = app
            .create_feature_from_request(&CreateFeatureRequest {
                review: true,
                ..base.clone()
            })
            .unwrap_err();
        assert!(err.to_string().contains("cannot both be on"), "{err}");
        assert!(app.store.projects[0].features.is_empty());

        app.create_feature_from_request(&base).unwrap();
        assert!(app.store.projects[0].features[0].design_review);
    }

    fn block_written(workdir: &std::path::Path) -> bool {
        std::fs::read_to_string(workdir.join("CLAUDE.local.md"))
            .is_ok_and(|text| text.contains(BEGIN))
    }

    #[test]
    fn creation_stores_the_flag_and_writes_the_block_only_for_claude() {
        let (app, workdir, _guards) =
            startup_prompt_overlay_launch(AgentKind::Claude, "claude", true);
        assert!(app.store.projects[0].features[0].design_review);
        assert!(block_written(&workdir));
    }

    #[test]
    fn creation_stores_no_flag_and_writes_no_block_for_an_unsupported_harness() {
        for (agent, window) in [
            (AgentKind::Codex, "codex"),
            (AgentKind::Opencode, "opencode"),
        ] {
            let (app, workdir, _guards) = startup_prompt_overlay_launch(agent, window, true);
            let feature = &app.store.projects[0].features[0];
            assert!(!feature.design_review, "no flag stored for {window}");
            assert!(!block_written(&workdir), "no block written for {window}");
            let persisted = ProjectStore::load(&app.store_path).unwrap();
            assert!(!persisted.projects[0].features[0].design_review);
        }
    }
}
