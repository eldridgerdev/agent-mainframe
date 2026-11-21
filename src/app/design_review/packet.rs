//! Preparing the Design Review packet (plan Phase 2b,
//! `docs/development/features/design-review.md`).
//!
//! [`App::prepare_design_review_packet`] gathers the inputs on a worker
//! thread, then [`App::poll_design_review_bg`] builds each section's context,
//! reuses a cached artifact when its input hash matches, and otherwise runs a
//! staff model through `HeadlessRunner` on its own thread. Every call, and
//! every cache hit, writes one `design_review_usage` row.
//!
//! One packet is active at a time. Each preparation takes a new request id;
//! a result carrying another id (a superseded round, another feature, a
//! review closed and reopened) is stale: its spend is still written to the
//! ledger, but it never becomes an artifact or changes the status.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::Instant;

use chrono::{DateTime, Utc};

use super::packet_inputs::{self, GatheredInputs};
use super::presenter::PresenterJob;
use crate::app::App;
use crate::db::design_review::NewArtifact;
use crate::design_packet::context::{self, DecisionSummary, PacketInputs};
use crate::design_packet::detectors;
use crate::design_packet::diagram::{self, DiagramOutcome};
use crate::design_packet::staff::{self, StaffModel, StaffRole};
use crate::headless::{HeadlessUsage, ModelSel, ReasoningLevel};
use crate::project::AgentKind;
use crate::prompts::{PromptContext, PromptId};
use crate::token_cost::ledger::{
    Agent, LedgerContext, LedgerRow, Phase, Task, estimate_tokens_from_bytes,
};

/// The packet's sections in meeting order. Their `as_str()` values are the
/// `design_review_artifacts.section` keys the review room reads.
pub const PACKET_SECTIONS: [Task; 6] = [
    Task::Brief,
    Task::Architecture,
    Task::KeyFlow,
    Task::Fit,
    Task::Unexplained,
    Task::PlanDrift,
];

/// `design_review_artifacts.format` of every packet section. Diagram
/// sections hold one fenced ```mermaid block (source, never rendered text);
/// their list fallback is plain Markdown.
pub const ARTIFACT_FORMAT: &str = "markdown";

/// Most staff runs one packet keeps in flight, before the concurrency limit
/// narrows it further.
pub const MAX_PARALLEL_RUNS: usize = 3;

/// Where one section stands, for the review room's progress display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionState {
    /// A stored artifact matched the inputs; nothing ran.
    Cached,
    /// Gathering inputs, queued, or a staff run is in flight.
    Running,
    /// A staff run produced the artifact.
    Done,
    /// No artifact; the reason is also in the debug log.
    Failed(String),
    /// Not prepared, for a reason that is not a failure (no `AMF_PLAN.md`,
    /// no staff model for the harness).
    Skipped(String),
}

impl SectionState {
    pub fn is_settled(&self) -> bool {
        !matches!(self, SectionState::Running)
    }
}

/// The active packet's sections and their states, in [`PACKET_SECTIONS`]
/// order. Empty when no packet was prepared for the feature in this run of
/// AMF.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PacketStatus {
    pub sections: Vec<(&'static str, SectionState)>,
}

impl PacketStatus {
    pub fn state(&self, section: &str) -> Option<&SectionState> {
        self.sections
            .iter()
            .find(|(key, _)| *key == section)
            .map(|(_, state)| state)
    }

    /// True when every section has finished one way or another.
    #[cfg(test)]
    pub fn is_settled(&self) -> bool {
        self.sections.iter().all(|(_, state)| state.is_settled())
    }
}

/// How a section is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SectionSpec {
    pub role: StaffRole,
    pub prompt: PromptId,
    /// Mermaid output, vetted with one retry and a list fallback.
    pub diagram: bool,
}

pub(crate) fn section_spec(task: Task) -> SectionSpec {
    let (role, prompt, diagram) = match task {
        Task::Architecture => (StaffRole::Cartographer, PromptId::DesignArchitecture, true),
        Task::KeyFlow => (StaffRole::Cartographer, PromptId::DesignKeyFlow, true),
        Task::Fit => (StaffRole::Cartographer, PromptId::DesignFit, false),
        Task::Unexplained => (StaffRole::Scribe, PromptId::DesignUnexplained, false),
        Task::PlanDrift => (StaffRole::Scribe, PromptId::DesignPlanDrift, false),
        _ => (StaffRole::Scribe, PromptId::DesignBrief, false),
    };
    SectionSpec {
        role,
        prompt,
        diagram,
    }
}

pub(super) fn agent_for(role: StaffRole) -> Agent {
    match role {
        StaffRole::Scribe => Agent::Scribe,
        StaffRole::Cartographer => Agent::Cartographer,
        StaffRole::Presenter => Agent::Presenter,
    }
}

/// The section's prompt: the template with `{{context}}` and, for diagrams,
/// `{{renderer_limits}}` filled in.
pub(crate) fn render_section_prompt(
    spec: &SectionSpec,
    template: &str,
    context_text: &str,
    renderer_limits: &str,
) -> String {
    let mut ctx = PromptContext::new().with("context", context_text);
    if spec.diagram {
        ctx.insert("renderer_limits", renderer_limits);
    }
    crate::prompts::render_template(template, &ctx)
}

/// The stored body of a diagram section.
pub(crate) fn diagram_body(outcome: DiagramOutcome) -> String {
    match outcome {
        DiagramOutcome::Valid(src) => format!("```mermaid\n{}\n```\n", src.trim_end()),
        DiagramOutcome::Fallback(list) => list,
    }
}

/// The reasoning level a harness will actually be sent (see
/// `headless::reasoning_args`): an unsupported level is dropped.
pub(super) fn effective_reasoning(staff: &StaffModel) -> Option<ReasoningLevel> {
    staff
        .reasoning
        .filter(|level| ReasoningLevel::supported_for(&staff.harness).contains(level))
}

/// One staff call, as handed to the runner.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StaffCall<'a> {
    pub task: Task,
    pub harness: &'a AgentKind,
    pub model: &'a str,
    pub reasoning: Option<ReasoningLevel>,
    /// Cartographer sections may explore read-only; scribe sections run
    /// with no tools.
    pub read_only: bool,
    pub workdir: &'a Path,
    pub prompt: &'a str,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StaffReply {
    pub text: String,
    /// What the harness reported; `None` means the ledger estimates.
    pub usage: Option<HeadlessUsage>,
}

pub(crate) type StaffRunFn =
    Arc<dyn Fn(&StaffCall<'_>) -> Result<StaffReply, String> + Send + Sync>;

/// The production runner. `HeadlessRunner` takes a headless lease for the
/// run, so it counts toward the concurrency limit. Under `cfg(test)` it
/// refuses, so a test that forgets to install a fake never launches a paid
/// harness.
fn headless_runner() -> StaffRunFn {
    Arc::new(|call: &StaffCall<'_>| {
        if cfg!(test) {
            return Err("headless runs are disabled in tests".to_string());
        }
        let model = (!call.model.is_empty()).then_some(call.model);
        let sel = ModelSel::new(model, call.reasoning);
        crate::headless::HeadlessRunner::run_with_usage(
            call.harness,
            call.workdir,
            call.prompt,
            sel,
            call.read_only,
        )
        .map(|(text, usage)| StaffReply {
            text,
            usage: Some(usage),
        })
        .map_err(|err| {
            format!(
                "{} failed ({}): {err}",
                call.harness.display_name(),
                call.task.as_str()
            )
        })
    })
}

#[derive(Debug, Clone)]
struct StaffJob {
    request: u64,
    feature_id: String,
    project_id: String,
    round: u32,
    task: Task,
    role: StaffRole,
    staff: StaffModel,
    workdir: PathBuf,
    prompt: String,
    input_hash: String,
    /// 1, or 2 for a diagram's one retry.
    attempt: u8,
    /// The first reply, kept for `diagram::accept` on the retry.
    first_response: Option<String>,
}

/// A finished staff run: the job that asked for it and what came back.
pub(crate) struct Finished<J> {
    pub job: J,
    pub result: Result<StaffReply, String>,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
}

type StaffOutcome = Finished<StaffJob>;

/// Run one staff call on its own thread through `runner` and send the
/// outcome to `tx`. The packet's sections and the presenter's answers share
/// it, so every staff run goes through the same runner (and its headless
/// lease) and is timed the same way.
pub(super) fn spawn_staff_run<J: Send + 'static>(
    runner: &StaffRunFn,
    tx: &Sender<Finished<J>>,
    job: J,
    call: for<'a> fn(&'a J) -> StaffCall<'a>,
) {
    let runner = Arc::clone(runner);
    let tx = tx.clone();
    std::thread::spawn(move || {
        let started_at = Utc::now();
        let clock = Instant::now();
        let result = runner(&call(&job));
        let duration_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        let _ = tx.send(Finished {
            job,
            result,
            started_at,
            duration_ms,
        });
    });
}

fn packet_call(job: &StaffJob) -> StaffCall<'_> {
    StaffCall {
        task: job.task,
        harness: &job.staff.harness,
        model: &job.staff.model,
        reasoning: effective_reasoning(&job.staff),
        read_only: job.role == StaffRole::Cartographer,
        workdir: &job.workdir,
        prompt: &job.prompt,
    }
}

/// The ledger row for one finished staff run: measured counts when the
/// harness reported usage, bytes/4 estimates when it reported nothing, and
/// every count unavailable for a failed call.
pub(super) fn staff_usage_row<J>(
    ctx: LedgerContext,
    harness: &AgentKind,
    prompt: &str,
    finished: &Finished<J>,
    pricing: &crate::token_tracking::TokenPricingConfig,
) -> LedgerRow {
    let (counts, provenance) = match &finished.result {
        Ok(reply) => packet_inputs::counts_for_run(
            harness,
            reply.usage.as_ref(),
            prompt,
            reply.text.as_str(),
        ),
        // Nothing is known about a failed call: every count unavailable.
        Err(_) => (
            Default::default(),
            crate::token_cost::ledger::Provenance::Estimated,
        ),
    };
    LedgerRow::from_counts(
        ctx,
        counts,
        provenance,
        Some(finished.duration_ms),
        finished.started_at,
        pricing,
    )
}

struct ActivePacket {
    request: u64,
    feature_id: String,
    project_id: String,
    round: u32,
    repo: PathBuf,
    workdir: PathBuf,
    feature_agent: AgentKind,
    gathering: bool,
    decisions: Vec<DecisionSummary>,
    inputs: Option<GatheredInputs>,
    states: Vec<(Task, SectionState)>,
    queue: VecDeque<StaffJob>,
    in_flight: usize,
    /// Plan drift is built from the brief, so it waits for it.
    plan_drift_waits_for_brief: bool,
}

impl ActivePacket {
    fn set(&mut self, task: Task, state: SectionState) {
        if let Some(slot) = self.states.iter_mut().find(|(t, _)| *t == task) {
            slot.1 = state;
        }
    }

    fn fail_unsettled(&mut self, reason: &str) {
        for (_, state) in &mut self.states {
            if !state.is_settled() {
                *state = SectionState::Failed(reason.to_string());
            }
        }
        self.queue.clear();
        self.plan_drift_waits_for_brief = false;
    }
}

/// App runtime state for packet preparation (not persisted).
pub(crate) struct PacketRuntime {
    next_request: u64,
    active: Option<ActivePacket>,
    gather_tx: Sender<(u64, Result<GatheredInputs, String>)>,
    gather_rx: Receiver<(u64, Result<GatheredInputs, String>)>,
    results_tx: Sender<StaffOutcome>,
    results_rx: Receiver<StaffOutcome>,
    /// Runs of superseded packets still to report, so their spend reaches
    /// the ledger.
    stale_in_flight: usize,
    pub(super) runner: StaffRunFn,
    /// Presenter answers (question ladder L1/L2) in flight. They are not
    /// tied to the active packet: an answer is kept after the room closes.
    pub(super) presenter_tx: Sender<Finished<PresenterJob>>,
    presenter_rx: Receiver<Finished<PresenterJob>>,
    pub(super) presenter_in_flight: usize,
}

impl Default for PacketRuntime {
    fn default() -> Self {
        let (gather_tx, gather_rx) = channel();
        let (results_tx, results_rx) = channel();
        let (presenter_tx, presenter_rx) = channel();
        Self {
            next_request: 0,
            active: None,
            gather_tx,
            gather_rx,
            results_tx,
            results_rx,
            stale_in_flight: 0,
            runner: headless_runner(),
            presenter_tx,
            presenter_rx,
            presenter_in_flight: 0,
        }
    }
}

impl PacketRuntime {
    /// True while anything is left for `poll_design_review_bg` to collect.
    pub(crate) fn pending(&self) -> bool {
        self.stale_in_flight > 0
            || self.presenter_in_flight > 0
            || self
                .active
                .as_ref()
                .is_some_and(|p| p.gathering || p.in_flight > 0 || !p.queue.is_empty())
    }

    /// A fresh request id, shared by packets and presenter runs.
    pub(super) fn next_request_id(&mut self) -> u64 {
        self.next_request += 1;
        self.next_request
    }

    fn supersede(&mut self) {
        if let Some(old) = self.active.take() {
            self.stale_in_flight += old.in_flight;
        }
    }

    #[cfg(test)]
    pub(crate) fn set_runner_for_test(&mut self, runner: StaffRunFn) {
        self.runner = runner;
    }

    #[cfg(test)]
    pub(crate) fn in_flight_for_test(&self) -> usize {
        self.active.as_ref().map_or(0, |p| p.in_flight)
    }

    #[cfg(test)]
    pub(crate) fn gathering_for_test(&self) -> bool {
        self.active.as_ref().is_some_and(|p| p.gathering)
    }
}

impl App {
    /// Prepare (or refresh) the packet for a feature: ingest its outbox,
    /// gather the diff on a worker thread, then reuse or produce each
    /// section as [`Self::poll_design_review_bg`] collects results. Calling
    /// it again, for any round or feature, supersedes the packet in flight.
    /// The review room calls it on open.
    pub(crate) fn prepare_design_review_packet(&mut self, pi: usize, fi: usize, round: u32) {
        self.ingest_design_review_outbox(pi, fi);
        let Some(project) = self.store.projects.get(pi) else {
            return;
        };
        let Some(feature) = project.features.get(fi) else {
            return;
        };
        let (project_id, repo) = (project.id.clone(), project.repo.clone());
        let (feature_id, workdir, feature_agent) = (
            feature.id.clone(),
            feature.workdir.clone(),
            feature.agent.clone(),
        );
        let Some(db) = self.db.as_ref() else {
            self.log_warn(
                "design-review",
                "cannot prepare the packet without the database".to_string(),
            );
            return;
        };
        let decisions = match db.design_decisions_for_feature(&feature_id) {
            Ok(rows) => rows
                .into_iter()
                .filter(|d| d.status != "superseded")
                .map(|d| DecisionSummary {
                    seq: u32::try_from(d.seq.max(0)).unwrap_or(u32::MAX),
                    kind: d.kind,
                    title: d.title,
                    paths: d.paths,
                })
                .collect(),
            Err(err) => {
                self.log_warn(
                    "design-review",
                    format!("cannot read decisions for {feature_id}: {err}"),
                );
                Vec::new()
            }
        };

        let runtime = &mut self.design_review_packet;
        runtime.supersede();
        let request = runtime.next_request_id();
        runtime.active = Some(ActivePacket {
            request,
            feature_id: feature_id.clone(),
            project_id,
            round,
            repo,
            workdir: workdir.clone(),
            feature_agent,
            gathering: true,
            decisions,
            inputs: None,
            states: PACKET_SECTIONS
                .iter()
                .map(|task| (*task, SectionState::Running))
                .collect(),
            queue: VecDeque::new(),
            in_flight: 0,
            plan_drift_waits_for_brief: false,
        });
        let tx = runtime.gather_tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send((request, packet_inputs::gather(&workdir)));
        });
        self.log_info(
            "design-review",
            format!("preparing packet round {round} for {feature_id}"),
        );
    }

    /// Stop tracking the feature's packet (the review room closed). Runs in
    /// flight finish and are recorded in the ledger, but are dropped.
    pub(crate) fn cancel_design_review_packet(&mut self, feature_id: &str) {
        let runtime = &mut self.design_review_packet;
        if runtime
            .active
            .as_ref()
            .is_some_and(|p| p.feature_id == feature_id)
        {
            runtime.supersede();
        }
    }

    /// The feature's packet progress, for the review room.
    pub(crate) fn design_review_packet_status(&self, feature_id: &str) -> PacketStatus {
        match self.design_review_packet.active.as_ref() {
            Some(packet) if packet.feature_id == feature_id => PacketStatus {
                sections: packet
                    .states
                    .iter()
                    .map(|(task, state)| (task.as_str(), state.clone()))
                    .collect(),
            },
            _ => PacketStatus::default(),
        }
    }

    /// Collect gathered inputs and finished staff runs, and start queued runs.
    /// Called from the main loop beside the other `poll_*_bg` calls; returns
    /// true when the status changed.
    pub fn poll_design_review_bg(&mut self) -> bool {
        let mut changed = false;
        while let Ok((request, result)) = self.design_review_packet.gather_rx.try_recv() {
            let Some(mut packet) = self.design_review_packet.active.take() else {
                continue;
            };
            if packet.request == request && packet.gathering {
                changed = true;
                packet.gathering = false;
                match result {
                    Ok(inputs) => {
                        packet.inputs = Some(inputs);
                        self.plan_packet_sections(&mut packet);
                    }
                    Err(reason) => {
                        self.log_warn("design-review", format!("packet not prepared: {reason}"));
                        packet.fail_unsettled(&reason);
                    }
                }
            }
            self.design_review_packet.active = Some(packet);
        }
        while let Ok(outcome) = self.design_review_packet.results_rx.try_recv() {
            changed |= self.finish_staff_run(outcome);
        }
        changed |= self.start_queued_staff_runs();
        while let Ok(outcome) = self.design_review_packet.presenter_rx.try_recv() {
            changed |= self.finish_presenter_run(outcome);
        }
        if changed {
            self.design_review_sync_packet();
        }
        changed
    }

    fn plan_packet_sections(&mut self, packet: &mut ActivePacket) {
        let Some(gathered) = packet.inputs.as_ref() else {
            return;
        };
        let hits = detectors::detect(&gathered.files, &gathered.base_dirs);
        let modules = packet_inputs::changed_modules(&gathered.files);
        let inputs = PacketInputs {
            files: &gathered.files,
            commit_subjects: &gathered.commit_subjects,
            changed_modules: &modules,
            decisions: &packet.decisions,
            plan_excerpt: gathered.plan_excerpt.as_deref(),
            brief: None,
            detector_hits: &hits,
        };
        let has_plan = inputs.plan_excerpt.is_some();
        let contexts = [
            (Task::Brief, context::brief_context(&inputs).0),
            (Task::Architecture, context::architecture_context(&inputs).0),
            (Task::KeyFlow, context::key_flow_context(&inputs).0),
            (Task::Fit, context::fit_context(&inputs).0),
            (Task::Unexplained, context::unexplained_context(&inputs).0),
        ];
        if has_plan {
            packet.plan_drift_waits_for_brief = true;
        } else {
            packet.set(
                Task::PlanDrift,
                SectionState::Skipped("no AMF_PLAN.md in the worktree".to_string()),
            );
        }
        for (task, context_text) in contexts {
            self.schedule_section(packet, task, &context_text);
        }
    }

    /// Reuse the cached artifact for these inputs, or queue a staff run.
    fn schedule_section(&mut self, packet: &mut ActivePacket, task: Task, context_text: &str) {
        let spec = section_spec(task);
        let Some(staff) =
            staff::resolve(spec.role, &self.config.design_review, &packet.feature_agent)
        else {
            packet.set(
                task,
                SectionState::Skipped(format!(
                    "no {} model is configured for {}",
                    spec.role.as_str(),
                    packet.feature_agent.display_name()
                )),
            );
            self.section_settled(packet, task, None);
            return;
        };
        let (template, _source) = self.resolve_headless_template(
            spec.prompt,
            &staff.harness,
            &packet.repo,
            &packet.workdir,
        );
        let limits = if spec.diagram {
            diagram::renderer_limits()
        } else {
            String::new()
        };
        let prompt = render_section_prompt(&spec, &template, context_text, &limits);
        let input_hash =
            context::input_hash(task.as_str(), context_text, &template, &limits, &staff);

        let cached = self.db.as_ref().and_then(|db| {
            db.design_review_artifact(&packet.feature_id, task.as_str(), &input_hash)
                .ok()
                .flatten()
        });
        if let Some(artifact) = cached {
            let saved = artifact.usage_id.and_then(|id| {
                self.db
                    .as_ref()
                    .and_then(|db| db.design_review_usage_record(id).ok().flatten())
                    .map(|usage| usage.row.cost_usd)
            });
            let mut row = LedgerRow::cache_hit(
                ledger_context(task, spec.role, &staff, packet.round),
                saved.unwrap_or(0.0),
                Utc::now(),
                &self.config.token_pricing,
            );
            row.saved_usd = saved;
            self.record_design_review_usage(&row, &packet.feature_id, &packet.project_id);
            packet.set(task, SectionState::Cached);
            self.section_settled(packet, task, Some(&artifact.body));
            return;
        }

        let prompt_tokens = estimate_tokens_from_bytes(prompt.len());
        if prompt_tokens > spec.role.input_cap() {
            let reason = format!(
                "the {} prompt is {prompt_tokens} tokens, over the {}'s {}-token cap",
                task.as_str(),
                spec.role.as_str(),
                spec.role.input_cap()
            );
            self.log_warn("design-review", reason.clone());
            packet.set(task, SectionState::Failed(reason));
            self.section_settled(packet, task, None);
            return;
        }
        packet.queue.push_back(StaffJob {
            request: packet.request,
            feature_id: packet.feature_id.clone(),
            project_id: packet.project_id.clone(),
            round: packet.round,
            task,
            role: spec.role,
            staff,
            workdir: packet.workdir.clone(),
            prompt,
            input_hash,
            attempt: 1,
            first_response: None,
        });
        packet.set(task, SectionState::Running);
    }

    /// Follow-ups once a section has an outcome: plan drift starts from a
    /// brief, and fails without one.
    fn section_settled(&mut self, packet: &mut ActivePacket, task: Task, body: Option<&str>) {
        if task != Task::Brief || !packet.plan_drift_waits_for_brief {
            return;
        }
        packet.plan_drift_waits_for_brief = false;
        let Some(brief) = body else {
            packet.set(
                Task::PlanDrift,
                SectionState::Failed("the brief was not prepared".to_string()),
            );
            return;
        };
        let Some(gathered) = packet.inputs.as_ref() else {
            return;
        };
        let inputs = PacketInputs {
            decisions: &packet.decisions,
            plan_excerpt: gathered.plan_excerpt.as_deref(),
            brief: Some(brief),
            ..Default::default()
        };
        let (context_text, _) = context::plan_drift_context(&inputs);
        self.schedule_section(packet, Task::PlanDrift, &context_text);
    }

    /// Slots for this packet's runs: [`MAX_PARALLEL_RUNS`], narrowed by the
    /// agent concurrency limit less what else may be running (harness
    /// sessions, other headless runs). At least one, so a review on a busy
    /// machine prepares serially instead of never.
    fn design_review_run_slots(&self, ours: usize) -> usize {
        let Some(limit) = self.config.agent_concurrency_limit() else {
            return MAX_PARALLEL_RUNS;
        };
        let others = crate::resources::limits::max_possible_harness_sessions(&self.store)
            + crate::resources::limits::in_flight_headless_runs()
                .saturating_sub(ours + self.design_review_packet.stale_in_flight);
        limit.saturating_sub(others).clamp(1, MAX_PARALLEL_RUNS)
    }

    fn start_queued_staff_runs(&mut self) -> bool {
        let ours = self
            .design_review_packet
            .active
            .as_ref()
            .map_or(0, |p| p.in_flight);
        let slots = self.design_review_run_slots(ours);
        let runtime = &mut self.design_review_packet;
        let Some(packet) = runtime.active.as_mut() else {
            return false;
        };
        let mut started = false;
        while packet.in_flight < slots {
            let Some(job) = packet.queue.pop_front() else {
                break;
            };
            packet.in_flight += 1;
            started = true;
            spawn_staff_run(&runtime.runner, &runtime.results_tx, job, packet_call);
        }
        started
    }

    /// Record a finished run and, unless it is stale, turn it into an
    /// artifact, a retry or a failure. Returns whether the status changed.
    fn finish_staff_run(&mut self, outcome: StaffOutcome) -> bool {
        let row = staff_usage_row(
            ledger_context(
                outcome.job.task,
                outcome.job.role,
                &outcome.job.staff,
                outcome.job.round,
            ),
            &outcome.job.staff.harness,
            &outcome.job.prompt,
            &outcome,
            &self.config.token_pricing,
        );
        let Finished { job, result, .. } = outcome;
        let usage_id = self.record_design_review_usage(&row, &job.feature_id, &job.project_id);

        let Some(mut packet) = self
            .design_review_packet
            .active
            .take_if(|p| p.request == job.request)
        else {
            let runtime = &mut self.design_review_packet;
            runtime.stale_in_flight = runtime.stale_in_flight.saturating_sub(1);
            self.log_debug(
                "design-review",
                format!(
                    "dropped a stale {} result (request {})",
                    job.task.as_str(),
                    job.request
                ),
            );
            return false;
        };
        packet.in_flight = packet.in_flight.saturating_sub(1);
        self.apply_staff_result(&mut packet, job, result, usage_id);
        self.design_review_packet.active = Some(packet);
        true
    }

    fn apply_staff_result(
        &mut self,
        packet: &mut ActivePacket,
        job: StaffJob,
        result: Result<StaffReply, String>,
        usage_id: Option<i64>,
    ) {
        let spec = section_spec(job.task);
        let body = match (spec.diagram, job.attempt, result) {
            (true, 1, Ok(reply)) => match diagram::check(&reply.text) {
                Ok(src) => Ok(diagram_body(DiagramOutcome::Valid(src))),
                Err(reason) => {
                    self.log_info(
                        "design-review",
                        format!("{} diagram rejected, retrying: {reason}", job.task.as_str()),
                    );
                    let mut retry = job;
                    retry.prompt = diagram::retry_prompt(&retry.prompt, &reason);
                    retry.attempt = 2;
                    retry.first_response = Some(reply.text);
                    packet.queue.push_front(retry);
                    return;
                }
            },
            (true, _, result) => {
                let first = job.first_response.as_deref().unwrap_or("");
                let retry = result.as_ref().ok().map(|r| r.text.as_str());
                Ok(diagram_body(diagram::accept(first, retry)))
            }
            (false, _, Ok(reply)) if reply.text.trim().is_empty() => {
                Err("the staff model returned an empty reply".to_string())
            }
            (false, _, Ok(reply)) => Ok(format!("{}\n", reply.text.trim())),
            (false, _, Err(reason)) => Err(reason),
        };
        let stored = body.and_then(|body| {
            let db = self.db.as_ref().ok_or_else(|| "no database".to_string())?;
            db.insert_design_review_artifact(&NewArtifact {
                feature_id: &job.feature_id,
                project_id: &job.project_id,
                section: job.task.as_str(),
                input_hash: &job.input_hash,
                format: ARTIFACT_FORMAT,
                body: &body,
                usage_id,
            })
            .map_err(|err| format!("cannot store the artifact: {err}"))?;
            Ok(body)
        });
        match stored {
            Ok(body) => {
                packet.set(job.task, SectionState::Done);
                self.section_settled(packet, job.task, Some(&body));
            }
            Err(reason) => {
                self.log_warn(
                    "design-review",
                    format!("{} section failed: {reason}", job.task.as_str()),
                );
                packet.set(job.task, SectionState::Failed(reason));
                self.section_settled(packet, job.task, None);
            }
        }
    }

    pub(super) fn record_design_review_usage(
        &mut self,
        row: &LedgerRow,
        feature_id: &str,
        project_id: &str,
    ) -> Option<i64> {
        let result = self
            .db
            .as_ref()?
            .insert_design_review_usage(row, feature_id, project_id);
        match result {
            Ok(id) => Some(id),
            Err(err) => {
                self.log_warn(
                    "design-review",
                    format!("cannot write a ledger row for {}: {err}", row.task.as_str()),
                );
                None
            }
        }
    }
}

fn ledger_context(task: Task, role: StaffRole, staff: &StaffModel, round: u32) -> LedgerContext {
    LedgerContext {
        phase: Phase::Prepare,
        task,
        agent: agent_for(role),
        harness: staff.harness.slug().to_string(),
        model: staff.model.clone(),
        reasoning: effective_reasoning(staff)
            .map(|level| level.slug().to_string())
            .unwrap_or_default(),
        round,
        agenda_item: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::design_packet::detectors::DetectorHit;
    use crate::diff::{DiffFile, DiffFileStatus};
    use crate::prompts::resolve_template_layered;

    fn big_inputs() -> (Vec<DiffFile>, Vec<String>, Vec<DecisionSummary>, String) {
        let files = (0..400)
            .map(|i| DiffFile {
                old_path: None,
                path: format!("src/m{}/file_{i}.rs", i % 30),
                status: if i % 3 == 0 {
                    DiffFileStatus::Added
                } else {
                    DiffFileStatus::Modified
                },
                additions: 200,
                deletions: 10,
                is_binary: false,
                old_content: None,
                new_content: None,
                patch: format!("@@ -1 +1 @@\n{}", "+line of code here\n".repeat(150)),
                hunks: Vec::new(),
            })
            .collect();
        let commits = (0..300).map(|i| format!("commit subject {i}")).collect();
        let decisions = (0..200)
            .map(|i| DecisionSummary {
                seq: i,
                kind: if i % 2 == 0 { "architecture" } else { "data" }.into(),
                title: "a decision title of some length ".repeat(3),
                paths: vec![format!("src/m{}", i % 30)],
            })
            .collect();
        (files, commits, decisions, "plan line\n".repeat(5_000))
    }

    #[test]
    fn every_built_prompt_stays_under_its_role_cap() {
        let (files, commits, decisions, plan) = big_inputs();
        let modules = packet_inputs::changed_modules(&files);
        let hits: Vec<DetectorHit> = detectors::detect(&files, &[]);
        let brief = "brief bullet\n".repeat(3_000);
        let inputs = PacketInputs {
            files: &files,
            commit_subjects: &commits,
            changed_modules: &modules,
            decisions: &decisions,
            plan_excerpt: Some(&plan),
            brief: Some(&brief),
            detector_hits: &hits,
        };
        let limits = diagram::renderer_limits();
        for task in PACKET_SECTIONS {
            let spec = section_spec(task);
            let context_text = match task {
                Task::Brief => context::brief_context(&inputs).0,
                Task::Architecture => context::architecture_context(&inputs).0,
                Task::KeyFlow => context::key_flow_context(&inputs).0,
                Task::Fit => context::fit_context(&inputs).0,
                Task::Unexplained => context::unexplained_context(&inputs).0,
                _ => context::plan_drift_context(&inputs).0,
            };
            let (template, _) = resolve_template_layered(
                spec.prompt,
                &AgentKind::Claude,
                &crate::prompts::PromptLayers::default(),
            );
            let prompt = render_section_prompt(&spec, &template, &context_text, &limits);
            let tokens = estimate_tokens_from_bytes(prompt.len());
            assert!(
                tokens <= spec.role.input_cap(),
                "{} prompt is {tokens} tokens, cap {}",
                task.as_str(),
                spec.role.input_cap()
            );
            assert!(!prompt.contains("{{context}}"), "{}", task.as_str());
            if spec.diagram {
                assert!(!prompt.contains("{{renderer_limits}}"));
                let retry = diagram::retry_prompt(&prompt, &"x".repeat(300));
                assert!(estimate_tokens_from_bytes(retry.len()) <= spec.role.input_cap());
            }
        }
    }

    #[test]
    fn sections_map_to_roles_and_prompts() {
        assert_eq!(section_spec(Task::Brief).role, StaffRole::Scribe);
        assert_eq!(section_spec(Task::Unexplained).role, StaffRole::Scribe);
        assert_eq!(section_spec(Task::PlanDrift).role, StaffRole::Scribe);
        for task in [Task::Architecture, Task::KeyFlow, Task::Fit] {
            assert_eq!(section_spec(task).role, StaffRole::Cartographer);
        }
        assert!(section_spec(Task::Architecture).diagram);
        assert!(section_spec(Task::KeyFlow).diagram);
        assert!(!section_spec(Task::Fit).diagram);
        let keys: Vec<_> = PACKET_SECTIONS.iter().map(|t| t.as_str()).collect();
        assert_eq!(
            keys,
            [
                "brief",
                "architecture",
                "key-flow",
                "fit",
                "unexplained",
                "plan-drift"
            ]
        );
    }

    #[test]
    fn diagram_body_is_one_mermaid_fence_or_the_list() {
        let body = diagram_body(DiagramOutcome::Valid("flowchart TD\n  A --> B\n".into()));
        assert_eq!(body, "```mermaid\nflowchart TD\n  A --> B\n```\n");
        assert_eq!(
            diagram_body(DiagramOutcome::Fallback("- a\n".into())),
            "- a\n"
        );
    }

    #[test]
    fn unsupported_reasoning_is_not_recorded() {
        let staff = StaffModel {
            harness: AgentKind::Pi,
            model: "m".into(),
            reasoning: Some(ReasoningLevel::High),
        };
        assert_eq!(effective_reasoning(&staff), None);
        let claude = StaffModel {
            harness: AgentKind::Claude,
            ..staff
        };
        assert_eq!(effective_reasoning(&claude), Some(ReasoningLevel::High));
        assert_eq!(
            ledger_context(Task::Fit, StaffRole::Cartographer, &claude, 2).reasoning,
            "high"
        );
    }
}
