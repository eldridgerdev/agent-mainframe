use serde::Serialize;

use crate::{gui_contract::SessionTarget, gui_plans::PrecallView, project::AgentKind};

use super::PrTriageTarget;
use crate::gui_github_errors::GithubProblem;

#[derive(Debug, Clone, Serialize)]
pub struct PrBatchFixView {
    pub comment_ids: Vec<u64>,
    pub target: SessionTarget,
    pub destination_revision: String,
    pub destination_kind: crate::gui_pr_workspace::DestinationKind,
    /// Binds the comment set, PR head, destination kind, session and its revision.
    pub digest: String,
    pub prompt: String,
    pub submission_prompt: Option<String>,
    /// Set past the TUI's soft ceilings; the batch is still allowed.
    pub warning: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrFixTargetView {
    pub target: SessionTarget,
    pub label: String,
    pub harness: AgentKind,
    pub stopped: bool,
    /// Why this row cannot be chosen, when more specific than `stopped`
    /// (it is listed disabled rather than silently dropped).
    pub unavailable: Option<String>,
    pub destination_revision: String,
    /// Whether this row is in the opened feature or in a companion of the PR.
    pub destination_kind: crate::gui_pr_workspace::DestinationKind,
    pub feature_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrFixDraftView {
    pub comment_id: u64,
    pub target: SessionTarget,
    pub prompt: String,
    pub submission_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrFixHandoff {
    pub target: SessionTarget,
    pub draft_prompt: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrEntryView {
    pub number: u32,
    /// The PR selected by the feature's current branch, when present.
    pub selected_for_feature: bool,
    pub title: String,
    pub author: String,
    pub head_ref: String,
    pub updated_at: String,
    pub is_draft: bool,
    pub state: String,
    pub mine: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrPickerView {
    pub entries: Vec<PrEntryView>,
    pub include_closed: bool,
    pub error: Option<String>,
    /// `error` as a GitHub problem the user can fix, when it is one.
    pub github_problem: Option<GithubProblem>,
    pub branch_pr: Option<u32>,
    /// The list is still being read; poll for it.
    pub loading: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrThreadReplyView {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub via_amf: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrInvestigationTurnView {
    pub question: String,
    pub answer: String,
    pub harness: AgentKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrInvestigationView {
    pub status: &'static str,
    pub harness: AgentKind,
    pub answer: Option<String>,
    pub error: Option<String>,
    pub follow_ups: Vec<PrInvestigationTurnView>,
    /// The PR head moved since this investigation ran.
    pub stale_head: bool,
    /// Title of the TODO this answer was already kept as, if it still exists.
    pub kept_todo: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrCommentView {
    pub id: u64,
    /// `inline`, `review_summary` or `conversation`.
    pub kind: &'static str,
    pub review_state: Option<String>,
    pub author: String,
    pub is_bot: bool,
    pub path: Option<String>,
    pub line: Option<u32>,
    pub side: Option<String>,
    pub outdated: bool,
    pub file_level: bool,
    pub body: String,
    pub snippet: String,
    pub hunk: Option<String>,
    pub resolved: bool,
    pub can_resolve: bool,
    pub triage: &'static str,
    pub local_note: Option<String>,
    pub actionable: bool,
    pub local_finding: bool,
    pub replies: Vec<PrThreadReplyView>,
    pub investigation: Option<PrInvestigationView>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrReviewView {
    pub number: u32,
    pub url: String,
    pub head_sha: String,
    pub head_ref: String,
    pub branch_mismatch: Option<String>,
    pub fetched_at: String,
    pub open_count: usize,
    pub total: usize,
    pub hide_resolved: bool,
    pub sort: &'static str,
    pub hidden_resolved: usize,
    /// Index into `comments` where the conversation section starts.
    pub conversation_start: Option<usize>,
    pub comments: Vec<PrCommentView>,
    /// The comment whose investigation is running, if any.
    pub investigating: Option<u64>,
    pub investigating_harness: Option<AgentKind>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrReplyDraftView {
    pub comment_id: u64,
    pub kind: &'static str,
    pub seed: String,
    pub agent_drafted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrWriteConfirmView {
    /// `reply`, `resolve` or `reopen`.
    pub kind: &'static str,
    pub comment_id: u64,
    pub destination: String,
    /// The exact text a reply posts, attribution included.
    pub body: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PrTriageView {
    pub workflow_id: String,
    pub revision: u64,
    pub target: PrTriageTarget,
    pub feature_name: String,
    pub branch: String,
    /// `pick`, `loading` or `review`.
    pub stage: &'static str,
    pub picker: Option<PrPickerView>,
    pub loading_pr: Option<u32>,
    pub review: Option<PrReviewView>,
    pub precall: Option<PrecallView>,
    pub reply: Option<PrReplyDraftView>,
    pub write_confirm: Option<PrWriteConfirmView>,
    pub fix_targets: Vec<PrFixTargetView>,
    pub fix_draft: Option<PrFixDraftView>,
    pub batch_fix: Option<PrBatchFixView>,
    /// Taken once by the action response; subsequent polls cannot redeliver.
    pub handoff: Option<PrFixHandoff>,
    pub harnesses: Vec<AgentKind>,
    pub default_harness: Option<AgentKind>,
    pub error: Option<String>,
    /// `error` as a GitHub problem the user can fix, when it is one.
    pub github_problem: Option<GithubProblem>,
    /// The GitHub account `gh` writes as, when it could be read.
    pub github_user: Option<String>,
    pub notice: Option<String>,
}
