//! Design Review: the room wired to the packet (preparation on open,
//! cancellation on close, section states, artifact arrival) and the question
//! ladder's presenter levels (L1, L2). Staff runs go through a fake runner.

use super::design_review_packet::repo;
use super::support::*;
use crate::app::design_review::packet::{SectionState, StaffCall, StaffReply, StaffRunFn};
use crate::app::design_review::room::DesignReviewRoom;
use crate::app::*;
use crate::design_review::outbox;
use crate::design_review::record::{DecisionKind, DecisionRecord, OutboxLine};
use crate::project::{SessionKind, VibeMode};
use crate::token_cost::ledger::{Agent, Phase, Task};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::{NamedTempFile, TempDir};

const QUESTION: &str = "Why a store module?";
const ANSWER: &str = "The diff shows a put() entry point; the motive is not recorded.";

#[derive(Debug, Clone)]
struct Call {
    task: Task,
    model: String,
    read_only: bool,
    prompt: String,
}

type Replies = Arc<dyn Fn(&Call) -> Result<StaffReply, String> + Send + Sync>;

fn reply(text: &str) -> Result<StaffReply, String> {
    Ok(StaffReply {
        text: text.to_string(),
        usage: None,
    })
}

fn default_reply(call: &Call) -> Result<StaffReply, String> {
    match call.task {
        Task::Brief => reply("- Adds a store module."),
        Task::Architecture | Task::KeyFlow => {
            reply("```mermaid\nflowchart TD\n    A[Caller] --> B[Store]\n```")
        }
        Task::Unexplained => reply("- `Cargo.toml`: serde added; not recorded."),
        Task::QuestionL1 | Task::QuestionL2 => reply(ANSWER),
        _ => reply("- Fits."),
    }
}

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

struct Fixture {
    _dir: TempDir,
    db_file: NamedTempFile,
    app: App,
    calls: Arc<Mutex<Vec<Call>>>,
    hold: Arc<AtomicBool>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_replies(Arc::new(default_reply))
    }

    fn with_replies(replies: Replies) -> Self {
        let dir = repo();
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
        let line = OutboxLine::Decision(DecisionRecord {
            uid: "u1".into(),
            seq: 1,
            kind: DecisionKind::Architecture,
            title: "Store module".into(),
            why: "keeps writes in one place".into(),
            rejected: vec![],
            ask: None,
            supersedes: None,
            paths: vec!["src/store/mod.rs".into()],
            dirty: Default::default(),
            head: None,
            session_id: None,
            harness: "claude".into(),
            ts: "2026-11-13T00:00:00Z".into(),
        });
        outbox::append(&outbox::outbox_path(dir.path()), &line).unwrap();

        let calls: Arc<Mutex<Vec<Call>>> = Arc::default();
        let hold = Arc::new(AtomicBool::new(false));
        let (log, gate) = (Arc::clone(&calls), Arc::clone(&hold));
        let runner: StaffRunFn = Arc::new(move |call: &StaffCall<'_>| {
            let call = Call {
                task: call.task,
                model: call.model.to_string(),
                read_only: call.read_only,
                prompt: call.prompt.to_string(),
            };
            log.lock().unwrap().push(call.clone());
            while gate.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
            replies(&call)
        });
        app.design_review_packet.set_runner_for_test(runner);
        app.mode = AppMode::Viewing(view());
        Self {
            _dir: dir,
            db_file,
            app,
            calls,
            hold,
        }
    }

    fn open(&mut self) {
        assert!(self.app.open_design_review(0, 0, view(), None));
    }

    fn settle(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.app.poll_design_review_bg();
            if !self.app.design_review_packet.pending() {
                break;
            }
            assert!(Instant::now() < deadline, "runs never settled");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn room(&self) -> &DesignReviewRoom {
        match &self.app.mode {
            AppMode::DesignReview(room) => room,
            _ => panic!("expected the review room"),
        }
    }

    fn press(&mut self, code: KeyCode) {
        crate::handlers::handle_key(&mut self.app, KeyEvent::new(code, KeyModifiers::NONE), 40)
            .unwrap();
    }

    fn select_kind(&mut self, kind: &str) {
        while self.room().selected_item().unwrap().kind != kind {
            let before = self.room().selected;
            self.press(KeyCode::Char('j'));
            assert_ne!(self.room().selected, before, "no {kind} item");
        }
    }

    /// Open, prepare the packet, select d1 and ask it a question (L0).
    fn asked(&mut self) -> String {
        self.open();
        self.settle();
        self.select_kind("decision");
        self.press(KeyCode::Char('s'));
        for c in QUESTION.chars() {
            self.press(KeyCode::Char(c));
        }
        self.press(KeyCode::Enter);
        self.room().selected_item().unwrap().id.clone()
    }

    fn calls_for(&self, task: Task) -> Vec<Call> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.task == task)
            .cloned()
            .collect()
    }

    fn turns(&self, item_id: &str) -> Vec<crate::db::design_review::DesignReviewTurn> {
        self.app
            .db
            .as_ref()
            .unwrap()
            .design_review_turns(item_id)
            .unwrap()
    }

    fn discuss_rows(&self) -> Vec<crate::db::design_review::UsageRecord> {
        self.app
            .db
            .as_ref()
            .unwrap()
            .list_design_review_usage("feat-1")
            .unwrap()
            .into_iter()
            .filter(|r| r.row.phase == Phase::Discuss)
            .collect()
    }

    fn last_toast(&self) -> String {
        self.app
            .toasts
            .last()
            .map(|t| t.message.clone())
            .unwrap_or_default()
    }
}

// ── Wiring the room to the packet ─────────────────────────

#[test]
fn opening_the_room_prepares_the_packet_and_closing_cancels_it() {
    let mut fx = Fixture::new();
    fx.hold.store(true, Ordering::SeqCst);
    fx.open();
    let status = fx.app.design_review_packet_status("feat-1");
    assert_eq!(status.sections.len(), 6, "preparation started");
    assert_eq!(fx.room().packet, status, "the room holds a copy");
    assert!(
        fx.room()
            .packet
            .sections
            .iter()
            .all(|(_, s)| *s == SectionState::Running)
    );

    fx.press(KeyCode::Esc);
    assert!(matches!(fx.app.mode, AppMode::Viewing(_)));
    assert!(
        fx.app
            .design_review_packet_status("feat-1")
            .sections
            .is_empty(),
        "closing cancels the packet"
    );
    fx.hold.store(false, Ordering::SeqCst);
    fx.settle();
}

#[test]
fn section_state_follows_the_packet_and_an_arriving_artifact_refreshes_content() {
    let mut fx = Fixture::new();
    fx.hold.store(true, Ordering::SeqCst);
    fx.open();
    assert_eq!(fx.room().selected_item().unwrap().kind, "brief");
    assert!(
        fx.room()
            .content
            .starts_with("Not prepared yet. The packet is being prepared"),
        "{}",
        fx.room().content
    );
    assert!(
        !fx.room().items.iter().any(|i| i.kind == "unexplained"),
        "no unexplained item before its artifact"
    );

    fx.hold.store(false, Ordering::SeqCst);
    fx.settle();
    let brief = fx.room().selected_item().unwrap().clone();
    assert_eq!(brief.kind, "brief", "the selection stays put");
    assert_eq!(fx.room().section_state(&brief), Some(&SectionState::Done));
    assert_eq!(fx.room().content, "- Adds a store module.\n");
    assert!(
        fx.room().items.iter().any(|i| i.kind == "unexplained"),
        "the unexplained item appears when its artifact arrives"
    );
}

#[test]
fn a_failed_section_shows_its_reason_in_the_content_pane() {
    let mut fx = Fixture::with_replies(Arc::new(|call: &Call| match call.task {
        Task::Brief => Err("the scribe timed out".into()),
        _ => default_reply(call),
    }));
    fx.open();
    fx.settle();
    let brief = fx.room().selected_item().unwrap().clone();
    assert!(matches!(
        fx.room().section_state(&brief),
        Some(SectionState::Failed(_))
    ));
    let content = &fx.room().content;
    assert!(
        content.contains("preparation failed") && content.contains("the scribe timed out"),
        "{content}"
    );
}

// ── The question ladder above L0 ──────────────────────────

#[test]
fn escalating_needs_a_question_first() {
    let mut fx = Fixture::new();
    fx.open();
    fx.settle();
    fx.select_kind("decision");
    fx.press(KeyCode::Char('1'));
    assert!(fx.room().confirm.is_none());
    assert_eq!(
        fx.last_toast(),
        "Ask a question first (s), then escalate it"
    );
}

#[test]
fn l1_shows_the_estimate_writes_a_pending_turn_and_one_ledger_row() {
    let mut fx = Fixture::new();
    let item_id = fx.asked();
    fx.press(KeyCode::Char('1'));
    let confirm = fx.room().confirm.clone().expect("a priced escalation");
    assert!(
        confirm
            .summary
            .starts_with("Ask the presenter (L1): sonnet on Claude, no tools"),
        "{}",
        confirm.summary
    );
    assert!(confirm.summary.contains("est. $"), "{}", confirm.summary);
    assert!(
        fx.calls_for(Task::QuestionL1).is_empty(),
        "nothing runs yet"
    );

    fx.hold.store(true, Ordering::SeqCst);
    fx.press(KeyCode::Char('y'));
    assert!(fx.room().confirm.is_none());
    let pending = fx.room().turns.last().unwrap().clone();
    assert_eq!(pending.role, "presenter");
    assert_eq!(pending.level.as_deref(), Some("L1"));
    assert_eq!(pending.status, "pending");
    assert_eq!(pending.usage_id, None);

    fx.hold.store(false, Ordering::SeqCst);
    fx.settle();
    let calls = fx.calls_for(Task::QuestionL1);
    assert_eq!(calls.len(), 1);
    assert!(!calls[0].read_only, "L1 runs with no tools");
    assert_eq!(calls[0].model, "sonnet");
    assert!(calls[0].prompt.contains(QUESTION));
    assert!(calls[0].prompt.contains("keeps writes in one place"));
    assert!(calls[0].prompt.contains("src/store/mod.rs"), "linked hunk");
    assert!(calls[0].prompt.contains("not recorded"));

    let rows = fx.discuss_rows();
    assert_eq!(rows.len(), 1, "one ledger row per presenter run");
    let row = &rows[0].row;
    assert_eq!(row.task, Task::QuestionL1);
    assert_eq!(row.agent, Agent::Presenter);
    assert_eq!(row.agenda_item.as_deref(), Some(item_id.as_str()));
    assert_eq!(row.round, 1);

    let filled = fx.turns(&item_id).pop().unwrap();
    assert_eq!(filled.id, pending.id);
    assert_eq!(filled.status, "done");
    assert_eq!(filled.usage_id, Some(rows[0].id));
    assert!(
        filled
            .body
            .starts_with("Answered by the presenter (sonnet on Claude, no tools)")
    );
    assert!(filled.body.ends_with(ANSWER));
    assert_eq!(
        fx.room().turns.last().unwrap(),
        &filled,
        "the open thread shows it"
    );

    // The Usage tab groups the row under the item's title.
    fx.app.design_review_reload_usage();
    let labelled = fx
        .room()
        .usage
        .rows
        .iter()
        .find(|r| r.task == "question-L1")
        .unwrap()
        .item
        .clone();
    assert_eq!(labelled.as_deref(), Some("d1 Store module"));
}

#[test]
fn l2_runs_the_deep_prompt_read_only() {
    let mut fx = Fixture::new();
    let item_id = fx.asked();
    fx.press(KeyCode::Char('2'));
    assert!(
        fx.room()
            .confirm
            .as_ref()
            .unwrap()
            .summary
            .contains("read-only")
    );
    fx.press(KeyCode::Enter);
    fx.settle();
    let calls = fx.calls_for(Task::QuestionL2);
    assert_eq!(calls.len(), 1);
    assert!(calls[0].read_only, "L2 explores read-only");
    assert!(calls[0].prompt.contains("read-only tools"));
    let rows = fx.discuss_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].row.task, Task::QuestionL2);
    let turn = fx.turns(&item_id).pop().unwrap();
    assert_eq!(turn.level.as_deref(), Some("L2"));
    assert_eq!(turn.status, "done");
}

#[test]
fn esc_drops_a_priced_escalation_without_running_it() {
    let mut fx = Fixture::new();
    let item_id = fx.asked();
    let before = fx.turns(&item_id).len();
    fx.press(KeyCode::Char('1'));
    fx.press(KeyCode::Esc);
    assert!(fx.room().confirm.is_none());
    assert!(
        matches!(fx.app.mode, AppMode::DesignReview(_)),
        "Esc only cancels"
    );
    fx.settle();
    assert!(fx.calls_for(Task::QuestionL1).is_empty());
    assert_eq!(fx.turns(&item_id).len(), before);
}

#[test]
fn an_answer_is_kept_after_the_room_closes() {
    let mut fx = Fixture::new();
    let item_id = fx.asked();
    fx.press(KeyCode::Char('1'));
    fx.hold.store(true, Ordering::SeqCst);
    fx.press(KeyCode::Char('y'));
    fx.press(KeyCode::Esc);
    assert!(matches!(fx.app.mode, AppMode::Viewing(_)));
    fx.hold.store(false, Ordering::SeqCst);
    fx.settle();
    let turn = fx.turns(&item_id).pop().unwrap();
    assert_eq!(turn.status, "done");
    assert!(turn.body.ends_with(ANSWER));
    assert_eq!(fx.discuss_rows().len(), 1);
}

#[test]
fn an_answer_for_a_deleted_item_is_dropped_but_still_ledgered() {
    let mut fx = Fixture::new();
    let item_id = fx.asked();
    fx.press(KeyCode::Char('1'));
    fx.hold.store(true, Ordering::SeqCst);
    fx.press(KeyCode::Char('y'));
    let conn = rusqlite::Connection::open(fx.db_file.path()).unwrap();
    conn.execute(
        "DELETE FROM design_review_items WHERE id = ?1",
        rusqlite::params![item_id],
    )
    .unwrap();
    fx.hold.store(false, Ordering::SeqCst);
    fx.settle();
    let turn = fx.turns(&item_id).pop().unwrap();
    assert_eq!(turn.status, "pending", "never filled in");
    assert_eq!(fx.discuss_rows().len(), 1, "the spend is still recorded");
}

#[test]
fn a_failed_presenter_run_marks_the_turn_failed_with_a_short_reason() {
    let mut fx = Fixture::with_replies(Arc::new(|call: &Call| match call.task {
        Task::QuestionL1 => Err("Claude failed: rate limited\nlong trace".into()),
        _ => default_reply(call),
    }));
    let item_id = fx.asked();
    fx.press(KeyCode::Char('1'));
    fx.press(KeyCode::Char('y'));
    fx.settle();
    let turn = fx.turns(&item_id).pop().unwrap();
    assert_eq!(turn.status, "failed");
    assert_eq!(
        turn.body,
        "The presenter could not answer: Claude failed: rate limited"
    );
    let rows = fx.discuss_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(turn.usage_id, Some(rows[0].id));
    assert!(rows[0].row.counts.is_all_unavailable());
}

#[test]
fn a_second_escalation_waits_for_the_pending_answer() {
    let mut fx = Fixture::new();
    fx.asked();
    fx.press(KeyCode::Char('1'));
    fx.hold.store(true, Ordering::SeqCst);
    fx.press(KeyCode::Char('y'));
    fx.press(KeyCode::Char('2'));
    assert!(fx.room().confirm.is_none());
    assert_eq!(
        fx.last_toast(),
        "The presenter is still answering this item"
    );
    fx.hold.store(false, Ordering::SeqCst);
    fx.settle();
}
