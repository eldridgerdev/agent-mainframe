//! Design Review Phase 3: the review room's open gate, agenda, persisted item
//! states and threads, diff-viewer round trip, and Usage tab.

use super::support::*;
use crate::app::design_review::room::{RoomTab, header_label};
use crate::app::*;
use crate::design_review::outbox;
use crate::design_review::record::{DecisionKind, DecisionRecord, OutboxLine};
use crate::project::{AgentKind, SessionKind, VibeMode};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tempfile::{NamedTempFile, TempDir};

fn view() -> ViewState {
    ViewState::new(
        "my-project".into(),
        "my-feat".into(),
        "amf-my-feat".into(),
        "claude".into(),
        "Claude".into(),
        SessionKind::Claude,
        VibeMode::default(),
        false,
    )
}

fn press(app: &mut App, code: KeyCode) {
    crate::handlers::handle_key(app, KeyEvent::new(code, KeyModifiers::NONE), 40).unwrap();
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn decision(uid: &str, seq: u32, paths: &[&str], ask: Option<&str>) -> OutboxLine {
    OutboxLine::Decision(DecisionRecord {
        uid: uid.into(),
        seq,
        kind: DecisionKind::Architecture,
        title: format!("decision {seq}"),
        why: format!("because {seq}"),
        rejected: vec!["db write: contention".into()],
        ask: ask.map(str::to_string),
        supersedes: None,
        paths: paths.iter().map(|p| p.to_string()).collect(),
        dirty: Default::default(),
        head: Some("abc123".into()),
        session_id: None,
        harness: "claude".into(),
        ts: "2026-11-13T00:00:00Z".into(),
    })
}

/// An app on a Design Review feature with a database, and two decisions in
/// its outbox, waiting to be ingested when the room opens.
fn setup(dir: &TempDir) -> (NamedTempFile, App) {
    let mut store = store_with_repo(dir.path().to_path_buf(), ProjectStatus::Idle);
    store.projects[0].features[0].design_review = true;
    let mut app = App::new_for_test(
        store,
        Box::new(MockTmuxOps::new()),
        Box::new(MockWorktreeOps::new()),
    );
    let db_file = NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    outbox::bind(dir.path(), "feat-1").unwrap();
    let path = outbox::outbox_path(dir.path());
    outbox::append(&path, &decision("u1", 1, &["src/a.rs"], Some("worth it?"))).unwrap();
    outbox::append(&path, &decision("u2", 2, &["src/b.rs"], None)).unwrap();
    app.mode = AppMode::Viewing(view());
    (db_file, app)
}

fn room(app: &App) -> &crate::app::design_review::room::DesignReviewRoom {
    match &app.mode {
        AppMode::DesignReview(room) => room,
        _ => panic!("expected the review room"),
    }
}

fn titles(app: &App) -> Vec<String> {
    room(app).items.iter().map(|i| i.title.clone()).collect()
}

fn last_toast(app: &App) -> String {
    app.toasts
        .last()
        .map(|t| t.message.clone())
        .unwrap_or_default()
}

fn open(app: &mut App) {
    assert!(app.open_design_review(0, 0, view(), Some(Default::default())));
}

fn select(app: &mut App, title: &str) {
    while room(app).selected_item().unwrap().title != title {
        let before = room(app).selected;
        press(app, KeyCode::Char('j'));
        assert_ne!(room(app).selected, before, "no item titled {title:?}");
    }
}

fn state_of(app: &App, title: &str) -> String {
    room(app)
        .items
        .iter()
        .find(|i| i.title == title)
        .unwrap()
        .state
        .clone()
}

/// Wait for the background snapshot of a leader-`y` open and apply it.
fn finish_opening(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.design_review_opening.is_some() {
        if app.poll_design_review_opening() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the room never opened"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

// ── Opening ───────────────────────────────────────────────

#[test]
fn opening_refuses_unsupported_and_flag_off_features_with_a_toast() {
    let dir = TempDir::new().unwrap();

    let (_db, mut app) = setup(&dir);
    app.store.projects[0].features[0].agent = AgentKind::Codex;
    app.open_design_review_from_view();
    assert_eq!(last_toast(&app), "Design Review is Claude only for now");
    assert!(matches!(app.mode, AppMode::Viewing(_)));

    let (_db, mut app) = setup(&dir);
    app.store.projects[0].containerized = true;
    app.open_design_review_from_view();
    assert_eq!(
        last_toast(&app),
        "Design Review is not available in container projects yet"
    );
    assert!(matches!(app.mode, AppMode::Viewing(_)));

    let (_db, mut app) = setup(&dir);
    app.store.projects[0].features[0].design_review = false;
    app.open_design_review_from_view();
    assert_eq!(last_toast(&app), "Design Review is off for this feature");
    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert!(
        app.db
            .as_ref()
            .unwrap()
            .design_review_latest_round("feat-1")
            .unwrap()
            .is_none(),
        "a refused open creates nothing"
    );
}

#[test]
fn leader_y_opens_the_room_with_ingested_decisions_in_meeting_order() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app) = setup(&dir);
    app.activate_leader();
    press(&mut app, KeyCode::Char('y'));
    // The git snapshot is read off the UI thread; the view stays meanwhile.
    assert!(matches!(app.mode, AppMode::Viewing(_)));
    assert!(app.design_review_opening.is_some());
    finish_opening(&mut app);

    assert!(matches!(app.mode, AppMode::DesignReview(_)));
    assert_eq!(
        titles(&app),
        [
            "Brief",
            "Architecture",
            "Key flow",
            "Fit",
            "d1 decision 1",
            "d2 decision 2",
            "d1? worth it?",
            "Plan drift"
        ]
    );
    assert_eq!(room(&app).round, 1);
    assert_eq!(header_label(), "Design Review · Claude only for now");
    let round = app
        .db
        .as_ref()
        .unwrap()
        .design_review_latest_round("feat-1")
        .unwrap()
        .unwrap();
    assert_eq!(round.round, 1);

    // Esc returns to the session view.
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.mode, AppMode::Viewing(_)));
}

#[test]
fn a_section_without_an_artifact_says_not_prepared_and_a_stored_one_renders() {
    let dir = TempDir::new().unwrap();
    let (db_file, mut app) = setup(&dir);
    open(&mut app);
    assert_eq!(room(&app).selected_item().unwrap().title, "Brief");
    assert_eq!(room(&app).content, "Not prepared yet");

    let conn = rusqlite::Connection::open(db_file.path()).unwrap();
    for (hash, body) in [
        ("h1", "old brief"),
        (
            "h2",
            "# New brief\n\n```mermaid\nflowchart TD\n  A-->B\n```",
        ),
    ] {
        conn.execute(
            "INSERT INTO design_review_artifacts
                (feature_id, project_id, section, input_hash, format, body, created_at)
             VALUES ('feat-1', 'proj-1', 'brief', ?1, 'markdown', ?2, datetime('now'))",
            rusqlite::params![hash, body],
        )
        .unwrap();
    }
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char('k'));
    assert!(room(&app).content.starts_with("# New brief"), "newest wins");

    let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    terminal
        .draw(|frame| crate::ui::draw(frame, &mut app))
        .unwrap();
    let screen: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(screen.contains("New brief"));
}

#[test]
fn a_snapshot_that_arrives_after_leaving_the_view_opens_nothing() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app) = setup(&dir);
    app.open_design_review_from_view();
    app.mode = AppMode::Normal;
    finish_opening(&mut app);
    assert!(app.design_review_opening.is_none());
    assert!(matches!(app.mode, AppMode::Normal));
}

#[test]
fn a_decision_whose_paths_left_the_diff_is_marked_stale() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app) = setup(&dir);
    let file = |path: &str| crate::diff::DiffFile {
        old_path: None,
        path: path.into(),
        status: crate::diff::DiffFileStatus::Modified,
        additions: 1,
        deletions: 0,
        is_binary: false,
        old_content: None,
        new_content: None,
        patch: String::new(),
        hunks: vec![],
    };
    // src/a.rs is still changed; src/b.rs is not.
    let snapshot = crate::app::design_review::room_snapshot_for_test(vec![file("src/a.rs")]);
    assert!(app.open_design_review(0, 0, view(), Some(snapshot)));
    let stale: Vec<_> = room(&app)
        .items
        .iter()
        .filter(|i| i.stale == crate::design_review::agenda::Staleness::PossiblyStale)
        .map(|i| i.title.clone())
        .collect();
    assert_eq!(stale, ["d2 decision 2"]);
}

// ── Item states ───────────────────────────────────────────

#[test]
fn item_states_and_threads_persist_and_resume_after_reopening() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app) = setup(&dir);
    open(&mut app);

    // Accept the brief.
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(state_of(&app, "Brief"), "accepted");
    press(&mut app, KeyCode::Char('a'));
    assert!(last_toast(&app).contains("Already accepted"));

    // Note on the architecture: reviewer turn, then accepted.
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Char('n'));
    type_text(&mut app, "looks right");
    press(&mut app, KeyCode::Enter);
    assert_eq!(state_of(&app, "Architecture"), "accepted");
    assert_eq!(room(&app).turns.len(), 1);
    assert_eq!(room(&app).turns[0].role, "reviewer");
    assert_eq!(room(&app).turns[0].body, "looks right");

    // Request a change: empty text is refused, then it sticks.
    select(&mut app, "d1 decision 1");
    press(&mut app, KeyCode::Char('c'));
    press(&mut app, KeyCode::Enter);
    assert!(room(&app).input.is_some(), "empty text keeps the prompt");
    type_text(&mut app, "use a table");
    press(&mut app, KeyCode::Enter);
    assert!(room(&app).input.is_none());
    assert_eq!(state_of(&app, "d1 decision 1"), "change-requested");
    press(&mut app, KeyCode::Char('a'));
    assert!(last_toast(&app).contains("developer's reply"));

    // Ask: the record answers at L0 and the item is pending again.
    select(&mut app, "d2 decision 2");
    press(&mut app, KeyCode::Char('s'));
    type_text(&mut app, "why not a db?");
    press(&mut app, KeyCode::Enter);
    assert_eq!(state_of(&app, "d2 decision 2"), "pending");
    let turns = &room(&app).turns;
    assert_eq!(
        turns.iter().map(|t| t.role.as_str()).collect::<Vec<_>>(),
        ["reviewer", "record"]
    );
    assert_eq!(turns[1].level.as_deref(), Some("L0"));
    assert!(turns[1].body.contains("because 2"));
    assert!(turns[1].body.contains("db write: contention"));

    // Close and reopen: the same round, states and threads.
    press(&mut app, KeyCode::Esc);
    open(&mut app);
    assert_eq!(room(&app).round, 1);
    assert_eq!(state_of(&app, "Brief"), "accepted");
    assert_eq!(state_of(&app, "Architecture"), "accepted");
    assert_eq!(state_of(&app, "d1 decision 1"), "change-requested");
    assert_eq!(state_of(&app, "d2 decision 2"), "pending");
    assert_eq!(room(&app).items.len(), 8, "no item was duplicated");
    // The room resumes at the first item still waiting.
    assert_eq!(room(&app).selected_item().unwrap().title, "Key flow");
    select(&mut app, "d2 decision 2");
    assert_eq!(room(&app).turns.len(), 2);
}

#[test]
fn the_room_resumes_past_items_waiting_on_the_developer() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app) = setup(&dir);
    open(&mut app);
    for _ in ["Brief", "Architecture", "Key flow", "Fit"] {
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char('j'));
    }
    assert_eq!(room(&app).selected_item().unwrap().title, "d1 decision 1");
    press(&mut app, KeyCode::Char('c'));
    type_text(&mut app, "use a table");
    press(&mut app, KeyCode::Enter);
    assert_eq!(state_of(&app, "d1 decision 1"), "change-requested");

    press(&mut app, KeyCode::Esc);
    open(&mut app);
    assert_eq!(room(&app).selected_item().unwrap().title, "d2 decision 2");
}

#[test]
fn reopening_adds_new_decisions_without_touching_existing_items() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app) = setup(&dir);
    open(&mut app);
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Esc);

    outbox::append(
        &outbox::outbox_path(dir.path()),
        &decision("u3", 3, &["src/c.rs"], None),
    )
    .unwrap();
    open(&mut app);
    assert!(titles(&app).contains(&"d3 decision 3".to_string()));
    assert_eq!(state_of(&app, "Brief"), "accepted");
    assert_eq!(state_of(&app, "d3 decision 3"), "pending");
}

// ── Diff viewer round trip ────────────────────────────────

#[test]
fn open_files_goes_to_the_diff_viewer_and_back_to_the_room() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app) = setup(&dir);
    open(&mut app);

    press(&mut app, KeyCode::Char('o'));
    assert!(last_toast(&app).contains("no linked files"));
    assert!(matches!(app.mode, AppMode::DesignReview(_)));

    select(&mut app, "d1 decision 1");
    press(&mut app, KeyCode::Char('o'));
    match &app.mode {
        AppMode::DiffViewerLoading(state) => {
            assert_eq!(state.focus_path.as_deref(), Some("src/a.rs"));
            assert!(matches!(
                state.return_to.as_deref(),
                Some(AppMode::DesignReview(_))
            ));
        }
        _ => panic!("expected the diff viewer"),
    }
    app.close_diff_viewer();
    assert!(matches!(app.mode, AppMode::DesignReview(_)));
    assert_eq!(room(&app).selected_item().unwrap().title, "d1 decision 1");
}

// ── Usage tab ─────────────────────────────────────────────

#[test]
fn usage_tab_groups_drills_and_the_header_totals_dollars() {
    let dir = TempDir::new().unwrap();
    let (db_file, mut app) = setup(&dir);
    open(&mut app);
    assert_eq!(room(&app).cost_total, "—");

    let conn = rusqlite::Connection::open(db_file.path()).unwrap();
    for (agent, task, model, cost) in [
        ("developer", "question-L3", "fable", 0.6),
        ("presenter", "brief", "sonnet", 0.2),
        ("presenter", "fit", "haiku", 0.1),
    ] {
        conn.execute(
            "INSERT INTO design_review_usage
                (feature_id, project_id, phase, task, agent, model, input, output,
                 cost_usd, provenance, started_at)
             VALUES ('feat-1', 'proj-1', 'prepare', ?1, ?2, ?3, 100, 10, ?4,
                     'measured', '2026-11-13T10:00:00Z')",
            rusqlite::params![task, agent, model, cost],
        )
        .unwrap();
    }

    press(&mut app, KeyCode::Tab);
    assert_eq!(room(&app).tab, RoomTab::Usage);
    assert_eq!(room(&app).cost_total, "$0.90");
    let table = room(&app).usage.table();
    assert_eq!(table.groups.len(), 2, "grouped by agent");
    assert!((table.premium_usd - 0.6).abs() < 1e-9);

    press(&mut app, KeyCode::Char('g'));
    assert_eq!(room(&app).usage.grouping.label(), "model");
    press(&mut app, KeyCode::Char('j'));
    press(&mut app, KeyCode::Enter);
    assert!(room(&app).usage.drill.is_some());
    press(&mut app, KeyCode::Esc);
    assert!(room(&app).usage.drill.is_none());
    assert_eq!(room(&app).tab, RoomTab::Usage);
    press(&mut app, KeyCode::Esc);
    assert_eq!(room(&app).tab, RoomTab::Agenda);

    // `u` jumps straight to the tab, as the header says.
    press(&mut app, KeyCode::Char('u'));
    assert_eq!(room(&app).tab, RoomTab::Usage);

    // A reload that shrinks the drilled group pulls the call selection back.
    while room(&app).usage.grouping.label() != "agent" {
        press(&mut app, KeyCode::Char('g'));
    }
    while room(&app).usage.table().groups[room(&app).usage.selected].key != "presenter" {
        press(&mut app, KeyCode::Char('j'));
    }
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char('j'));
    assert_eq!(room(&app).usage.call_selected, 1);
    conn.execute(
        "DELETE FROM design_review_usage WHERE task = 'fit'",
        rusqlite::params![],
    )
    .unwrap();
    app.design_review_reload_usage();
    assert_eq!(room(&app).usage.call_selected, 0);
}
