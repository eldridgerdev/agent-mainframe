//! Design Review Phase 4: finishing a round (brief, destination, dispatch),
//! developer replies, re-review, and approval.

use super::support::*;
use crate::app::design_review::room::DesignReviewRoom;
use crate::app::*;
use crate::design_review::outbox;
use crate::design_review::record::{
    DecisionKind, DecisionRecord, OutboxLine, ReplyKind, ReplyRecord,
};
use crate::project::{SessionKind, VibeMode};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::{Arc, Mutex};
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

fn decision(uid: &str, seq: u32, supersedes: Option<&str>) -> OutboxLine {
    OutboxLine::Decision(DecisionRecord {
        uid: uid.into(),
        seq,
        kind: DecisionKind::Architecture,
        title: format!("decision {seq}"),
        why: format!("because {seq}"),
        rejected: vec![],
        ask: None,
        supersedes: supersedes.map(str::to_string),
        paths: vec![format!("src/{seq}.rs")],
        dirty: Default::default(),
        head: Some("abc123".into()),
        session_id: None,
        harness: "claude".into(),
        ts: "2026-11-13T00:00:00Z".into(),
    })
}

fn reply(uid: &str, label: &str, kind: ReplyKind, text: &str) -> OutboxLine {
    OutboxLine::Reply(ReplyRecord {
        uid: uid.into(),
        label: label.into(),
        kind,
        text: text.into(),
        session_id: Some("s1".into()),
        ts: "2026-11-14T00:00:00Z".into(),
    })
}

/// The pasted texts, in order.
type Pasted = Arc<Mutex<Vec<(String, String, String)>>>;

/// An app on a Design Review feature with a live Claude session, whose tmux
/// double records every paste, and two decisions waiting in the outbox.
fn setup(dir: &TempDir) -> (NamedTempFile, App, Pasted) {
    let mut store = store_with_repo(dir.path().to_path_buf(), ProjectStatus::Idle);
    store.projects[0].features[0].design_review = true;
    store.projects[0].features[0]
        .sessions
        .push(make_session("claude", None));
    let pasted: Pasted = Arc::default();
    let sink = pasted.clone();
    let mut tmux = MockTmuxOps::new();
    tmux.expect_paste_text()
        .returning(move |session, window, text| {
            sink.lock()
                .unwrap()
                .push((session.into(), window.into(), text.into()));
            Ok(())
        });
    let mut app = App::new_for_test(store, Box::new(tmux), Box::new(MockWorktreeOps::new()));
    app.config.final_review_submit_prompt = false;
    let db_file = NamedTempFile::new().unwrap();
    app.db = Some(crate::db::AmfDb::open(db_file.path()).unwrap());
    outbox::bind(dir.path(), "feat-1").unwrap();
    let path = outbox::outbox_path(dir.path());
    outbox::append(&path, &decision("u1", 1, None)).unwrap();
    outbox::append(&path, &decision("u2", 2, None)).unwrap();
    app.mode = AppMode::Viewing(view());
    (db_file, app, pasted)
}

fn room(app: &App) -> &DesignReviewRoom {
    match &app.mode {
        AppMode::DesignReview(room) => room,
        _ => panic!("expected the review room"),
    }
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

fn item<'a>(app: &'a App, title: &str) -> &'a crate::db::design_review::DesignReviewItem {
    room(app).items.iter().find(|i| i.title == title).unwrap()
}

fn request_change(app: &mut App, title: &str, text: &str) {
    select(app, title);
    press(app, KeyCode::Char('c'));
    type_text(app, text);
    press(app, KeyCode::Enter);
    assert_eq!(item(app, title).state, "change-requested");
}

fn last_toast(app: &App) -> String {
    app.toasts
        .last()
        .map(|t| t.message.clone())
        .unwrap_or_default()
}

fn sql<T>(
    db_file: &NamedTempFile,
    query: &str,
    map: impl FnOnce(&rusqlite::Row) -> rusqlite::Result<T>,
) -> T {
    rusqlite::Connection::open(db_file.path())
        .unwrap()
        .query_row(query, [], map)
        .unwrap()
}

/// Accept everything the reviewer has not decided yet.
fn accept_all(app: &mut App) {
    for index in 0..room(app).items.len() {
        while room(app).selected < index {
            press(app, KeyCode::Char('j'));
        }
        if matches!(
            room(app).selected_item().unwrap().state.as_str(),
            "pending" | "re-review"
        ) {
            press(app, KeyCode::Char('a'));
        }
    }
}

/// Finish the round to the first destination (the live session).
fn send_brief(app: &mut App) {
    press(app, KeyCode::Char('f'));
    assert!(room(app).finish.is_some(), "{}", last_toast(app));
    press(app, KeyCode::Enter); // the live session
    press(app, KeyCode::Enter); // confirm
}

fn ingest(app: &mut App) -> usize {
    app.ingest_design_review_outbox(0, 0)
}

fn reset_cursor(app: &App) {
    app.db
        .as_ref()
        .unwrap()
        .set_design_review_outbox_cursor("proj-1", "feat-1", &Default::default())
        .unwrap();
}

// ── Finishing a round ────────────────────────────────────────

#[test]
fn finishing_with_changes_persists_the_verdict_and_pastes_the_brief_to_the_session() {
    let dir = TempDir::new().unwrap();
    let (db_file, mut app, pasted) = setup(&dir);
    open(&mut app);
    request_change(&mut app, "d1 decision 1", "use a table");
    // A question the reviewer left open (answers are L0 today, so set it up
    // directly).
    select(&mut app, "Plan drift");
    let drift = item(&app, "Plan drift").id.clone();
    {
        let db = app.db.as_ref().unwrap();
        db.append_design_review_turn(&crate::db::design_review::DesignReviewTurn {
            id: "t-q".into(),
            item_id: drift.clone(),
            round_id: None,
            feature_id: "feat-1".into(),
            project_id: "proj-1".into(),
            seq: 0,
            role: "reviewer".into(),
            level: None,
            body: "Is the drift\nexpected?".into(),
            status: "done".into(),
            usage_id: None,
            created_at: "t".into(),
        })
        .unwrap();
        db.set_design_review_item_state(&drift, "question-open", "t")
            .unwrap();
    }
    open(&mut app);

    send_brief(&mut app);

    let pasted = pasted.lock().unwrap().clone();
    assert_eq!(pasted.len(), 1, "{pasted:?}");
    assert_eq!(
        (pasted[0].0.as_str(), pasted[0].1.as_str()),
        ("amf-my-feat", "claude")
    );
    let expected = "Design review round 1: 1 change, 1 question.\n\
        \n\
        C1 [d1 decision 1] use a table\n\
        Q1 [Plan drift] Is the drift expected?\n\
        \n\
        Reply once per item, one line:\n\
        \x20 amf review reply C1 --done \"<what changed>\"\n\
        \x20 amf review reply C1 --pushback \"<why not>\"\n\
        \x20 amf review reply Q1 --answer \"<answer>\"\n\
        If a change replaces a recorded decision, add one with --supersedes dN.\n";
    assert_eq!(pasted[0].2, expected);
    assert!(matches!(app.mode, AppMode::Viewing(_)));

    let (verdict, brief, destination, finished): (String, String, String, Option<String>) = sql(
        &db_file,
        "SELECT verdict, brief, destination, finished_at FROM design_review_rounds",
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    );
    assert_eq!(verdict, "changes-requested");
    assert_eq!(brief, expected);
    assert_eq!(destination, "live");
    assert!(finished.is_some());
    let labels: Vec<(String, String)> = {
        let conn = rusqlite::Connection::open(db_file.path()).unwrap();
        let mut stmt = conn
            .prepare("SELECT title, label FROM design_review_items WHERE label LIKE 'C%' OR label LIKE 'Q%' ORDER BY title")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        labels,
        [
            ("Plan drift".to_string(), "Q1".to_string()),
            ("d1 decision 1".to_string(), "C1".to_string())
        ]
    );

    // Reopening keeps the labels (the agenda would otherwise rewrite them) and
    // shows the verdict.
    open(&mut app);
    assert_eq!(item(&app, "d1 decision 1").label.as_deref(), Some("C1"));
    assert_eq!(room(&app).verdict.as_deref(), Some("changes-requested"));
    assert!(!room(&app).read_only());
}

#[test]
fn a_brief_of_two_thousand_tokens_goes_through_a_file() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app, pasted) = setup(&dir);
    open(&mut app);
    request_change(&mut app, "d1 decision 1", &"word ".repeat(1_800));
    send_brief(&mut app);

    let path = dir.path().join(".amf/design-review/round-1.md");
    let file = std::fs::read_to_string(&path).unwrap();
    assert!(file.starts_with("Design review round 1: 1 change, 0 questions."));
    assert!(file.contains("C1 [d1 decision 1] word word"));
    let pasted = pasted.lock().unwrap().clone();
    assert_eq!(pasted.len(), 1);
    assert!(
        pasted[0].2.contains(&path.to_string_lossy().to_string()),
        "{}",
        pasted[0].2
    );
    assert!(pasted[0].2.len() < 500, "{}", pasted[0].2);
}

#[test]
fn finish_confirms_first_and_cancel_sends_nothing() {
    let dir = TempDir::new().unwrap();
    let (db_file, mut app, pasted) = setup(&dir);
    open(&mut app);
    request_change(&mut app, "d1 decision 1", "use a table");
    press(&mut app, KeyCode::Char('f'));
    press(&mut app, KeyCode::Char('j')); // another destination, then back
    press(&mut app, KeyCode::Char('k'));
    press(&mut app, KeyCode::Enter);
    assert!(matches!(
        room(&app).finish.as_ref().unwrap().stage,
        crate::app::design_review::room::FinishStage::ConfirmSend(_)
    ));
    press(&mut app, KeyCode::Esc); // back to destinations
    press(&mut app, KeyCode::Esc); // out
    assert!(room(&app).finish.is_none());
    assert!(pasted.lock().unwrap().is_empty());
    let verdict: Option<String> = sql(&db_file, "SELECT verdict FROM design_review_rounds", |r| {
        r.get(0)
    });
    assert_eq!(verdict, None);
}

#[test]
fn finish_with_undecided_items_says_what_is_left() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app, _pasted) = setup(&dir);
    open(&mut app);
    press(&mut app, KeyCode::Char('f'));
    assert!(room(&app).finish.is_none());
    assert!(
        last_toast(&app).contains("still wait on you"),
        "{}",
        last_toast(&app)
    );
}

// ── Replies ──────────────────────────────────────────────────

/// Two requested changes, finished, replies appended afterwards.
fn finished_with_two_changes(dir: &TempDir) -> (NamedTempFile, App, Pasted) {
    let (db_file, mut app, pasted) = setup(dir);
    open(&mut app);
    request_change(&mut app, "d1 decision 1", "use a table");
    request_change(&mut app, "d2 decision 2", "split it");
    send_brief(&mut app);
    open(&mut app);
    (db_file, app, pasted)
}

#[test]
fn done_and_pushback_replies_move_items_to_re_review_with_a_developer_turn() {
    let dir = TempDir::new().unwrap();
    let (_db, mut app, _pasted) = finished_with_two_changes(&dir);
    let path = outbox::outbox_path(dir.path());
    outbox::append(&path, &reply("r1", "C1", ReplyKind::Done, "now a table")).unwrap();
    outbox::append(&path, &reply("r2", "c2", ReplyKind::Pushback, "too big")).unwrap();
    ingest(&mut app);
    open(&mut app);

    assert_eq!(item(&app, "d1 decision 1").state, "re-review");
    assert_eq!(item(&app, "d2 decision 2").state, "re-review");
    assert_eq!(
        item(&app, "d1 decision 1").reply_kind.as_deref(),
        Some("done")
    );
    assert_eq!(
        item(&app, "d2 decision 2").reply.as_deref(),
        Some("too big")
    );
    select(&mut app, "d1 decision 1");
    let turns = &room(&app).turns;
    assert_eq!(turns.last().unwrap().role, "developer");
    assert_eq!(turns.last().unwrap().body, "Done: now a table");

    // Re-review: accept one, request another change on the other.
    press(&mut app, KeyCode::Char('a'));
    assert_eq!(item(&app, "d1 decision 1").state, "accepted");
    select(&mut app, "d2 decision 2");
    press(&mut app, KeyCode::Char('c'));
    type_text(&mut app, "still too big");
    press(&mut app, KeyCode::Enter);
    assert_eq!(item(&app, "d2 decision 2").state, "change-requested");
}

#[test]
fn ingesting_a_reply_twice_changes_nothing() {
    let dir = TempDir::new().unwrap();
    let (db_file, mut app, _pasted) = finished_with_two_changes(&dir);
    let path = outbox::outbox_path(dir.path());
    outbox::append(&path, &reply("r1", "C1", ReplyKind::Done, "now a table")).unwrap();
    ingest(&mut app);
    // The reviewer moves on; a lost cursor re-reads the line.
    open(&mut app);
    select(&mut app, "d1 decision 1");
    press(&mut app, KeyCode::Char('a'));
    reset_cursor(&app);
    ingest(&mut app);

    open(&mut app);
    assert_eq!(item(&app, "d1 decision 1").state, "accepted");
    let turns: i64 = sql(
        &db_file,
        "SELECT COUNT(*) FROM design_review_turns WHERE role = 'developer'",
        |r| r.get(0),
    );
    assert_eq!(turns, 1);
}

#[test]
fn an_answer_becomes_a_review_decision_and_returns_the_question_to_pending() {
    let dir = TempDir::new().unwrap();
    let (db_file, mut app, _pasted) = setup(&dir);
    open(&mut app);
    select(&mut app, "Plan drift");
    let drift = item(&app, "Plan drift").id.clone();
    {
        let db = app.db.as_ref().unwrap();
        db.append_design_review_turn(&crate::db::design_review::DesignReviewTurn {
            id: "t-q".into(),
            item_id: drift.clone(),
            round_id: None,
            feature_id: "feat-1".into(),
            project_id: "proj-1".into(),
            seq: 0,
            role: "reviewer".into(),
            level: None,
            body: "Why not one table per review?".into(),
            status: "done".into(),
            usage_id: None,
            created_at: "t".into(),
        })
        .unwrap();
        db.set_design_review_item_state(&drift, "question-open", "t")
            .unwrap();
    }
    open(&mut app);
    request_change(&mut app, "d1 decision 1", "x"); // so the finish has a change too
    send_brief(&mut app);
    open(&mut app);

    let path = outbox::outbox_path(dir.path());
    outbox::append(
        &path,
        &reply("r1", "Q1", ReplyKind::Answer, "One table keeps joins out"),
    )
    .unwrap();
    ingest(&mut app);
    reset_cursor(&app);
    ingest(&mut app);
    open(&mut app);

    assert_eq!(item(&app, "Plan drift").state, "pending");
    let (count, title, why, source, status, kind): (i64, String, String, String, String, String) =
        sql(
            &db_file,
            "SELECT COUNT(*), title, why, source, status, kind FROM design_decisions WHERE source = 'review'",
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                ))
            },
        );
    assert_eq!(count, 1, "idempotent");
    assert_eq!(title, "Why not one table per review?");
    assert_eq!(why, "One table keeps joins out");
    assert_eq!(
        (source.as_str(), status.as_str(), kind.as_str()),
        ("review", "open", "tradeoff")
    );
    // The review's own record is memory, not an agenda item.
    assert!(
        !room(&app)
            .items
            .iter()
            .any(|i| i.title.contains("Why not one table"))
    );
}

#[test]
fn a_reply_for_an_unknown_label_is_skipped() {
    let dir = TempDir::new().unwrap();
    let (db_file, mut app, _pasted) = finished_with_two_changes(&dir);
    let path = outbox::outbox_path(dir.path());
    outbox::append(&path, &reply("r9", "C9", ReplyKind::Done, "ghost")).unwrap();
    outbox::append(&path, &reply("r1", "C1", ReplyKind::Done, "real")).unwrap();
    ingest(&mut app);
    open(&mut app);
    assert_eq!(item(&app, "d1 decision 1").state, "re-review");
    let turns: i64 = sql(
        &db_file,
        "SELECT COUNT(*) FROM design_review_turns WHERE role = 'developer'",
        |r| r.get(0),
    );
    assert_eq!(turns, 1);
}

#[test]
fn a_later_record_supersedes_the_one_it_names() {
    let dir = TempDir::new().unwrap();
    let (db_file, mut app, _pasted) = setup(&dir);
    outbox::append(
        &outbox::outbox_path(dir.path()),
        &decision("u3", 3, Some("d1")),
    )
    .unwrap();
    ingest(&mut app);
    let status = |uid: &str| -> String {
        sql(
            &db_file,
            &format!("SELECT status FROM design_decisions WHERE uid = '{uid}'"),
            |r| r.get(0),
        )
    };
    assert_eq!(status("u1"), "superseded");
    assert_eq!(status("u2"), "open");
    assert_eq!(status("u3"), "open");
    open(&mut app);
    let state = |title: &str| {
        room(&app)
            .items
            .iter()
            .find(|i| i.title == title)
            .map(|i| i.stale)
    };
    assert_eq!(
        state("d1 decision 1"),
        None,
        "a superseded decision is not derived"
    );
    assert!(state("d3 decision 3").is_some());
}

// ── Approval ─────────────────────────────────────────────────

#[test]
fn approving_accepts_open_decisions_and_the_review_reopens_read_only() {
    let dir = TempDir::new().unwrap();
    let (db_file, mut app, _pasted) = setup(&dir);
    outbox::append(
        &outbox::outbox_path(dir.path()),
        &decision("u3", 3, Some("d1")),
    )
    .unwrap();
    open(&mut app);
    accept_all(&mut app);
    press(&mut app, KeyCode::Char('f'));
    assert!(matches!(
        room(&app).finish.as_ref().unwrap().stage,
        crate::app::design_review::room::FinishStage::ConfirmApprove
    ));
    press(&mut app, KeyCode::Enter);

    assert_eq!(room(&app).verdict.as_deref(), Some("approved"));
    let status = |uid: &str| -> String {
        sql(
            &db_file,
            &format!("SELECT status FROM design_decisions WHERE uid = '{uid}'"),
            |r| r.get(0),
        )
    };
    assert_eq!(
        status("u1"),
        "superseded",
        "superseded decisions are untouched"
    );
    assert_eq!(status("u2"), "accepted");
    assert_eq!(status("u3"), "accepted");
    let (verdict, finished): (String, Option<String>) = sql(
        &db_file,
        "SELECT verdict, finished_at FROM design_review_rounds",
        |r| Ok((r.get(0)?, r.get(1)?)),
    );
    assert_eq!(verdict, "approved");
    assert!(finished.is_some());

    // Reopen: read-only, with the verdict in the header.
    press(&mut app, KeyCode::Esc);
    open(&mut app);
    assert!(room(&app).read_only());
    let before = item(&app, "Brief").state.clone();
    press(&mut app, KeyCode::Char('c'));
    assert!(
        last_toast(&app).contains("read-only"),
        "{}",
        last_toast(&app)
    );
    assert!(room(&app).input.is_none());
    press(&mut app, KeyCode::Char('a'));
    press(&mut app, KeyCode::Char('f'));
    assert!(room(&app).finish.is_none());
    assert_eq!(item(&app, "Brief").state, before);

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
    assert!(screen.contains("Approved"), "{screen}");
}
