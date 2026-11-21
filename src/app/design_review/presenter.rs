//! The question ladder above L0 (plan section 4): the presenter, a staff
//! model, answers the reviewer's latest question on an item at L1 (no tools)
//! or L2 (read-only exploration).
//!
//! `1`/`2` builds the prompt and shows its estimated cost; one confirm key
//! launches it. The run goes through the packet's runner and channel
//! machinery ([`super::packet::spawn_staff_run`]), so it takes a headless
//! lease and is timed like every staff run. A pending turn is written at
//! launch and filled in when the answer arrives. The answer is kept even if
//! the room was closed meanwhile (it was paid for), but dropped when the
//! item no longer exists. Every run writes one ledger row: phase `discuss`,
//! task `question-L1`/`question-L2`, agent `presenter`, the agenda item.

use std::path::PathBuf;

use crate::db::design_review::DesignReviewTurn;
use crate::design_packet::context::{PresenterInputs, presenter_context};
use crate::design_packet::staff::{self, StaffModel, StaffRole};
use crate::design_review::agenda::{self, ItemKind};
use crate::prompts::{PromptContext, PromptId};
use crate::token_cost::cost::{TokenCounts, price};
use crate::token_cost::ledger::{LedgerContext, Phase, Task, estimate_tokens_from_bytes};
use crate::token_cost::pricing::lookup_with;
use crate::token_tracking::{TokenPricingConfig, format_dollar_cost, format_token_count};

use super::actions::ROLE_REVIEWER;
use super::packet::{
    Finished, StaffCall, agent_for, effective_reasoning, spawn_staff_run, staff_usage_row,
};
use super::room::DesignReviewRoom;
use crate::app::{App, AppMode};

/// The role of a presenter turn.
pub const ROLE_PRESENTER: &str = "presenter";
/// A turn whose answer is still on its way.
pub const STATUS_PENDING: &str = "pending";
pub const STATUS_DONE: &str = "done";
pub const STATUS_FAILED: &str = "failed";

/// Longest failure reason kept on a failed turn.
const MAX_REASON_CHARS: usize = 160;

/// A presenter level of the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderLevel {
    /// No tools, an 8K-token prompt.
    L1,
    /// Read-only exploration from a 24K-token seed.
    L2,
}

impl LadderLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            LadderLevel::L1 => "L1",
            LadderLevel::L2 => "L2",
        }
    }

    pub fn task(self) -> Task {
        match self {
            LadderLevel::L1 => Task::QuestionL1,
            LadderLevel::L2 => Task::QuestionL2,
        }
    }

    pub fn prompt_id(self) -> PromptId {
        match self {
            LadderLevel::L1 => PromptId::DesignPresenter,
            LadderLevel::L2 => PromptId::DesignPresenterDeep,
        }
    }

    pub fn read_only(self) -> bool {
        self == LadderLevel::L2
    }

    /// The prompt cap: the presenter's input cap, or its deep-dive seed cap.
    pub fn cap(self) -> u64 {
        match self {
            LadderLevel::L1 => StaffRole::Presenter.input_cap(),
            LadderLevel::L2 => StaffRole::Presenter.total_cap(),
        }
    }

    /// Reply tokens assumed by the cost estimate.
    pub fn reply_estimate(self) -> u64 {
        match self {
            LadderLevel::L1 => 600,
            LadderLevel::L2 => 1_000,
        }
    }

    fn tools(self) -> &'static str {
        match self {
            LadderLevel::L1 => "no tools",
            LadderLevel::L2 => "read-only",
        }
    }
}

/// An escalation built and priced, waiting for the reviewer to confirm it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Escalation {
    pub level: LadderLevel,
    pub item_id: String,
    pub staff: StaffModel,
    pub prompt: String,
    /// The confirm line: level, model, prompt size and estimated cost.
    pub summary: String,
}

/// One presenter run, as sent to the worker thread and back.
#[derive(Debug, Clone)]
pub(crate) struct PresenterJob {
    pub request: u64,
    pub feature_id: String,
    pub project_id: String,
    pub item_id: String,
    pub turn_id: String,
    pub round: u32,
    pub level: LadderLevel,
    pub staff: StaffModel,
    pub workdir: PathBuf,
    pub prompt: String,
}

fn presenter_call(job: &PresenterJob) -> StaffCall<'_> {
    StaffCall {
        task: job.level.task(),
        harness: &job.staff.harness,
        model: &job.staff.model,
        reasoning: effective_reasoning(&job.staff),
        read_only: job.level.read_only(),
        workdir: &job.workdir,
        prompt: &job.prompt,
    }
}

/// The reviewer's latest question in a thread: the last reviewer turn that a
/// ladder answer (a turn with a level) follows. Notes and change requests
/// have no answer after them, so they are never picked.
pub fn latest_question(turns: &[DesignReviewTurn]) -> Option<&str> {
    turns.windows(2).rev().find_map(|pair| {
        (pair[0].role == ROLE_REVIEWER && pair[1].level.is_some()).then_some(pair[0].body.as_str())
    })
}

/// The confirm line: what will run, how big the prompt is and what it is
/// estimated to cost at the presenter model's own rates.
pub fn estimate_line(
    level: LadderLevel,
    staff: &StaffModel,
    prompt_tokens: u64,
    fallback: &TokenPricingConfig,
) -> String {
    let model = lookup_with(&staff.model, fallback);
    let counts = TokenCounts {
        input: Some(prompt_tokens),
        output: Some(level.reply_estimate()),
        ..Default::default()
    };
    let cost = price(&counts, &model.rates).cost_usd;
    let assumed = if model.price_assumed {
        " (price assumed)"
    } else {
        ""
    };
    let reads = if level.read_only() {
        " plus what it reads"
    } else {
        ""
    };
    format!(
        "Ask the presenter ({}): {} on {}, {} · ~{} in, ~{} out · est. {}{}{}",
        level.as_str(),
        staff.model,
        staff.harness.display_name(),
        level.tools(),
        format_token_count(prompt_tokens),
        format_token_count(level.reply_estimate()),
        format_dollar_cost(cost),
        assumed,
        reads,
    )
}

/// Who answered, as the first line of a presenter turn.
fn attribution(job: &PresenterJob) -> String {
    format!(
        "Answered by the presenter ({} on {}, {}), not the developer.",
        job.staff.model,
        job.staff.harness.display_name(),
        job.level.tools()
    )
}

fn short_reason(reason: &str) -> String {
    let reason = reason.lines().next().unwrap_or("").trim();
    if reason.chars().count() <= MAX_REASON_CHARS {
        return reason.to_string();
    }
    let kept: String = reason.chars().take(MAX_REASON_CHARS - 1).collect();
    format!("{kept}…")
}

impl App {
    /// `1`/`2`: build the presenter prompt for the selected item's latest
    /// question and show its estimated cost; nothing runs until confirmed.
    pub fn design_review_begin_escalation(&mut self, level: LadderLevel) {
        match self.build_escalation(level) {
            Ok(escalation) => {
                if let Some(room) = self.design_review_room_mut() {
                    room.confirm = Some(escalation);
                }
            }
            Err(message) => self.push_toast_warning(message),
        }
    }

    fn build_escalation(&self, level: LadderLevel) -> Result<Escalation, String> {
        let AppMode::DesignReview(room) = &self.mode else {
            return Err("The review room is not open".to_string());
        };
        let item = room
            .selected_item()
            .ok_or_else(|| "Nothing selected".to_string())?;
        if room
            .turns
            .iter()
            .any(|t| t.role == ROLE_PRESENTER && t.status == STATUS_PENDING)
        {
            return Err("The presenter is still answering this item".to_string());
        }
        let question = latest_question(&room.turns)
            .ok_or_else(|| "Ask a question first (s), then escalate it".to_string())?;
        let (repo, agent) = self
            .store
            .projects
            .iter()
            .find_map(|p| {
                p.features
                    .iter()
                    .find(|f| f.id == room.feature_id)
                    .map(|f| (p.repo.clone(), f.agent.clone()))
            })
            .ok_or_else(|| "This feature no longer exists".to_string())?;
        let staff = staff::resolve(StaffRole::Presenter, &self.config.design_review, &agent)
            .ok_or_else(|| {
                format!(
                    "No presenter model is configured for {}",
                    agent.display_name()
                )
            })?;
        let (template, _) =
            self.resolve_headless_template(level.prompt_id(), &staff.harness, &repo, &room.workdir);
        let prompt = render_presenter_prompt(
            &template,
            &self.presenter_context_text(room, item, question, level),
        );
        let tokens = estimate_tokens_from_bytes(prompt.len());
        if tokens > level.cap() {
            return Err(format!(
                "The {} prompt is {tokens} tokens, over the presenter's {}-token cap",
                level.as_str(),
                level.cap()
            ));
        }
        Ok(Escalation {
            level,
            item_id: item.id.clone(),
            summary: estimate_line(level, &staff, tokens, &self.config.token_pricing),
            staff,
            prompt,
        })
    }

    /// The budgeted presenter context for one question: the record, the
    /// other decisions' titles, the item's hunks from the room's snapshot
    /// and the relevant packet section (the item's own, else the brief).
    fn presenter_context_text(
        &self,
        room: &DesignReviewRoom,
        item: &crate::db::design_review::DesignReviewItem,
        question: &str,
        level: LadderLevel,
    ) -> String {
        let section = ItemKind::parse(&item.kind)
            .and_then(ItemKind::section_key)
            .unwrap_or("brief");
        let excerpt = self.db.as_ref().and_then(|db| {
            db.latest_design_review_artifact(&room.feature_id, section)
                .ok()
                .flatten()
                .map(|a| a.body)
        });
        let decision = room.decision_for(item);
        let record = decision.map(agenda::decision_markdown);
        let neighbours: Vec<String> = room
            .decisions
            .iter()
            .filter(|d| d.status != "superseded" && Some(d.uid.as_str()) != item.ref_key.as_deref())
            .map(|d| format!("d{} [{}] {}", d.seq, d.kind, d.title))
            .collect();
        let files = room.snapshot.files.as_deref().unwrap_or(&[]);
        presenter_context(
            &PresenterInputs {
                question,
                item_title: &item.title,
                record: record.as_deref(),
                neighbours: &neighbours,
                item_paths: &item.paths,
                files,
                packet_excerpt: excerpt.as_deref(),
            },
            level == LadderLevel::L2,
        )
        .0
    }

    pub fn design_review_cancel_escalation(&mut self) {
        if let Some(room) = self.design_review_room_mut() {
            room.confirm = None;
        }
    }

    /// The confirm key: write the pending turn and launch the run.
    pub fn design_review_confirm_escalation(&mut self) {
        let Some((escalation, room_round, feature_id, workdir)) =
            self.design_review_room_mut().and_then(|room| {
                let escalation = room.confirm.take()?;
                Some((
                    escalation,
                    room.round,
                    room.feature_id.clone(),
                    room.workdir.clone(),
                ))
            })
        else {
            return;
        };
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let exists = db
            .design_review_item_exists(&escalation.item_id)
            .unwrap_or(false);
        let item = match &self.mode {
            AppMode::DesignReview(room) if exists => room
                .items
                .iter()
                .find(|i| i.id == escalation.item_id)
                .cloned(),
            _ => None,
        };
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let Some(item) = item else {
            self.push_toast_warning("That item is no longer on the agenda");
            return;
        };
        let turn = DesignReviewTurn {
            id: uuid::Uuid::new_v4().to_string(),
            item_id: item.id.clone(),
            round_id: Some(item.round_id.clone()),
            feature_id: item.feature_id.clone(),
            project_id: item.project_id.clone(),
            seq: 0,
            role: ROLE_PRESENTER.to_string(),
            level: Some(escalation.level.as_str().to_string()),
            body: format!(
                "Waiting for the presenter ({}, {})…",
                escalation.staff.model,
                escalation.level.tools()
            ),
            status: STATUS_PENDING.to_string(),
            usage_id: None,
            created_at: crate::db::learning::now_timestamp(),
        };
        if let Err(err) = db.append_design_review_turn(&turn) {
            self.log_error(
                "design-review",
                format!("cannot write the pending presenter turn: {err:#}"),
            );
            self.push_toast_error("Could not save that to the database; nothing was asked");
            return;
        }
        let runtime = &mut self.design_review_packet;
        let request = runtime.next_request_id();
        let job = PresenterJob {
            request,
            feature_id,
            project_id: item.project_id.clone(),
            item_id: item.id.clone(),
            turn_id: turn.id.clone(),
            round: u32::try_from(room_round).unwrap_or(0),
            level: escalation.level,
            staff: escalation.staff,
            workdir,
            prompt: escalation.prompt,
        };
        runtime.presenter_in_flight += 1;
        spawn_staff_run(&runtime.runner, &runtime.presenter_tx, job, presenter_call);
        self.log_info(
            "design-review",
            format!(
                "asked the presenter at {} for item {} (request {request})",
                escalation.level.as_str(),
                item.id
            ),
        );
        self.design_review_reload_thread(&item.id);
    }

    /// Record a finished presenter run and fill in its pending turn. The
    /// ledger row is written whatever happens (the spend happened); the
    /// answer is dropped only when its item no longer exists.
    pub(super) fn finish_presenter_run(&mut self, outcome: Finished<PresenterJob>) -> bool {
        let runtime = &mut self.design_review_packet;
        runtime.presenter_in_flight = runtime.presenter_in_flight.saturating_sub(1);
        let job = &outcome.job;
        let ctx = LedgerContext {
            phase: Phase::Discuss,
            task: job.level.task(),
            agent: agent_for(StaffRole::Presenter),
            harness: job.staff.harness.slug().to_string(),
            model: job.staff.model.clone(),
            reasoning: effective_reasoning(&job.staff)
                .map(|level| level.slug().to_string())
                .unwrap_or_default(),
            round: job.round,
            agenda_item: Some(job.item_id.clone()),
        };
        let row = staff_usage_row(
            ctx,
            &job.staff.harness,
            &job.prompt,
            &outcome,
            &self.config.token_pricing,
        );
        let usage_id = self.record_design_review_usage(&row, &job.feature_id, &job.project_id);
        let job = &outcome.job;

        let Some(db) = self.db.as_ref() else {
            return false;
        };
        if !db.design_review_item_exists(&job.item_id).unwrap_or(false) {
            self.log_debug(
                "design-review",
                format!(
                    "dropped a presenter answer for a deleted item (request {})",
                    job.request
                ),
            );
            return false;
        }
        let (body, status) = match &outcome.result {
            Ok(reply) if !reply.text.trim().is_empty() => (
                format!("{}\n\n{}", attribution(job), reply.text.trim()),
                STATUS_DONE,
            ),
            Ok(_) => (
                "The presenter could not answer: it returned an empty reply.".to_string(),
                STATUS_FAILED,
            ),
            Err(reason) => (
                format!("The presenter could not answer: {}", short_reason(reason)),
                STATUS_FAILED,
            ),
        };
        let stored = db.complete_design_review_turn(&job.turn_id, &body, status, usage_id);
        match stored {
            Ok(true) => {}
            Ok(false) => {
                self.log_debug(
                    "design-review",
                    format!(
                        "dropped a presenter answer whose turn is gone (request {})",
                        job.request
                    ),
                );
                return false;
            }
            Err(err) => {
                self.log_error(
                    "design-review",
                    format!("cannot store the presenter answer: {err:#}"),
                );
                return false;
            }
        }
        if status == STATUS_FAILED {
            self.log_warn("design-review", format!("presenter run failed: {body}"));
        }
        let item_id = job.item_id.clone();
        let open_here =
            matches!(&self.mode, AppMode::DesignReview(room) if room.feature_id == job.feature_id);
        if open_here {
            self.design_review_reload_thread(&item_id);
            self.design_review_reload_usage();
        }
        true
    }

    /// Re-read the thread when `item_id` is the selected item.
    fn design_review_reload_thread(&mut self, item_id: &str) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let AppMode::DesignReview(room) = &mut self.mode else {
            return;
        };
        if room.selected_item().map(|i| i.id.as_str()) != Some(item_id) {
            return;
        }
        match db.design_review_turns(item_id) {
            Ok(turns) => {
                room.turns = turns;
                room.thread_scroll = 0;
            }
            Err(err) => {
                let message = format!("cannot read thread: {err}");
                self.log_warn("design-review", message);
            }
        }
    }
}

/// The presenter prompt: the template with `{{context}}` filled in.
pub(crate) fn render_presenter_prompt(template: &str, context_text: &str) -> String {
    crate::prompts::render_template(
        template,
        &PromptContext::new().with("context", context_text),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::{DiffFile, DiffFileStatus};
    use crate::project::AgentKind;
    use crate::prompts::resolve_template_layered;

    fn turn(role: &str, level: Option<&str>, body: &str) -> DesignReviewTurn {
        DesignReviewTurn {
            id: body.into(),
            item_id: "i".into(),
            round_id: None,
            feature_id: "f".into(),
            project_id: "p".into(),
            seq: 0,
            role: role.into(),
            level: level.map(str::to_string),
            body: body.into(),
            status: "done".into(),
            usage_id: None,
            created_at: "t".into(),
        }
    }

    #[test]
    fn the_latest_question_is_the_last_answered_reviewer_turn() {
        let turns = vec![
            turn("reviewer", None, "first?"),
            turn("record", Some("L0"), "record"),
            turn("reviewer", None, "second?"),
            turn("record", Some("L0"), "record"),
            turn("presenter", Some("L1"), "answer"),
            turn("reviewer", None, "a note, unanswered"),
        ];
        assert_eq!(latest_question(&turns), Some("second?"));
        assert_eq!(latest_question(&turns[..1]), None);
        assert_eq!(latest_question(&[]), None);
    }

    #[test]
    fn the_estimate_names_level_model_size_and_cost() {
        let sonnet = StaffModel {
            harness: AgentKind::Claude,
            model: "sonnet".into(),
            reasoning: None,
        };
        let pricing = TokenPricingConfig::default();
        let line = estimate_line(LadderLevel::L1, &sonnet, 4_000, &pricing);
        // 4K in at $2/M plus 600 out at $10/M: $0.014.
        assert_eq!(
            line,
            "Ask the presenter (L1): sonnet on Claude, no tools · ~4.0k in, ~600 out · est. $0.01"
        );
        let deep = estimate_line(LadderLevel::L2, &sonnet, 20_000, &pricing);
        assert!(
            deep.contains("(L2)") && deep.contains("read-only"),
            "{deep}"
        );
        assert!(deep.ends_with("plus what it reads"), "{deep}");
        let unknown = StaffModel {
            model: "mystery-model".into(),
            ..sonnet
        };
        assert!(estimate_line(LadderLevel::L1, &unknown, 10, &pricing).contains("price assumed"));
    }

    #[test]
    fn presenter_prompts_stay_under_their_caps_and_refuse_to_invent() {
        let files: Vec<DiffFile> = (0..200)
            .map(|i| DiffFile {
                old_path: None,
                path: format!("src/m/f{i}.rs"),
                status: DiffFileStatus::Modified,
                additions: 300,
                deletions: 0,
                is_binary: false,
                old_content: None,
                new_content: None,
                patch: "+code line\n".repeat(1_000),
                hunks: vec![],
            })
            .collect();
        let neighbours: Vec<String> = (0..500).map(|i| format!("d{i} [data] t")).collect();
        let paths = vec!["src/m".to_string()];
        let record = "Why: x\n".repeat(3_000);
        let excerpt = "- brief\n".repeat(5_000);
        for level in [LadderLevel::L1, LadderLevel::L2] {
            let (context_text, _) = presenter_context(
                &PresenterInputs {
                    question: &"why? ".repeat(2_000),
                    item_title: "d1 t",
                    record: Some(&record),
                    neighbours: &neighbours,
                    item_paths: &paths,
                    files: &files,
                    packet_excerpt: Some(&excerpt),
                },
                level == LadderLevel::L2,
            );
            for harness in [
                AgentKind::Claude,
                AgentKind::Codex,
                AgentKind::Opencode,
                AgentKind::Pi,
            ] {
                let (template, _) = resolve_template_layered(
                    level.prompt_id(),
                    &harness,
                    &crate::prompts::PromptLayers::default(),
                );
                assert!(template.contains("not recorded"), "{}", level.as_str());
                assert!(template.contains("presenter"), "{}", level.as_str());
                let prompt = render_presenter_prompt(&template, &context_text);
                assert!(!prompt.contains("{{context}}"));
                let tokens = estimate_tokens_from_bytes(prompt.len());
                assert!(tokens <= level.cap(), "{}: {tokens}", level.as_str());
            }
        }
    }

    #[test]
    fn failure_reasons_are_one_short_line() {
        assert_eq!(short_reason("boom\nstack"), "boom");
        let long = short_reason(&"x".repeat(500));
        assert_eq!(long.chars().count(), MAX_REASON_CHARS);
        assert!(long.ends_with('…'));
    }
}
