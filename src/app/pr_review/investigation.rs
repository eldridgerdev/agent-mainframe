use super::{PrComment, PrInvestigationStatus, PrInvestigationTurn, ReplyKind};
use crate::app::{
    AgentKind, App, AppMode, InvestigationAction, InvestigationActionPick,
    InvestigationFollowUpDraft, InvestigationHarnessPick, PendingFollowUp,
    PrInvestigationLoadState,
};
use crate::github::{GhCli, PrChangedFile};
use crate::headless::HeadlessRunner;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// The answer (plus any follow-up turns) of a **completed** read-only
/// investigation, formatted for embedding in a fix prompt so the fixing agent
/// starts from that analysis. `None` when there is no completed run or the
/// answer is empty; callers add the surrounding header.
pub(crate) fn investigation_findings_for_prompt(
    inv: &crate::db::pr_investigations::PrInvestigation,
) -> Option<String> {
    if inv.status != PrInvestigationStatus::Complete {
        return None;
    }
    let answer = inv
        .answer
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())?;
    let mut out = answer.to_string();
    for turn in &inv.follow_ups {
        out.push_str(&format!(
            "\n\nFollow-up — Q: {}\nA: {}",
            turn.question.trim(),
            turn.answer.trim()
        ));
    }
    Some(out)
}

/// Most changed-file rows to list in an investigation prompt before collapsing
/// the rest to "…and N more" — a large PR must not bury the comment under its
/// own file list.
pub(super) const INVESTIGATION_CHANGED_FILES_LIMIT: usize = 40;

/// Prior follow-up turns fed back into a follow-up investigation, oldest first
/// (the initial finding is always included on top of this).
pub(super) const INVESTIGATION_FOLLOW_UP_DEPTH: usize = 3;

/// Character cap on the optional operator-supplied investigation note (`e`).
/// A hypothesis, not a spec — roomier than the plan-interview custom answer
/// (500) but still bounded so it can't crowd out the prompt.
pub(super) const INVESTIGATION_CONTEXT_MAX_LEN: usize = 1000;

/// One file a PR touches — path plus GitHub's per-file line deltas
/// (`gh pr view --json files`). Handed to the investigation prompt so the
/// read-only agent knows the PR's blast radius and can open any of them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvestigationChangedFile {
    pub path: String,
    pub additions: u32,
    pub deletions: u32,
}

impl InvestigationChangedFile {
    fn from_gh(f: &PrChangedFile) -> Self {
        Self {
            path: f.path.clone(),
            additions: f.additions,
            deletions: f.deletions,
        }
    }
}

/// The initial finding plus any earlier follow-up turns and the operator's new
/// question — the extra context a follow-up investigation carries (Learning
/// Mode `F` behaviour).
#[derive(Debug, Clone)]
pub struct InvestigationFollowUp<'a> {
    /// The initial investigation's answer.
    pub initial_answer: &'a str,
    /// Earlier follow-up turns, oldest first. Trimmed by the builder.
    pub prior_turns: &'a [PrInvestigationTurn],
    /// The operator's new question.
    pub question: &'a str,
}

/// Everything [`build_investigation_prompt`] needs, as a plain value so the
/// builder stays pure and unit-testable (mirrors
/// [`crate::app::learning::LearningPromptContext`]).
#[derive(Debug, Clone)]
pub struct InvestigationPromptContext<'a> {
    /// The review comment being investigated.
    pub comment: &'a PrComment,
    pub pr_number: u32,
    pub pr_title: &'a str,
    /// The PR description (GitHub `body`); may be empty.
    pub pr_description: &'a str,
    /// Files the PR touches. Empty is acceptable (a fetch failure or an older
    /// cache) — the builder just omits the section.
    pub changed_files: &'a [InvestigationChangedFile],
    /// `None` for the initial investigation; `Some` for a follow-up.
    pub follow_up: Option<InvestigationFollowUp<'a>>,
    /// Optional free-form note the operator attached at triage time (`e`): a
    /// hypothesis to check, not a fact to assume. `None` or all-whitespace
    /// renders nothing at all — the prompt is byte-identical to today.
    pub user_context: Option<&'a str>,
}

/// Assemble the minimal read-only investigation prompt for one review comment.
///
/// Like [`PrComment::fix_prompt`] it carries **no file contents**: the agent
/// has the repo checked out (read-only) and opens what it needs. The context is
/// only the review comment, the PR's title + description, and the list of files
/// the PR touches — the "minimal context" the plan calls for. The read-only
/// instruction block is last so it's the freshest thing the model reads (the
/// same ordering rationale as [`crate::app::learning::build_prompt`]).
pub fn build_investigation_prompt(ctx: &InvestigationPromptContext<'_>) -> String {
    let mut out = String::from(
        "Investigate this PR review comment. Someone triaging the pull request \
         flagged it as a question to answer, not a change to make — do not \
         modify anything.\n\n",
    );

    let title = ctx.pr_title.trim();
    if title.is_empty() {
        out.push_str(&format!("PR #{}\n\n", ctx.pr_number));
    } else {
        out.push_str(&format!("PR #{}: {title}\n\n", ctx.pr_number));
    }

    out.push_str("PR description:\n");
    let description = ctx.pr_description.trim();
    if description.is_empty() {
        out.push_str("(none)\n\n");
    } else {
        out.push_str(description);
        out.push_str("\n\n");
    }

    if !ctx.changed_files.is_empty() {
        out.push_str("Files changed by this PR (open any of these as needed):\n");
        for file in ctx
            .changed_files
            .iter()
            .take(INVESTIGATION_CHANGED_FILES_LIMIT)
        {
            out.push_str(&format!(
                "  {}  (+{} -{})\n",
                file.path, file.additions, file.deletions
            ));
        }
        let extra = ctx
            .changed_files
            .len()
            .saturating_sub(INVESTIGATION_CHANGED_FILES_LIMIT);
        if extra > 0 {
            out.push_str(&format!("  …and {extra} more\n"));
        }
        out.push('\n');
    }

    out.push_str("--- The review comment ---\n");
    out.push_str(&ctx.comment.fix_prompt_body());
    out.push_str("\n\n");

    if let Some(hypothesis) = ctx
        .user_context
        .map(str::trim)
        .filter(|note| !note.is_empty())
    {
        out.push_str("--- What the person triaging suspects ---\n");
        out.push_str(
            "Treat the following as a hypothesis to verify, not an established \
             fact. Check it against the PR and the repository and say plainly \
             whether the code confirms or contradicts it:\n",
        );
        out.push_str(hypothesis);
        out.push_str("\n\n");
    }

    if let Some(follow_up) = &ctx.follow_up {
        out.push_str("--- The investigation so far ---\n");
        out.push_str(&format!(
            "Initial finding: {}\n\n",
            follow_up.initial_answer.trim()
        ));
        let start = follow_up
            .prior_turns
            .len()
            .saturating_sub(INVESTIGATION_FOLLOW_UP_DEPTH);
        for turn in &follow_up.prior_turns[start..] {
            out.push_str(&format!("They then asked: {}\n", turn.question.trim()));
            out.push_str(&format!("You answered: {}\n\n", turn.answer.trim()));
        }
        out.push_str("--- Their follow-up question ---\n");
        out.push_str(follow_up.question.trim());
        out.push_str("\n\n");
    }

    out.push_str(
        "You have read-only access to this repository. Do not edit files, run \
         commands that change state, or stage, commit, or push anything — this \
         is an investigation only. Open the referenced file and whatever it \
         depends on, ground every claim in what you actually read, and name the \
         files and symbols you checked. If you looked for something and could \
         not find it, say so rather than guessing.\n",
    );
    out.push_str(match ctx.follow_up {
        None => {
            "Answer in Markdown: start with a one-line verdict on whether the \
             comment's concern is valid, then the evidence, then — without \
             making the change — what a fix would involve.\n"
        }
        Some(_) => {
            "Answer their follow-up question in Markdown, grounded the same way. \
             Do not make any change; describe it if one is warranted.\n"
        }
    });

    out.trim_end().to_string()
}

/// What a background investigation run hands back to the poll loop. Carries
/// everything needed to persist the row without touching `self.mode` (which may
/// have moved on) — the built prompt is included so the finished row records
/// exactly what the agent was given, and `created_at` is carried so the
/// completing `upsert` keeps the marker row's timestamp.
pub struct InvestigationOutcome {
    pub project_id: String,
    pub pr_number: u32,
    pub comment_id: u64,
    pub harness: AgentKind,
    pub head_sha: String,
    pub context_snapshot: String,
    pub created_at: String,
    /// `Some(question)` when this run answered a follow-up: the poll loop then
    /// appends a [`crate::app::pr_review::PrInvestigationTurn`] to the row's
    /// thread rather than replacing the initial answer.
    pub follow_up_question: Option<String>,
    /// `Ok(answer_markdown)` or `Err(user-facing message)`.
    pub result: std::result::Result<String, String>,
}

/// Insert `row` into the in-memory investigation list, replacing any existing
/// entry for the same comment (one investigation per comment).
pub(crate) fn upsert_investigation_in_memory(
    list: &mut Vec<crate::db::pr_investigations::PrInvestigation>,
    row: crate::db::pr_investigations::PrInvestigation,
) {
    match list.iter_mut().find(|r| r.comment_id == row.comment_id) {
        Some(existing) => *existing = row,
        None => list.push(row),
    }
}

/// A one-line, user-facing failure message for a headless investigation run.
pub(super) fn investigation_failure_message(harness: &AgentKind, err: &anyhow::Error) -> String {
    let raw = err.to_string();
    let raw = raw.split('\n').next().unwrap_or(&raw).trim();
    format!(
        "{} couldn't finish the investigation: {raw}. Try again, or pick another harness.",
        harness.display_name()
    )
}

impl App {
    /// Load this PR's persisted read-only investigations for the triage pane's
    /// in-memory list. Any row still marked `running` is stale — the only
    /// process that could be filling it belongs to a `PrInvestigationLoading`
    /// that is not open right now — so it is reconciled to `failed` (in memory
    /// and, best-effort, on disk) with a message, rather than reopening as a
    /// throbber that never resolves. No DB, or a workdir that isn't a tracked
    /// feature, yields an empty list.
    pub(crate) fn pr_review_load_investigations(
        &mut self,
        workdir: &Path,
        pr_number: u32,
    ) -> Vec<crate::db::pr_investigations::PrInvestigation> {
        use crate::app::pr_review::PrInvestigationStatus;

        let Some((pi, _)) = self.feature_indices_for_workdir(workdir) else {
            return Vec::new();
        };
        let project_id = self.store.projects[pi].id.clone();
        let Some(db) = self.db.as_ref() else {
            return Vec::new();
        };
        let mut rows = match db.load_pr_investigations(&project_id, pr_number) {
            Ok(rows) => rows,
            Err(e) => {
                self.log_warn("pr_triage", format!("investigation load failed: {e}"));
                return Vec::new();
            }
        };

        const INTERRUPTED: &str =
            "investigation interrupted (AMF restarted or the pane closed before it finished)";
        let stale: Vec<u64> = rows
            .iter()
            .filter(|r| r.status == PrInvestigationStatus::Running)
            .map(|r| r.comment_id)
            .collect();
        for row in &mut rows {
            if row.status == PrInvestigationStatus::Running {
                row.status = PrInvestigationStatus::Failed;
                row.error = Some(INTERRUPTED.to_string());
            }
        }
        if !stale.is_empty()
            && let Some(db) = self.db.as_ref()
        {
            for comment_id in stale {
                let _ = db.finish_pr_investigation(
                    &project_id,
                    pr_number,
                    comment_id,
                    None,
                    PrInvestigationStatus::Failed,
                    Some(INTERRUPTED),
                );
            }
        }
        rows
    }

    /// Begin the read-only investigation flow for the selected comment: validate
    /// it, then open the per-run harness picker (or skip straight to the run
    /// when only one harness is available).
    pub fn pr_review_start_investigation(&mut self) {
        if self.pr_review_work.investigation_pending() {
            self.message = Some("An investigation is already running".into());
            return;
        }
        let selected = match &self.mode {
            AppMode::PrReview(state) => state.selected_comment().map(|c| (c.id, c.is_actionable())),
            _ => return,
        };
        let Some((_id, actionable)) = selected else {
            self.message = Some("No comment selected".into());
            return;
        };
        if !actionable {
            self.message =
                Some("AMF follow-up replies cannot be fixed or investigated".to_string());
            return;
        }

        let workdir = match &self.mode {
            AppMode::PrReview(state) => state.workdir.clone(),
            _ => return,
        };
        let mut harnesses = self.allowed_agents_for_project_path(&workdir);
        if harnesses.is_empty() {
            harnesses = self.store.available_harnesses.clone();
        }
        if harnesses.is_empty() {
            self.push_toast_error(
                "No agent harness available to run an investigation — check `amf doctor`",
            );
            return;
        }
        if harnesses.len() == 1 {
            let only = harnesses[0].clone();
            let follow_up = match &mut self.mode {
                AppMode::PrReview(state) => state.pending_follow_up.take(),
                _ => None,
            };
            self.pr_review_launch_investigation(only, follow_up);
            return;
        }
        let preferred = self
            .feature_indices_for_workdir(&workdir)
            .map(|(pi, _)| self.store.projects[pi].preferred_agent.clone());
        let selected = preferred
            .and_then(|p| harnesses.iter().position(|h| *h == p))
            .unwrap_or(0);
        if let AppMode::PrReview(state) = &mut self.mode {
            state.investigation_harness_pick = Some(InvestigationHarnessPick {
                harnesses,
                selected,
            });
        }
    }

    pub fn pr_review_investigation_harness_picking(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state) if state.investigation_harness_pick.is_some()
        )
    }

    pub fn pr_review_investigation_harness_move(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(pick) = &mut state.investigation_harness_pick
        {
            let len = pick.harnesses.len();
            if len == 0 {
                return;
            }
            pick.selected = (pick.selected as isize + delta).rem_euclid(len as isize) as usize;
        }
    }

    pub fn pr_review_investigation_harness_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.investigation_harness_pick = None;
        }
    }

    pub fn pr_review_investigation_harness_confirm(&mut self) {
        let harness = match &mut self.mode {
            AppMode::PrReview(state) => {
                let picked = state
                    .investigation_harness_pick
                    .as_ref()
                    .and_then(|p| p.harnesses.get(p.selected).cloned());
                state.investigation_harness_pick = None;
                picked
            }
            _ => None,
        };
        if let Some(harness) = harness {
            // A parked follow-up question routes this run through the follow-up
            // path; anything else is a fresh investigation of the selection.
            let follow_up = match &mut self.mode {
                AppMode::PrReview(state) => state.pending_follow_up.take(),
                _ => None,
            };
            self.pr_review_launch_investigation(harness, follow_up);
        }
    }

    /// Stash the pane, write a `Running` marker row (fresh runs only), and spawn
    /// the blocking read-only run. The overlay stays modal
    /// (`AppMode::PrInvestigationLoading`) until [`Self::poll_pr_investigation_bg`]
    /// restores it with the result.
    ///
    /// `follow_up` is `Some` for a Learning-Mode-`F`-style re-run: the prompt
    /// carries the prior turn(s) and the poll loop appends a thread turn instead
    /// of replacing the initial answer.
    pub(super) fn pr_review_launch_investigation(
        &mut self,
        harness: AgentKind,
        follow_up: Option<PendingFollowUp>,
    ) {
        let (workdir, pr) = match &self.mode {
            AppMode::PrReview(state) => (state.workdir.clone(), state.review.pr.clone()),
            _ => return,
        };
        // The target comment is the follow-up's (fixed when the question was
        // asked) or the current selection for a fresh run.
        let target_comment_id = follow_up.as_ref().map(|f| f.comment_id);
        let comment = match &self.mode {
            AppMode::PrReview(state) => {
                let found = match target_comment_id {
                    Some(id) => state.review.comments.iter().find(|c| c.id == id),
                    None => state.selected_comment(),
                };
                match found {
                    Some(c) => c.clone(),
                    None => {
                        self.message = Some("No comment selected".into());
                        return;
                    }
                }
            }
            _ => return,
        };
        let Some((pi, _)) = self.feature_indices_for_workdir(&workdir) else {
            self.push_toast_error("Can't investigate: this worktree isn't a tracked AMF feature");
            return;
        };
        let project_id = self.store.projects[pi].id.clone();
        let pr_number = pr.number;
        let head_sha = pr.head_sha.clone();
        let comment_id = comment.id;
        let follow_up_question = follow_up.as_ref().map(|f| f.question.clone());

        // The optional operator-supplied hypothesis, for a fresh investigation
        // only. Taken (and cleared) here whether or not the run ultimately
        // succeeds — the note described *this* attempt; a retry re-attaches it.
        let user_context: Option<String> = if follow_up.is_none() {
            match &mut self.mode {
                AppMode::PrReview(state) => {
                    let note = std::mem::take(&mut state.investigation_context.note);
                    (!note.trim().is_empty()).then_some(note)
                }
                _ => None,
            }
        } else {
            None
        };

        // Prior turns for a follow-up prompt: the initial answer plus any
        // earlier follow-up turns already on the row.
        let prior_context: Option<(String, Vec<PrInvestigationTurn>)> =
            follow_up.as_ref().and_then(|_| {
                let AppMode::PrReview(state) = &self.mode else {
                    return None;
                };
                state
                    .investigations
                    .iter()
                    .find(|r| r.comment_id == comment_id)
                    .map(|r| (r.answer.clone().unwrap_or_default(), r.follow_ups.clone()))
            });
        if follow_up.is_some() && prior_context.is_none() {
            self.push_toast_error("No completed investigation to follow up on");
            return;
        }

        // A fresh run writes a `Running` marker (empty context/answer) so a
        // crash mid-run reopens visible; a follow-up leaves the existing
        // completed row untouched until its turn lands.
        let created_at = if follow_up.is_none() {
            let marker = crate::db::pr_investigations::PrInvestigation::new_running(
                project_id.clone(),
                pr_number,
                comment_id,
                head_sha.clone(),
                harness.clone(),
                "",
            );
            let created_at = marker.created_at.clone();
            if let Some(db) = self.db.as_ref()
                && let Err(e) = db.upsert_pr_investigation(&marker)
            {
                self.log_warn(
                    "pr_triage",
                    format!("investigation marker write failed: {e}"),
                );
            }
            if let AppMode::PrReview(state) = &mut self.mode {
                upsert_investigation_in_memory(&mut state.investigations, marker);
            }
            created_at
        } else {
            crate::db::learning::now_timestamp()
        };

        // Move the pane into the modal loading state.
        let AppMode::PrReview(state) = std::mem::replace(&mut self.mode, AppMode::Normal) else {
            return;
        };
        let pr_url = pr.url.clone();
        self.mode = AppMode::PrInvestigationLoading(PrInvestigationLoadState {
            review: Box::new(state),
            comment_id,
            harness: harness.clone(),
            started_at: std::time::Instant::now(),
            pr_number,
            pr_url,
        });

        self.log_info(
            "pr_triage",
            format!(
                "starting read-only {} of comment {comment_id} on PR #{pr_number} with {}",
                if follow_up.is_some() {
                    "follow-up"
                } else {
                    "investigation"
                },
                harness.display_name()
            ),
        );

        let (tx, rx) = std::sync::mpsc::channel();
        self.pr_review_work.begin_investigation(rx);
        // STRICTLY READ-ONLY (`AMF_PLAN.md`). This worker may only: read the PR
        // over `gh`, build the prompt, and run `HeadlessRunner::run_investigation`
        // (a read-only repo pass repo config cannot loosen). It must never write
        // the worktree, switch tmux sessions, deliver a prompt to an interactive
        // agent, or reach `pr_review_inject_fix` / the Vibeless edit-review path.
        std::thread::spawn(move || {
            let built: std::result::Result<String, String> = (|| {
                let meta = GhCli::pr_meta(&workdir, pr_number)
                    .map_err(|e| format!("couldn't load PR #{pr_number}: {e}"))?;
                let changed: Vec<InvestigationChangedFile> = meta
                    .files
                    .iter()
                    .map(InvestigationChangedFile::from_gh)
                    .collect();
                let follow_up_ctx = prior_context
                    .as_ref()
                    .zip(follow_up_question.as_deref())
                    .map(|((initial, turns), question)| InvestigationFollowUp {
                        initial_answer: initial.as_str(),
                        prior_turns: turns.as_slice(),
                        question,
                    });
                let ctx = InvestigationPromptContext {
                    comment: &comment,
                    pr_number,
                    pr_title: &meta.title,
                    pr_description: &meta.body,
                    changed_files: &changed,
                    follow_up: follow_up_ctx,
                    user_context: user_context.as_deref(),
                };
                Ok(build_investigation_prompt(&ctx))
            })();
            let (context_snapshot, result) = match built {
                Ok(prompt) => {
                    let answer = HeadlessRunner::run_investigation(&harness, &workdir, &prompt)
                        .map_err(|e| investigation_failure_message(&harness, &e));
                    (prompt, answer)
                }
                Err(e) => (String::new(), Err(e)),
            };
            let _ = tx.send(InvestigationOutcome {
                project_id,
                pr_number,
                comment_id,
                harness,
                head_sha,
                context_snapshot,
                created_at,
                follow_up_question,
                result,
            });
        });
        self.message = None;
    }

    /// Apply a finished investigation and restore the PR Triage pane. A result
    /// that lands after the user cancelled (`Esc` on the loading frame) is still
    /// persisted so a reopen isn't stuck on `Running`.
    pub fn poll_pr_investigation_bg(&mut self) -> bool {
        let Some(result) = self.pr_review_work.poll_investigation() else {
            return false;
        };
        let outcome = match result {
            Ok(o) => o,
            Err(std::sync::mpsc::TryRecvError::Empty) => return false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pr_review_work.cancel_investigation();
                if let AppMode::PrInvestigationLoading(load) =
                    std::mem::replace(&mut self.mode, AppMode::Normal)
                {
                    self.mode = AppMode::PrReview(*load.review);
                }
                self.push_toast_error("Investigation failed unexpectedly");
                return true;
            }
        };
        self.pr_review_work.cancel_investigation();
        use crate::app::pr_review::PrInvestigationStatus;

        // Restore the pane (still loading = the user didn't bail).
        let AppMode::PrInvestigationLoading(load) =
            std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            // The user cancelled: the fresh-run marker is already reconciled on
            // next open; a follow-up left the completed row intact. Nothing to do.
            return true;
        };
        let mut review = *load.review;

        let (persisted_row, toast): (
            Option<crate::db::pr_investigations::PrInvestigation>,
            Result<String, String>,
        ) = match (&outcome.follow_up_question, &outcome.result) {
            // A follow-up turn: append to the existing row, keep its answer.
            (Some(question), Ok(answer)) => {
                match review
                    .investigations
                    .iter_mut()
                    .find(|r| r.comment_id == outcome.comment_id)
                {
                    Some(row) => {
                        row.follow_ups
                            .push(crate::app::pr_review::PrInvestigationTurn {
                                question: question.clone(),
                                answer: answer.clone(),
                                harness: outcome.harness.clone(),
                                created_at: crate::db::learning::now_timestamp(),
                            });
                        row.status = PrInvestigationStatus::Complete;
                        (Some(row.clone()), Ok("Follow-up answered".to_string()))
                    }
                    None => (
                        None,
                        Err("the investigation it belonged to is gone".to_string()),
                    ),
                }
            }
            (Some(_), Err(e)) => (None, Err(format!("Follow-up failed: {e}"))),
            // A fresh investigation: replace the row wholesale.
            (None, result) => {
                let (status, answer, error) = match result {
                    Ok(a) => (PrInvestigationStatus::Complete, Some(a.clone()), None),
                    Err(e) => (PrInvestigationStatus::Failed, None, Some(e.clone())),
                };
                let row = crate::db::pr_investigations::PrInvestigation {
                    project_id: outcome.project_id.clone(),
                    pr_number: outcome.pr_number,
                    comment_id: outcome.comment_id,
                    head_sha: outcome.head_sha.clone(),
                    harness: outcome.harness.clone(),
                    context_snapshot: outcome.context_snapshot.clone(),
                    answer,
                    follow_ups: Vec::new(),
                    status,
                    error,
                    created_at: outcome.created_at.clone(),
                    updated_at: outcome.created_at.clone(),
                };
                upsert_investigation_in_memory(&mut review.investigations, row.clone());
                let toast = match result {
                    Ok(_) => Ok("Investigation complete".to_string()),
                    Err(e) => Err(format!("Investigation failed: {e}")),
                };
                (Some(row), toast)
            }
        };

        if let (Some(row), Some(db)) = (&persisted_row, self.db.as_ref())
            && let Err(e) = db.upsert_pr_investigation(row)
        {
            self.log_warn("pr_triage", format!("investigation write failed: {e}"));
        }

        self.mode = AppMode::PrReview(review);
        match toast {
            Ok(msg) => self.push_toast_success(msg),
            Err(msg) => self.push_toast_error(msg),
        }
        true
    }

    /// `Esc` on the loading frame: abandon the wait and go back to triage. The
    /// background run keeps going to completion (its `gh`/harness process is
    /// already spawned); its result is discarded but the row is marked failed so
    /// a reopen isn't stuck on `Running`.
    pub fn pr_investigation_cancel(&mut self) {
        let AppMode::PrInvestigationLoading(load) =
            std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            return;
        };
        self.pr_review_work.cancel_investigation();
        use crate::app::pr_review::PrInvestigationStatus;
        let mut review = *load.review;
        let project_id = self
            .feature_indices_for_workdir(&review.workdir)
            .map(|(pi, _)| self.store.projects[pi].id.clone());
        let pr_number = review.review.pr.number;
        // Only a fresh run leaves a `Running` marker to reconcile; a cancelled
        // follow-up leaves its completed row exactly as it was.
        let cancelled_fresh_run = review
            .investigations
            .iter_mut()
            .find(|r| r.comment_id == load.comment_id && r.status == PrInvestigationStatus::Running)
            .map(|row| {
                row.status = PrInvestigationStatus::Failed;
                row.error = Some("investigation cancelled".into());
            })
            .is_some();
        if cancelled_fresh_run && let (Some(db), Some(project_id)) = (self.db.as_ref(), project_id)
        {
            let _ = db.finish_pr_investigation(
                &project_id,
                pr_number,
                load.comment_id,
                None,
                PrInvestigationStatus::Failed,
                Some("investigation cancelled"),
            );
        }
        self.mode = AppMode::PrReview(review);
        self.push_toast_info("Investigation cancelled");
    }

    // ── Completed-investigation actions (the `a` menu) ──────────

    /// The investigation for the selected comment, if any.
    pub(super) fn pr_investigation_selected(
        &self,
    ) -> Option<&crate::db::pr_investigations::PrInvestigation> {
        let AppMode::PrReview(state) = &self.mode else {
            return None;
        };
        let comment_id = state.selected_comment()?.id;
        state
            .investigations
            .iter()
            .find(|r| r.comment_id == comment_id)
    }

    /// Whether the selected comment has an investigation whose action menu is
    /// worth opening (it finished, one way or another).
    pub fn pr_review_investigation_actionable(&self) -> bool {
        self.pr_investigation_selected().is_some_and(|r| {
            use crate::app::pr_review::PrInvestigationStatus;
            matches!(
                r.status,
                PrInvestigationStatus::Complete
                    | PrInvestigationStatus::Failed
                    | PrInvestigationStatus::Dismissed
            )
        })
    }

    /// Open the completed-investigation action menu for the selected comment.
    pub fn pr_review_open_investigation_actions(&mut self) {
        if self.pr_review_work.investigation_pending() {
            return;
        }
        if !self.pr_review_investigation_actionable() {
            self.message =
                Some("No finished investigation on this comment — press v to run one".into());
            return;
        }
        let comment_id = match &self.mode {
            AppMode::PrReview(state) => match state.selected_comment() {
                Some(c) => c.id,
                None => return,
            },
            _ => return,
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            state.investigation_action_pick = Some(InvestigationActionPick {
                comment_id,
                selected: 0,
            });
        }
    }

    pub fn pr_review_investigation_action_picking(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state) if state.investigation_action_pick.is_some()
        )
    }

    pub fn pr_review_investigation_action_move(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(pick) = &mut state.investigation_action_pick
        {
            let len = InvestigationAction::ALL.len();
            pick.selected = (pick.selected as isize + delta).rem_euclid(len as isize) as usize;
        }
    }

    pub fn pr_review_investigation_action_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.investigation_action_pick = None;
        }
    }

    /// Apply the highlighted action to the selected comment's investigation.
    pub fn pr_review_investigation_action_confirm(&mut self) -> Result<()> {
        let action = match &mut self.mode {
            AppMode::PrReview(state) => {
                let Some(pick) = state.investigation_action_pick.take() else {
                    return Ok(());
                };
                // Re-resolve the selection to the menu's comment so a background
                // refresh that moved the cursor can't misfire the action.
                if let Some(idx) = state
                    .review
                    .comments
                    .iter()
                    .position(|c| c.id == pick.comment_id)
                {
                    state.selected = idx;
                }
                InvestigationAction::ALL[pick.selected]
            }
            _ => return Ok(()),
        };
        match action {
            InvestigationAction::PostReply => self.pr_investigation_post_reply(),
            InvestigationAction::AskFollowUp => self.pr_investigation_start_follow_up(),
            InvestigationAction::Dismiss => self.pr_investigation_dismiss(),
            InvestigationAction::KeepAsTodo => self.pr_investigation_keep_as_todo()?,
        }
        Ok(())
    }

    /// Open an editable reply draft for the selected comment, seeded from its
    /// investigation answer. Approving in the reply dialog posts via the
    /// existing `gh` path (`pr_review_post_reply`) and marks the comment
    /// `Replied`.
    pub(super) fn pr_investigation_post_reply(&mut self) {
        let seed = match &self.mode {
            AppMode::PrReview(state) => {
                let comment_id = match state.selected_comment() {
                    Some(c) => c.id,
                    None => return,
                };
                state
                    .investigations
                    .iter()
                    .find(|r| r.comment_id == comment_id)
                    .and_then(|r| r.answer.clone())
                    .filter(|a| !a.trim().is_empty())
            }
            _ => return,
        };
        let Some(answer) = seed else {
            self.push_toast_error("No investigation answer to reply with");
            return;
        };
        self.open_reply(
            ReplyKind::Investigation,
            format!(
                "_From a read-only investigation of this comment:_\n\n{}",
                answer.trim()
            ),
        );
    }

    /// Mark the selected comment's investigation `Dismissed` (in memory and,
    /// best-effort, on disk). The answer is kept.
    pub(super) fn pr_investigation_dismiss(&mut self) {
        use crate::app::pr_review::PrInvestigationStatus;
        let (project_id, pr_number, comment_id) = match &mut self.mode {
            AppMode::PrReview(state) => {
                let Some(comment_id) = state.selected_comment().map(|c| c.id) else {
                    return;
                };
                let Some(row) = state
                    .investigations
                    .iter_mut()
                    .find(|r| r.comment_id == comment_id)
                else {
                    return;
                };
                row.status = PrInvestigationStatus::Dismissed;
                (row.project_id.clone(), row.pr_number, comment_id)
            }
            _ => return,
        };
        if let Some(db) = self.db.as_ref() {
            let _ = db.set_pr_investigation_status(
                &project_id,
                pr_number,
                comment_id,
                PrInvestigationStatus::Dismissed,
            );
        }
        self.push_toast_info("Investigation dismissed — the finding is kept");
    }

    /// Write the selected comment's investigation answer to a scoped TODO list
    /// (the feature's worktree list, or the project's at the repo root — the
    /// same target Learning Mode's `a` uses).
    pub(super) fn pr_investigation_keep_as_todo(&mut self) -> Result<()> {
        let (comment_locator, title, body) = match &self.mode {
            AppMode::PrReview(state) => {
                let Some(comment) = state.selected_comment() else {
                    return Ok(());
                };
                let Some(inv) = state
                    .investigations
                    .iter()
                    .find(|r| r.comment_id == comment.id)
                else {
                    self.message = Some("No investigation to keep".into());
                    return Ok(());
                };
                let locator = match (&comment.path, comment.line) {
                    (Some(p), Some(l)) if !comment.file_level => format!("{p}:{l}"),
                    (Some(p), _) => p.clone(),
                    (None, _) => format!("PR #{}", state.review.pr.number),
                };
                let answer = inv.answer.clone().unwrap_or_default();
                let title = answer
                    .lines()
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or("PR investigation finding")
                    .chars()
                    .take(120)
                    .collect::<String>();
                let body = format!(
                    "From a read-only investigation of a PR review comment ({locator}).\n\n{answer}"
                );
                (locator, title, body)
            }
            _ => return Ok(()),
        };
        let _ = comment_locator;

        let Some((pi, fi)) = self.pr_review_target_feature() else {
            self.push_toast_error("Can't keep as TODO: no feature for this PR");
            return Ok(());
        };
        let has_session = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .is_some_and(|f| f.has_todos_session());
        if !has_session && let Err(e) = self.add_todos_session_for_picker(pi, fi, None) {
            self.push_toast_error(format!("Couldn't start a TODO list: {e}"));
            return Ok(());
        }
        let Some(scope) = self.default_todo_scope(pi, fi) else {
            self.push_toast_error("Couldn't resolve a TODO scope");
            return Ok(());
        };
        let feature_id = self
            .store
            .projects
            .get(pi)
            .and_then(|p| p.features.get(fi))
            .map(|f| f.id.clone());
        let written = self.db.as_ref().map(|db| {
            db.load_or_create_todo_list(&scope, feature_id.as_deref())
                .and_then(|list| {
                    db.add_todo(
                        &list.id,
                        &title,
                        Some(&body),
                        crate::db::todos::TodoPriority::Med,
                    )
                })
        });
        match written {
            Some(Ok(_)) => self.push_toast_success("Kept the finding on the TODO list"),
            Some(Err(e)) => self.push_toast_error(format!("Couldn't add the TODO: {e}")),
            None => self.push_toast_warning("No database — the TODO wasn't saved"),
        }
        Ok(())
    }

    /// Open the follow-up question editor for the selected comment's
    /// investigation (Learning Mode `F`).
    pub(super) fn pr_investigation_start_follow_up(&mut self) {
        let comment_id = match &self.mode {
            AppMode::PrReview(state) => match state.selected_comment() {
                Some(c) => c.id,
                None => return,
            },
            _ => return,
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            state.investigation_follow_up = Some(InvestigationFollowUpDraft {
                comment_id,
                editor: crate::editor::TextEditor::new(String::new()),
            });
        }
    }

    pub fn pr_review_investigation_follow_up_open(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state) if state.investigation_follow_up.is_some()
        )
    }

    pub fn pr_review_investigation_follow_up_editor_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(draft) = &mut state.investigation_follow_up
        {
            draft.editor.handle_key(key);
        }
    }

    pub fn pr_review_investigation_follow_up_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.investigation_follow_up = None;
        }
    }

    /// Submit the follow-up question: park it and open the per-run harness
    /// picker; confirming that starts the blocking follow-up run.
    pub fn pr_review_investigation_follow_up_submit(&mut self) {
        let parked = match &mut self.mode {
            AppMode::PrReview(state) => {
                let Some(draft) = &state.investigation_follow_up else {
                    return;
                };
                let question = draft.editor.text().trim().to_string();
                if question.is_empty() {
                    self.message = Some("Type a follow-up question first".into());
                    return;
                }
                let comment_id = draft.comment_id;
                state.investigation_follow_up = None;
                state.pending_follow_up = Some(PendingFollowUp {
                    comment_id,
                    question,
                });
                true
            }
            _ => false,
        };
        if parked {
            // Reuse the per-run harness picker; its confirm reads
            // `pending_follow_up` and routes into the follow-up run.
            self.pr_review_start_investigation();
        }
    }

    /// Open the optional investigation-context edit box (`e`), seeded from the
    /// note already attached (empty on first open). A no-op if it's already
    /// open or the pane isn't in PR Triage.
    pub fn pr_review_investigation_context_open(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && state.investigation_context.editor.is_none()
        {
            let seed = state.investigation_context.note.clone();
            state.investigation_context.editor = Some(crate::editor::TextEditor::new(seed));
        }
    }

    pub fn pr_review_investigation_context_editing(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state) if state.investigation_context.editor.is_some()
        )
    }

    /// Feed a keystroke to the context editor, re-checking the length cap after
    /// each change (a paste that overflows is rejected whole, matching the
    /// plan-interview custom-answer box).
    pub fn pr_review_investigation_context_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(editor) = &mut state.investigation_context.editor
        {
            let before = editor.text().to_string();
            editor.handle_key(key);
            if editor.text().chars().count() > INVESTIGATION_CONTEXT_MAX_LEN {
                *editor = crate::editor::TextEditor::new(before);
            }
        }
    }

    /// Commit the editor's text to the persistent note and close the box
    /// (`Enter`). Trimmed; an all-whitespace note clears back to "none".
    pub fn pr_review_investigation_context_commit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(editor) = state.investigation_context.editor.take()
        {
            state.investigation_context.note = editor.text().trim().to_string();
        }
    }

    /// Discard the in-progress edit and close the box (`Esc`); the previously
    /// committed note is untouched.
    pub fn pr_review_investigation_context_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.investigation_context.editor = None;
        }
    }
}
