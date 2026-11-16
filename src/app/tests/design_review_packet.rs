//! Design Review Phase 2b: packet preparation, the artifact cache and the
//! ledger. Staff runs go through a fake runner; no harness is launched.

use super::support::*;
use crate::app::design_review::packet::{
    PacketStatus, SectionState, StaffCall, StaffReply, StaffRunFn,
};
use crate::app::*;
use crate::db::design_review::{DesignDecision, UsageRecord};
use crate::headless::HeadlessUsage;
use crate::token_cost::ledger::{Agent, Phase, Provenance, Task};
use crate::traits::{MockTmuxOps, MockWorktreeOps};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::{NamedTempFile, TempDir};

const VALID_DIAGRAM: &str = "```mermaid\nflowchart TD\n    A[Caller] --> B[Store]\n```\nTwo boxes.";
const INVALID_DIAGRAM: &str = "```mermaid\nnot a diagram at all\n```";
const RETRY_MARK: &str = "rejected by the renderer";

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?}: {status:?}");
}

/// A repo on `my-feat` with one commit over `main`: a new module and a
/// manifest change.
fn repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let p = dir.path();
    git(p, &["init", "-q", "-b", "main"]);
    std::fs::create_dir_all(p.join("src")).unwrap();
    std::fs::write(p.join("src/lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(
        p.join("Cargo.toml"),
        "[package]\nname = \"x\"\n\n[dependencies]\n",
    )
    .unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "base"]);
    git(p, &["checkout", "-q", "-b", "my-feat"]);
    std::fs::create_dir_all(p.join("src/store")).unwrap();
    std::fs::write(p.join("src/store/mod.rs"), "pub fn put() {}\n").unwrap();
    std::fs::write(
        p.join("Cargo.toml"),
        "[package]\nname = \"x\"\n\n[dependencies]\nserde = \"1\"\n",
    )
    .unwrap();
    git(p, &["add", "."]);
    git(p, &["commit", "-q", "-m", "Add the store module"]);
    dir
}

#[derive(Debug, Clone)]
struct Call {
    task: Task,
    model: String,
    read_only: bool,
    prompt: String,
}

type Replies = Arc<dyn Fn(&Call) -> Result<StaffReply, String> + Send + Sync>;

struct Fixture {
    dir: TempDir,
    _db: NamedTempFile,
    app: App,
    calls: Arc<Mutex<Vec<Call>>>,
    hold: Arc<AtomicBool>,
}

fn text(reply: &str) -> Result<StaffReply, String> {
    Ok(StaffReply {
        text: reply.to_string(),
        usage: None,
    })
}

fn default_reply(call: &Call) -> Result<StaffReply, String> {
    match call.task {
        Task::Brief => Ok(StaffReply {
            text: "- Adds a store module.".into(),
            usage: Some(HeadlessUsage {
                input_tokens: Some(900),
                output_tokens: Some(40),
                cached_tokens: Some(0),
                total_tokens: None,
            }),
        }),
        Task::Architecture | Task::KeyFlow => text(VALID_DIAGRAM),
        Task::Fit => text("- Fits src/."),
        Task::Unexplained => text("None."),
        Task::PlanDrift => text("No drift."),
        _ => Err("unexpected task".into()),
    }
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
        let db = NamedTempFile::new().unwrap();
        app.db = Some(crate::db::AmfDb::open(db.path()).unwrap());
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
        Self {
            dir,
            _db: db,
            app,
            calls,
            hold,
        }
    }

    fn prepare(&mut self, round: u32) -> PacketStatus {
        self.app.prepare_design_review_packet(0, 0, round);
        self.settle()
    }

    fn settle(&mut self) -> PacketStatus {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            self.app.poll_design_review_bg();
            if !self.app.design_review_packet.pending() {
                break;
            }
            assert!(Instant::now() < deadline, "packet never settled");
            std::thread::sleep(Duration::from_millis(5));
        }
        self.app.design_review_packet_status("feat-1")
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    fn calls_for(&self, task: Task) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|c| c.task == task)
            .collect()
    }

    fn usage(&self) -> Vec<UsageRecord> {
        self.app
            .db
            .as_ref()
            .unwrap()
            .list_design_review_usage("feat-1")
            .unwrap()
    }

    fn artifact(&self, section: &str) -> Option<crate::db::design_review::Artifact> {
        let db = self.app.db.as_ref().unwrap();
        let latest = db
            .latest_design_review_artifact("feat-1", section)
            .unwrap()?;
        db.design_review_artifact_by_id(latest.id).unwrap()
    }
}

fn states(status: &PacketStatus) -> Vec<(&'static str, SectionState)> {
    status.sections.clone()
}

#[test]
fn first_preparation_runs_each_section_once_and_ledgers_every_call() {
    let mut fx = Fixture::new();
    let status = fx.prepare(1);
    assert!(status.is_settled());
    for section in ["brief", "architecture", "key-flow", "fit", "unexplained"] {
        assert_eq!(
            status.state(section),
            Some(&SectionState::Done),
            "{section}"
        );
    }
    assert!(matches!(
        status.state("plan-drift"),
        Some(SectionState::Skipped(_))
    ));
    assert_eq!(
        fx.calls().len(),
        5,
        "plan drift does not run without a plan"
    );
    assert!(fx.calls_for(Task::PlanDrift).is_empty());
    for call in fx.calls() {
        assert_eq!(call.model, "haiku");
        assert_eq!(
            call.read_only,
            matches!(call.task, Task::Architecture | Task::KeyFlow | Task::Fit),
            "{:?}",
            call.task
        );
    }

    let usage = fx.usage();
    assert_eq!(usage.len(), 5);
    for record in &usage {
        let row = &record.row;
        assert_eq!(row.phase, Phase::Prepare);
        assert_eq!(row.harness, "claude");
        assert_eq!(row.model, "haiku");
        assert_eq!(row.round, 1);
        assert!(!row.cache_hit);
        assert!(row.duration_ms.is_some());
        let expected_agent = match row.task {
            Task::Architecture | Task::KeyFlow | Task::Fit => Agent::Cartographer,
            _ => Agent::Scribe,
        };
        assert_eq!(row.agent, expected_agent, "{:?}", row.task);
        if row.task == Task::Brief {
            assert_eq!(row.provenance, Provenance::Measured);
            assert_eq!(row.counts.input, Some(900));
            assert_eq!(row.counts.output, Some(40));
        } else {
            assert_eq!(row.provenance, Provenance::Estimated);
            assert!(row.counts.input.unwrap() > 0);
        }
        assert!(row.cost_usd > 0.0);
    }

    for (section, task) in [
        ("brief", Task::Brief),
        ("architecture", Task::Architecture),
        ("key-flow", Task::KeyFlow),
        ("fit", Task::Fit),
        ("unexplained", Task::Unexplained),
    ] {
        let artifact = fx.artifact(section).unwrap();
        assert_eq!(artifact.format, "markdown");
        let row = usage
            .iter()
            .find(|r| Some(r.id) == artifact.usage_id)
            .unwrap();
        assert_eq!(row.row.task, task);
    }
    let diagram = fx.artifact("architecture").unwrap().body;
    assert_eq!(diagram.matches("```mermaid").count(), 1);
    assert!(diagram.contains("A[Caller] --> B[Store]"));
    assert!(!diagram.contains("Two boxes"), "only the diagram is stored");
    // The detectors and the commit made it into the prompts.
    let fit = &fx.calls_for(Task::Fit)[0].prompt;
    assert!(
        fit.contains("New module or directory src/store"),
        "detector hit (base dirs from the base commit) in the fit context: {fit}"
    );
    assert!(
        !fit.contains("New module or directory src\n"),
        "src existed at base"
    );
    assert!(
        fx.calls_for(Task::Brief)[0]
            .prompt
            .contains("Add the store module")
    );
    assert_eq!(fx.artifact("plan-drift"), None);
}

#[test]
fn cache_hit_writes_a_zero_spend_row_and_launches_nothing() {
    let mut fx = Fixture::new();
    fx.prepare(1);
    let first = fx.usage();
    let status = fx.prepare(2);

    assert_eq!(fx.calls().len(), 5, "no new staff run");
    for section in ["brief", "architecture", "key-flow", "fit", "unexplained"] {
        assert_eq!(
            status.state(section),
            Some(&SectionState::Cached),
            "{section}"
        );
    }
    let usage = fx.usage();
    assert_eq!(usage.len(), 10);
    for record in &usage[5..] {
        let row = &record.row;
        assert!(row.cache_hit);
        assert_eq!(row.cost_usd, 0.0);
        assert!(row.counts.is_all_unavailable());
        assert_eq!(row.round, 2);
        let original = first.iter().find(|r| r.row.task == row.task).unwrap();
        assert_eq!(row.saved_usd, Some(original.row.cost_usd));
        assert_eq!(row.model, "haiku");
    }
}

#[test]
fn a_new_decision_or_a_prompt_override_reruns_only_affected_sections() {
    let mut fx = Fixture::new();
    fx.prepare(1);
    assert_eq!(fx.calls().len(), 5);

    // Every section shows the decisions, so a new one reruns them all.
    let db = fx.app.db.as_ref().unwrap();
    db.insert_design_decision_if_absent(&DesignDecision {
        uid: "u1".into(),
        feature_id: "feat-1".into(),
        project_id: "proj-1".into(),
        seq: 1,
        kind: "architecture".into(),
        title: "Keep the store in its own module".into(),
        why: "isolation".into(),
        rejected: vec![],
        ask: None,
        supersedes: None,
        paths: vec!["src/store".into()],
        head: None,
        session_id: None,
        harness: "claude".into(),
        source: "agent".into(),
        status: "open".into(),
        created_at: "2026-11-16T00:00:00Z".into(),
    })
    .unwrap();
    fx.prepare(1);
    assert_eq!(fx.calls().len(), 10);
    assert!(
        fx.calls_for(Task::Brief)[1]
            .prompt
            .contains("Keep the store in its own module")
    );

    // An override of one template reruns that section only.
    fx.app
        .db
        .as_ref()
        .unwrap()
        .upsert_prompt_override(
            "design_review.fit",
            &crate::db::prompt_overrides::OverrideScope::Global,
            None,
            "Fit, tersely.\n\n{{context}}",
        )
        .unwrap();
    let status = fx.prepare(1);
    assert_eq!(fx.calls().len(), 11);
    let fit_calls = fx.calls_for(Task::Fit);
    assert_eq!(fit_calls.len(), 3);
    assert!(fit_calls[2].prompt.starts_with("Fit, tersely."));
    assert_eq!(status.state("fit"), Some(&SectionState::Done));
    for section in ["brief", "architecture", "key-flow", "unexplained"] {
        assert_eq!(
            status.state(section),
            Some(&SectionState::Cached),
            "{section}"
        );
    }
}

#[test]
fn an_invalid_diagram_is_retried_once_then_falls_back_to_a_list() {
    let replies: Replies = Arc::new(|call: &Call| match call.task {
        // Never valid: retry, then the list fallback.
        Task::Architecture => text(INVALID_DIAGRAM),
        // Valid on the retry.
        Task::KeyFlow if call.prompt.contains(RETRY_MARK) => text(VALID_DIAGRAM),
        Task::KeyFlow => text(INVALID_DIAGRAM),
        _ => default_reply(call),
    });
    let mut fx = Fixture::with_replies(replies);
    let status = fx.prepare(1);

    let arch = fx.calls_for(Task::Architecture);
    assert_eq!(arch.len(), 2, "one retry, no more");
    assert!(!arch[0].prompt.contains(RETRY_MARK));
    assert!(arch[1].prompt.contains(RETRY_MARK));
    assert_eq!(status.state("architecture"), Some(&SectionState::Done));
    let body = fx.artifact("architecture").unwrap().body;
    assert!(
        !body.contains("```mermaid"),
        "fallback is a plain list: {body}"
    );

    assert_eq!(fx.calls_for(Task::KeyFlow).len(), 2);
    let flow = fx.artifact("key-flow").unwrap();
    assert!(flow.body.starts_with("```mermaid\n"));

    // Each attempt is its own ledger row; the artifact points at the last.
    let usage = fx.usage();
    let arch_rows: Vec<_> = usage
        .iter()
        .filter(|r| r.row.task == Task::Architecture)
        .collect();
    assert_eq!(arch_rows.len(), 2);
    assert_eq!(
        fx.artifact("architecture").unwrap().usage_id,
        Some(arch_rows[1].id)
    );
    assert_eq!(usage.len(), 7);
}

#[test]
fn a_failed_run_leaves_no_artifact_but_a_ledger_row() {
    let replies: Replies = Arc::new(|call: &Call| match call.task {
        Task::Fit => Err("Claude failed: boom".into()),
        _ => default_reply(call),
    });
    let mut fx = Fixture::with_replies(replies);
    let status = fx.prepare(1);
    assert_eq!(
        status.state("fit"),
        Some(&SectionState::Failed("Claude failed: boom".into()))
    );
    assert_eq!(fx.artifact("fit"), None);
    let fit_rows: Vec<_> = fx
        .usage()
        .into_iter()
        .filter(|r| r.row.task == Task::Fit)
        .collect();
    assert_eq!(fit_rows.len(), 1);
    assert!(fit_rows[0].row.counts.is_all_unavailable());
    assert_eq!(fit_rows[0].row.provenance, Provenance::Estimated);

    // A failure is not cached: the next preparation runs it again.
    fx.prepare(1);
    assert_eq!(fx.calls_for(Task::Fit).len(), 2);
}

#[test]
fn a_stale_result_is_recorded_but_dropped() {
    let mut fx = Fixture::new();
    fx.hold.store(true, Ordering::SeqCst);
    fx.app.prepare_design_review_packet(0, 0, 1);
    let deadline = Instant::now() + Duration::from_secs(20);
    while fx.app.design_review_packet.in_flight_for_test() == 0 {
        fx.app.poll_design_review_bg();
        assert!(Instant::now() < deadline, "round 1 never started a run");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!fx.app.design_review_packet.gathering_for_test());
    let round_one_runs = fx.app.design_review_packet.in_flight_for_test();

    // Round 2 supersedes round 1 while its runs are still out.
    fx.app.prepare_design_review_packet(0, 0, 2);
    fx.hold.store(false, Ordering::SeqCst);
    let status = fx.settle();

    for section in ["brief", "architecture", "key-flow", "fit", "unexplained"] {
        assert_eq!(
            status.state(section),
            Some(&SectionState::Done),
            "{section}"
        );
    }
    let usage = fx.usage();
    let stale = usage.iter().filter(|r| r.row.round == 1).count();
    assert_eq!(stale, round_one_runs, "the stale spend is still ledgered");
    for section in ["brief", "architecture", "key-flow", "fit", "unexplained"] {
        let artifact = fx.artifact(section).unwrap();
        let row = usage
            .iter()
            .find(|r| Some(r.id) == artifact.usage_id)
            .unwrap();
        assert_eq!(row.row.round, 2, "{section} came from the live round");
    }
    assert_eq!(fx.calls().len(), round_one_runs + 5);

    // Another feature's status is empty; cancelling drops the packet.
    assert_eq!(
        fx.app.design_review_packet_status("other"),
        PacketStatus::default()
    );
    fx.app.cancel_design_review_packet("feat-1");
    assert!(
        fx.app
            .design_review_packet_status("feat-1")
            .sections
            .is_empty()
    );
}

#[test]
fn plan_drift_runs_from_the_brief_when_a_plan_exists() {
    let mut fx = Fixture::new();
    std::fs::write(
        fx.dir.path().join("AMF_PLAN.md"),
        "# Plan\n\nAdd a store module and a cache.\n",
    )
    .unwrap();
    let status = fx.prepare(1);
    assert_eq!(status.state("plan-drift"), Some(&SectionState::Done));
    let drift = fx.calls_for(Task::PlanDrift);
    assert_eq!(drift.len(), 1);
    assert!(drift[0].prompt.contains("Add a store module and a cache."));
    assert!(
        drift[0].prompt.contains("- Adds a store module."),
        "the brief"
    );
    assert_eq!(fx.artifact("plan-drift").unwrap().body, "No drift.\n");
    assert_eq!(
        states(&status).iter().map(|(k, _)| *k).collect::<Vec<_>>(),
        [
            "brief",
            "architecture",
            "key-flow",
            "fit",
            "unexplained",
            "plan-drift"
        ]
    );

    // Cached brief, cached plan drift.
    let status = fx.prepare(2);
    assert_eq!(status.state("plan-drift"), Some(&SectionState::Cached));
    assert_eq!(fx.calls().len(), 6);
}
