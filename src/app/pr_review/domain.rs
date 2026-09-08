use super::strip_bot_boilerplate;
use crate::app::session_kind_for_agent;
use crate::app::{AgentKind, Feature, ReplyState, SessionKind};
use crate::github::PrRef;
use anyhow::Result;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

/// Snippet length (chars) shown in the comment list.
pub(super) const SNIPPET_LEN: usize = 80;

/// Default label (and de-facto identity) of a dedicated PR-triage agent
/// session. Users can choose another label before the first fix so multiple
/// triage sessions can coexist; keeping this default preserves the original
/// found-or-created behavior for an unnamed session.
pub(crate) const TRIAGE_SESSION_LABEL: &str = "PR Triage";

/// Label used before the feature was renamed to PR Triage. Keep recognizing it
/// so an upgrade reuses an already-running dedicated session instead of quietly
/// creating a second one.
pub(super) const LEGACY_REVIEW_SESSION_LABEL: &str = "PR Review";

/// Soft ceilings for the combined-batch prompt (`B`). Past either, the confirm
/// dialog still opens but a warning toast fires so the user knows a single
/// prompt this large risks blowing the agent's context window (plan: "keep the
/// set bounded"). They gate a warning, not the action.
pub(super) const BATCH_COMBINED_COMMENT_WARN: usize = 15;

pub(super) const BATCH_COMBINED_TOKEN_WARN: usize = 6000;

/// Parse a GitHub-provided hunk and retain only the lines immediately around
/// its comment anchor. The synthetic file headers let the regular unified-diff
/// parser do the fiddly line-kind/header work without maintaining a second
/// parser here.
pub(super) fn window_github_hunk(
    text: &str,
    line: usize,
    old_side: bool,
    context: usize,
) -> Option<String> {
    let synthetic = format!(
        "diff --git a/__amf_comment__ b/__amf_comment__\n\
         --- a/__amf_comment__\n\
         +++ b/__amf_comment__\n{text}\n"
    );
    let files = crate::diff::parse_unified_diff(&synthetic).ok()?;
    let hunk = files.first()?.hunks.first()?;
    window_parsed_hunk(hunk, line, old_side, context)
}

/// Render a bounded slice of a parsed hunk centered on `line`. `old_side`
/// selects base-file numbering for comments on removed lines; otherwise the
/// current-file numbering is used.
pub(crate) fn window_parsed_hunk(
    hunk: &crate::diff::DiffHunk,
    line: usize,
    old_side: bool,
    context: usize,
) -> Option<String> {
    // Walk the hunk tracking the old/new line number *at* each entry (before
    // that line is consumed), both to find the target line's index and to
    // know the old/new start of whatever window we slice out below.
    let mut old_line = hunk.old_start;
    let mut new_line = hunk.new_start;
    let mut line_starts = Vec::with_capacity(hunk.lines.len());
    let mut target_idx = None;
    for (i, l) in hunk.lines.iter().enumerate() {
        line_starts.push((old_line, new_line));
        match l.kind {
            crate::diff::DiffLineKind::Context => {
                let candidate = if old_side { old_line } else { new_line };
                if target_idx.is_none() && candidate == line {
                    target_idx = Some(i);
                }
                old_line += 1;
                new_line += 1;
            }
            crate::diff::DiffLineKind::Added => {
                if !old_side && target_idx.is_none() && new_line == line {
                    target_idx = Some(i);
                }
                new_line += 1;
            }
            crate::diff::DiffLineKind::Removed => {
                if old_side && target_idx.is_none() && old_line == line {
                    target_idx = Some(i);
                }
                old_line += 1;
            }
            crate::diff::DiffLineKind::NoNewlineMarker => {}
        }
    }
    let target_idx = target_idx?;

    let start_idx = target_idx.saturating_sub(context);
    let end_idx = (target_idx + context + 1).min(hunk.lines.len());
    let window = &hunk.lines[start_idx..end_idx];
    let (window_old_start, window_new_start) = line_starts[start_idx];
    let (mut window_old_count, mut window_new_count) = (0usize, 0usize);
    for l in window {
        match l.kind {
            crate::diff::DiffLineKind::Context => {
                window_old_count += 1;
                window_new_count += 1;
            }
            crate::diff::DiffLineKind::Added => window_new_count += 1,
            crate::diff::DiffLineKind::Removed => window_old_count += 1,
            crate::diff::DiffLineKind::NoNewlineMarker => {}
        }
    }

    let mut text = format!(
        "@@ -{window_old_start},{window_old_count} +{window_new_start},{window_new_count} @@"
    );
    for l in window {
        if matches!(l.kind, crate::diff::DiffLineKind::NoNewlineMarker) {
            continue;
        }
        text.push('\n');
        text.push_str(&l.text);
    }
    Some(text)
}

/// Lighter disclosure appended to a reply the user wrote (or edited) through
/// PR Triage's "Done in `<sha>`"/"not needed" templates — distinct from
/// [`append_ai_attribution`], which marks content the AI *generated*. This
/// marks the *channel*, not the authorship: the words are the user's own,
/// but a reader on GitHub should be able to tell the reply was posted
/// through tooling rather than typed directly into the GitHub UI. Applied at
/// post time (not part of the editable seed) so composing a "not needed"
/// reason — which starts from an empty buffer — isn't complicated by a
/// footer already sitting in the editor.
pub(crate) const AMF_ATTRIBUTION_FOOTER: &str = "— posted via AMF";

pub(crate) const AI_ATTRIBUTION_FOOTER: &str = "— drafted by AI via AMF";

/// Attribution on findings and summaries posted by the separate AI Review
/// workflow. These are still actionable review findings after refresh; the
/// footer identifies their origin without turning them into follow-up replies.
pub(crate) const AI_REVIEW_ATTRIBUTION_FOOTER: &str = "— AI review via AMF";

/// Which agent session AMF asked for a reply draft, captured at fix injection
/// and persisted with the draft (`db::pr_comment_triage::begin_reply_draft`).
///
/// The disclosure has to describe the session that actually wrote the draft.
/// Reading that off the pane's *current* fix target instead would attribute it
/// to whatever is selected by the time the reply opens: re-entering PR Triage
/// resets the target to the default, and deleting the session leaves nothing to
/// read at all — so a Codex draft could be posted as Claude's work, or lose its
/// disclosure entirely. Pinning the session id at injection is what makes the
/// later reading verifiable rather than assumed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyDraftProvenance {
    pub harness: String,
    /// AMF feature-session id of the session the fix was injected into.
    pub session_id: String,
    /// Model as known at injection time — usually `None`, since a session
    /// created by this very fix has no transcript yet. Kept as the fallback for
    /// when the session no longer exists to be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The target session's usage immediately before the fix ran. The
    /// disclosure reports the delta against it, so a long-lived triage session's
    /// earlier spend is not billed to this draft.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_baseline: Option<crate::token_tracking::SessionTokenUsage>,
}

/// Best-effort provenance attached to an unchanged agent-written reply draft.
/// Usage and cost are the drafting session's delta since the fix was injected:
/// that is the narrowest reliable accounting boundary shared by every
/// interactive harness without asking the agent to self-report its own usage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyGenerationMetadata {
    pub harness: Option<String>,
    pub model: Option<String>,
    pub estimated_tokens: Option<u64>,
    pub estimated_cost: Option<String>,
    /// Set when this comment was resolved as part of a combined batch (`B`):
    /// the `estimated_*` figures are the whole run's cost, shared across every
    /// resolved comment in the batch. Drives the `· combined (N)` marker in the
    /// disclosure and the batch note in the posted reply.
    pub combined_batch: Option<crate::app::fix_cost::CombinedBatch>,
}

impl ReplyGenerationMetadata {
    /// An agent draft whose provenance was never recorded (written before AMF
    /// persisted it) or no longer decodes. The reply still discloses that it
    /// was AI-generated and says every detail is unknown — the one thing it
    /// must not do is fill the gap from an unrelated session.
    pub fn unattributed() -> Self {
        Self {
            harness: None,
            model: None,
            estimated_tokens: None,
            estimated_cost: None,
            combined_batch: None,
        }
    }

    pub fn source_disclosure(&self) -> String {
        format!(
            "AI generation: harness {} · model {}",
            self.harness.as_deref().unwrap_or("unreported"),
            self.model.as_deref().unwrap_or("unreported")
        )
    }

    pub fn usage_disclosure(&self) -> String {
        let tokens = self
            .estimated_tokens
            .map(crate::token_tracking::format_token_count)
            .map(|tokens| format!("~{tokens}"))
            .unwrap_or_else(|| "unavailable".to_string());
        format!(
            "estimated tokens {tokens} · {}",
            crate::app::fix_cost::fix_cost_line(
                self.estimated_cost.as_deref(),
                self.combined_batch
            )
        )
    }

    /// Compact GitHub-flavored Markdown line inserted immediately above the
    /// stable attribution footer. Missing provider telemetry is explicit rather
    /// than silently dropping one of the promised provenance fields.
    ///
    /// A batched fix adds a second italic line spelling out — for a PR reader
    /// who isn't an AMF user — that this comment was one of several fixed in a
    /// single agent run and that the cost above is the whole run's, shared.
    pub fn disclosure(&self) -> String {
        let mut out = format!(
            "_{} · {}_",
            self.source_disclosure(),
            self.usage_disclosure()
        );
        if let Some(batch) = self.combined_batch {
            let n = batch.sibling_count.max(1);
            let comments = if n == 1 { "comment" } else { "comments" };
            out.push_str(&format!(
                "\n_Fixed as one of {n} {comments} handled in a single combined agent run; the fix cost above is that run's total, shared across them._"
            ));
        }
        out
    }
}

pub(super) fn append_amf_attribution(body: &str) -> String {
    format!("{}\n\n{}", body.trim_end(), AMF_ATTRIBUTION_FOOTER)
}

/// Attribution for reply text drafted by an agent harness. Provider-neutral
/// wording stays accurate for Claude, Codex, OpenCode, and Pi.
///
/// Reply-flow only — [`reply_posted_via_amf`] treats this footer as proof a
/// reply went through AMF's `R`/`n` dialog, so anything else that wants
/// AI-authorship attribution (e.g. `ai_review`'s posted findings) needs its
/// own distinct footer rather than reusing this one, or it would falsely
/// register as an AMF-posted reply.
pub(crate) fn append_ai_attribution(
    body: &str,
    metadata: Option<&ReplyGenerationMetadata>,
) -> String {
    match metadata {
        Some(metadata) => format!(
            "{}\n\n{}\n\n{}",
            body.trim_end(),
            metadata.disclosure(),
            AI_ATTRIBUTION_FOOTER
        ),
        None => format!("{}\n\n{}", body.trim_end(), AI_ATTRIBUTION_FOOTER),
    }
}

pub(super) fn append_reply_attribution(
    body: &str,
    agent_drafted: bool,
    metadata: Option<&ReplyGenerationMetadata>,
) -> String {
    if agent_drafted {
        append_ai_attribution(body, metadata)
    } else {
        append_amf_attribution(body)
    }
}

/// Whether `reply`'s current text still matches what it was seeded with. A
/// captured agent draft only earns AI-authorship attribution while it stays
/// unedited; once the user changes it — even down to entirely their own
/// words — the posted text is no longer purely the agent's own, so it falls
/// back to channel-only AMF attribution.
pub fn reply_effective_agent_drafted(reply: &ReplyState) -> bool {
    reply.agent_drafted && reply.editor.text().trim() == reply.original_seed.trim()
}

/// Which agent session a "fix" prompt is injected into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum FixTarget {
    /// A single dedicated triage session, spun up once and reused for every fix
    /// in the PR. The default: per-session overhead (system prompt, tool
    /// definitions, skills) is paid once and file reads amortize across
    /// comments, and review work stays out of the user's working session.
    #[default]
    DedicatedReview,
    /// The feature's existing live agent session — warm in-progress context, at
    /// the cost of carrying that session's unrelated conversation into each fix.
    ExistingLive,
    /// A **companion triage feature**: its own worktree, its own tmux session,
    /// and its own harness/vibe-mode chosen independently of the source
    /// feature. The isolated option — worktree-local hooks and permissions the
    /// triage agent writes can't mutate the source feature — at the cost of an
    /// explicit integration step to land the fixes on the PR branch.
    NewFeature,
    /// Another feature that already exists in the store — its live agent
    /// session receives the "address the feedback" prompt. Final-review only:
    /// PR Triage's picker never offers this row (it has no reason to route a
    /// PR's fixes into an unrelated feature), so PR-Triage code paths treat it
    /// exactly like [`FixTarget::ExistingLive`].
    ExistingFeature,
}

impl FixTarget {
    /// Short human label for footers / toasts.
    pub fn label(self) -> &'static str {
        match self {
            FixTarget::DedicatedReview => "dedicated triage session",
            FixTarget::ExistingLive => "existing live session",
            FixTarget::NewFeature => "triage feature",
            FixTarget::ExistingFeature => "another feature's session",
        }
    }

    /// Compact footer tag.
    pub fn tag(self) -> &'static str {
        match self {
            FixTarget::DedicatedReview => "dedicated",
            FixTarget::ExistingLive => "live",
            FixTarget::NewFeature => "new feature",
            FixTarget::ExistingFeature => "other feature",
        }
    }

    /// Whether this target's session lives in a **companion** feature rather
    /// than the feature PR Triage was opened from.
    pub fn is_companion_feature(self) -> bool {
        matches!(self, FixTarget::NewFeature)
    }
}

/// One row of the fix-target picker (`HarnessPickState`): either the
/// feature's existing live session, or a dedicated triage session pinned to
/// a specific harness. Choosing a row resolves both `FixTarget` and (for the
/// dedicated case) `review_harness` in one step.
#[derive(Debug, Clone, PartialEq)]
pub enum FixTargetPickRow {
    /// Reuse the feature's existing live agent session. Carries that
    /// session's label (e.g. "Claude 2") when one already exists, so the
    /// picker names exactly where a fix lands instead of a generic
    /// fallback; `None` when no live agent session exists yet to resolve a
    /// name from.
    ExistingLive(Option<String>),
    /// Spin up (or reuse) the dedicated triage session on this harness.
    Dedicated(AgentKind),
    /// Create a **companion triage feature**: its own worktree, its own tmux
    /// session, and harness/vibe-mode chosen independently of the source
    /// feature. Choosing it opens the compact setup overlay rather than
    /// resolving a harness inline, so it carries no `AgentKind` of its own.
    NewFeature,
}

impl FixTargetPickRow {
    /// Display label for the picker list.
    pub fn label(&self) -> String {
        match self {
            FixTargetPickRow::ExistingLive(Some(name)) => {
                format!("Existing live session ({name})")
            }
            FixTargetPickRow::ExistingLive(None) => "Existing live session".to_string(),
            FixTargetPickRow::Dedicated(agent) => {
                format!("Dedicated triage session ({})", agent.display_name())
            }
            FixTargetPickRow::NewFeature => {
                "New feature… (isolated worktree, own harness + mode)".to_string()
            }
        }
    }
}

/// Order the comment list is shown in. Cycled with `o`; independent of the
/// `hide_resolved` filter. Sorting is stable, so comments that tie on the sort
/// key (e.g. same file, or all-human/all-bot) keep their original fetch order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PrSortMode {
    /// The order `gh` returned them in.
    #[default]
    FetchOrder,
    /// Grouped by file path; comments with no path (conversation/summary) sort
    /// after every file.
    ByFile,
    /// Grouped alphabetically by author login.
    ByAuthor,
    /// Human-authored comments first, bot comments after.
    HumansFirst,
    /// Conversation comments (no `path`/resolution, top-level PR discussion)
    /// grouped into their own section after everything anchored to code —
    /// resolves the "group conversation comments" open question. The list
    /// draws a divider ahead of the group (`draw_comment_list`) so it reads
    /// as a real section, not just a silent reorder.
    Conversations,
}

impl PrSortMode {
    /// Advance to the next mode, wrapping back to `FetchOrder`.
    pub fn next(self) -> Self {
        match self {
            PrSortMode::FetchOrder => PrSortMode::ByFile,
            PrSortMode::ByFile => PrSortMode::ByAuthor,
            PrSortMode::ByAuthor => PrSortMode::HumansFirst,
            PrSortMode::HumansFirst => PrSortMode::Conversations,
            PrSortMode::Conversations => PrSortMode::FetchOrder,
        }
    }

    /// Short label for the footer / toast.
    pub fn label(self) -> &'static str {
        match self {
            PrSortMode::FetchOrder => "fetch order",
            PrSortMode::ByFile => "by file",
            PrSortMode::ByAuthor => "by author",
            PrSortMode::HumansFirst => "humans first",
            PrSortMode::Conversations => "conversations last",
        }
    }
}

/// Index of the session a fix should target within a feature, given the
/// strategy. For [`FixTarget::DedicatedReview`], `None` means no session with
/// `dedicated_label` exists yet and one must be created; for
/// [`FixTarget::ExistingLive`], `None` means there is no live agent session to
/// reuse. `dedicated_label` lets callers reuse this for their own dedicated
/// session (e.g. the final review's "Final Review" window vs PR review's).
pub(crate) fn fix_session_index(
    feature: &Feature,
    target: FixTarget,
    dedicated_label: &str,
) -> Option<usize> {
    match target {
        // `ExistingFeature` is final-review's "route into a feature you pick"
        // row; once the caller has swapped in that feature, resolving its first
        // agent session is identical to `ExistingLive`.
        FixTarget::ExistingLive | FixTarget::ExistingFeature => feature
            .sessions
            .iter()
            .position(|s| s.kind.is_agent_harness()),
        // `NewFeature` resolves the same labelled session, just inside the
        // companion feature rather than the source one — callers pass the
        // companion in `feature` (see `App::pr_review_target_feature`).
        FixTarget::DedicatedReview | FixTarget::NewFeature => feature
            .sessions
            .iter()
            .position(|s| s.kind.is_agent_harness() && s.label == dedicated_label),
    }
}

/// Resolve the dedicated PR-triage session, preferring the current label while
/// retaining compatibility with sessions created under the old "PR Review"
/// label. Existing-live targeting is unchanged.
pub(crate) fn pr_triage_session_index(feature: &Feature, target: FixTarget) -> Option<usize> {
    pr_triage_session_index_named(feature, target, TRIAGE_SESSION_LABEL)
}

/// Resolve the PR-triage session selected for this pane visit. Legacy
/// `PR Review` compatibility only applies to the default name: a custom name
/// is an exact identity and must never silently resolve to a different window.
pub(crate) fn pr_triage_session_index_named(
    feature: &Feature,
    target: FixTarget,
    dedicated_label: &str,
) -> Option<usize> {
    fix_session_index(feature, target, dedicated_label).or_else(|| {
        (target == FixTarget::DedicatedReview && dedicated_label == TRIAGE_SESSION_LABEL)
            .then(|| fix_session_index(feature, target, LEGACY_REVIEW_SESSION_LABEL))
            .flatten()
    })
}

/// Resolve a named triage session while honoring an explicitly selected
/// harness. A label is the session identity, so finding that label on another
/// harness is a conflict rather than permission to silently reuse it.
pub(super) fn pr_triage_session_index_named_for_harness(
    feature: &Feature,
    target: FixTarget,
    dedicated_label: &str,
    harness: Option<&AgentKind>,
) -> Result<Option<usize>> {
    let Some(si) = pr_triage_session_index_named(feature, target, dedicated_label) else {
        return Ok(None);
    };
    let Some(harness) = harness.filter(|_| target == FixTarget::DedicatedReview) else {
        return Ok(Some(si));
    };
    let session = &feature.sessions[si];
    if session.kind != session_kind_for_agent(harness) {
        let existing_harness = match session.kind {
            SessionKind::Claude => "Claude",
            SessionKind::Opencode => "Opencode",
            SessionKind::Codex => "Codex",
            SessionKind::Pi => "Pi",
            _ => "another harness",
        };
        anyhow::bail!(
            "triage session '{}' already runs {existing_harness}; choose {existing_harness} or another session name",
            session.label
        );
    }
    Ok(Some(si))
}

/// What kind of GitHub comment this is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommentKind {
    /// Inline review comment anchored to a file/line.
    Inline,
    /// A review summary body (Approve / Request changes / Comment).
    ReviewSummary { state: String },
    /// A conversation comment on the PR timeline (no code anchor).
    Conversation,
}

/// Local triage decision, persisted in SQLite (`pr_comment_triage`). GitHub
/// thread resolution is the source of truth for "done"; this is the local layer
/// on top of it (a fix was injected, the user marked it done, skipped it, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum TriageState {
    #[default]
    Untriaged,
    Fixing,
    Done,
    Skipped,
    Replied,
}

impl TriageState {
    /// Stable token persisted in SQLite. Kept separate from the `Display`/UI
    /// label so the on-disk encoding never shifts with cosmetic changes.
    pub fn as_db_str(self) -> &'static str {
        match self {
            TriageState::Untriaged => "untriaged",
            TriageState::Fixing => "fixing",
            TriageState::Done => "done",
            TriageState::Skipped => "skipped",
            TriageState::Replied => "replied",
        }
    }

    /// Parse the persisted token back into a state; an unknown token (older or
    /// corrupt row) falls back to [`TriageState::Untriaged`].
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "fixing" => TriageState::Fixing,
            "done" => TriageState::Done,
            "skipped" => TriageState::Skipped,
            "replied" => TriageState::Replied,
            _ => TriageState::Untriaged,
        }
    }

    /// One-char list checkbox marker (plan legend: `[ ]` untriaged, `[x]` done,
    /// `[-]` skipped, `[~]` fixing, `[r]` replied).
    pub fn marker(self) -> char {
        match self {
            TriageState::Untriaged => ' ',
            TriageState::Fixing => '~',
            TriageState::Done => 'x',
            TriageState::Skipped => '-',
            TriageState::Replied => 'r',
        }
    }

    /// Short label for the detail chip / toasts (`None` for untriaged — nothing
    /// to show).
    pub fn label(self) -> Option<&'static str> {
        match self {
            TriageState::Untriaged => None,
            TriageState::Fixing => Some("fixing"),
            TriageState::Done => Some("done"),
            TriageState::Skipped => Some("skipped"),
            TriageState::Replied => Some("replied"),
        }
    }
}

/// Lifecycle of a per-comment read-only investigation (started with `v` in the
/// triage list). Persisted in `pr_investigations`, one row per
/// `(project, PR, comment)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrInvestigationStatus {
    /// The blocking headless run is in flight. Written *before* the call so a
    /// crash mid-run reopens as a visible failed state rather than a silent
    /// gap (reconciled to `Failed` on load, like Learning Mode's stuck runs).
    #[default]
    Running,
    /// The run returned an answer.
    Complete,
    /// The run failed; the row's `error` carries the message.
    Failed,
    /// The operator dismissed the finding. The answer is kept — only the
    /// status changes — so a later reopen shows it was already handled.
    Dismissed,
}

impl PrInvestigationStatus {
    /// Stable SQLite token, kept apart from any UI label.
    pub fn as_db_str(self) -> &'static str {
        match self {
            PrInvestigationStatus::Running => "running",
            PrInvestigationStatus::Complete => "complete",
            PrInvestigationStatus::Failed => "failed",
            PrInvestigationStatus::Dismissed => "dismissed",
        }
    }

    /// Parse a stored token; an unknown/corrupt value degrades to `Failed` so
    /// the row stays visible and re-runnable rather than masquerading as a
    /// finished answer.
    pub fn from_db_str(s: &str) -> Self {
        match s {
            "running" => PrInvestigationStatus::Running,
            "complete" => PrInvestigationStatus::Complete,
            "dismissed" => PrInvestigationStatus::Dismissed,
            _ => PrInvestigationStatus::Failed,
        }
    }
}

/// One follow-up turn on an investigation: the operator's question and the
/// answer a fresh read-only headless run produced with the prior turn as
/// context (Learning Mode `F` behaviour). Serialized as a JSON array in the
/// parent row's `follow_ups` column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrInvestigationTurn {
    pub question: String,
    pub answer: String,
    /// The harness this turn actually ran on (the operator picks per run, so it
    /// can differ from the initial investigation's).
    pub harness: AgentKind,
    pub created_at: String,
}

/// One normalized, display-ready comment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrComment {
    pub id: u64,
    pub kind: CommentKind,
    pub author: String,
    pub is_bot: bool,
    pub path: Option<String>,
    /// Best-known line: the current diff line, falling back to the original.
    pub line: Option<u32>,
    /// GitHub diff side for `line` (`RIGHT`/current or `LEFT`/base).
    /// Older cache rows predate this field and default to the current side.
    #[serde(default)]
    pub side: Option<String>,
    /// True when the comment's anchor line no longer exists in the diff.
    pub outdated: bool,
    /// True when the comment is on the *file* rather than a line (GitHub
    /// `subject_type: "file"`), or when its `diff_hunk` is so large it's
    /// effectively the whole file. Either way the hunk is suppressed in favor of
    /// a bare `File:` reference — see [`PrComment::prompt_hunk`].
    ///
    /// `#[serde(default)]`: cached `pr_review_cache` rows written before this
    /// field existed still deserialize (as `false`, i.e. line-anchored).
    #[serde(default)]
    pub file_level: bool,
    pub diff_hunk: Option<String>,
    /// Original comment body as returned by GitHub.
    pub body: String,
    /// One-line snippet for the list (boilerplate-stripped, truncated).
    pub snippet: String,
    pub in_reply_to: Option<u64>,
    /// GraphQL review-thread node id (inline comments that belong to a thread).
    pub thread_id: Option<String>,
    /// Resolution state from GitHub (source of truth for done/not-done).
    pub is_resolved: bool,
    pub triage: TriageState,
    pub local_note: Option<String>,
    /// Set when this comment was resolved as part of a combined batch (`B`):
    /// every resolved comment in that batch shares the id. Drives the "combined"
    /// badge and sibling highlighting, and is noted in the posted GitHub reply.
    /// `#[serde(default)]` so cached `pr_review_cache` rows written before this
    /// field existed still deserialize (as `None`, i.e. not part of a batch).
    #[serde(default)]
    pub batch_id: Option<String>,
    /// Real GitHub comment/review id, when known independent of `id` (kept for
    /// forward compatibility with cached rows; currently always `None` for a
    /// fetched comment, which already uses `id` directly).
    #[serde(default)]
    pub github_id: Option<u64>,
    /// GitHub review containing this finding. This lets a later refresh finish
    /// identity reconciliation if the immediate post-write fetch failed.
    #[serde(default)]
    pub github_review_id: Option<u64>,
}

/// A hunk without a usable line anchor longer than this is treated as
/// effectively the whole file. Line-anchored comments are safely windowed
/// around their target instead (see [`COMMENT_HUNK_CONTEXT_LINES`]).
pub(super) const WHOLE_FILE_HUNK_LINES: usize = 150;

/// Context retained on either side of a line-anchored review comment. GitHub's
/// `diff_hunk` can encompass an entire newly-added function even when the
/// comment itself points at one line; rendering or injecting all of it makes
/// the referenced code hard to spot and wastes prompt context.
pub(super) const COMMENT_HUNK_CONTEXT_LINES: usize = 3;

impl PrComment {
    /// The diff hunk worth showing and injecting, or `None` when it should be
    /// replaced by a bare `File:` reference — for a file-level comment (whose
    /// hunk is the entire file diff) or an oversized hunk.
    ///
    /// The suppressed case compounds in the combined batch (`B`), where several
    /// whole-file hunks would otherwise land in one prompt.
    pub fn prompt_hunk(&self) -> Option<Cow<'_, str>> {
        let hunk = self.diff_hunk.as_deref()?;
        if self.file_level {
            return None;
        }

        let hunk_lines = hunk.lines().count();
        if hunk_lines > COMMENT_HUNK_CONTEXT_LINES * 2 + 2
            && let Some(line) = self.line
            && let Some(window) = window_github_hunk(
                hunk,
                line as usize,
                self.side.as_deref() == Some("LEFT"),
                COMMENT_HUNK_CONTEXT_LINES,
            )
        {
            return Some(Cow::Owned(window));
        }

        // Keep the old safety net for a malformed/unanchored hunk that cannot
        // be windowed. Valid line-anchored hunks return through the bounded
        // branch above, regardless of their original size.
        if hunk_lines > WHOLE_FILE_HUNK_LINES {
            return None;
        }

        Some(Cow::Borrowed(hunk))
    }

    /// Whether a hunk exists but is being withheld as whole-file-sized. Drives
    /// the "comment on file" note in both the prompt and the detail pane.
    pub fn hunk_suppressed(&self) -> bool {
        self.diff_hunk.is_some() && self.prompt_hunk().is_none()
    }

    /// Text to send to the agent: boilerplate-stripped for bots, verbatim for
    /// humans. Keeps token-heavy bot scaffolding out of prompts.
    pub fn agent_text(&self) -> String {
        if self.is_bot {
            strip_bot_boilerplate(&self.body)
        } else {
            self.body.clone()
        }
    }

    /// Seed text for the "add to memory" dialog: the bot-stripped comment text
    /// with a `file`/`file:line` hint appended, so a finding phrased as a
    /// general rule still carries where it came from. Edited freely before
    /// [`review_memory::append_finding`] writes it as a single bullet
    /// (whitespace/newlines collapsed at that point).
    pub fn memory_finding_seed(&self) -> String {
        let text = self.agent_text().trim().to_string();
        let hint = match &self.path {
            Some(path) if !self.file_level => match self.line {
                Some(line) => Some(format!("{path}:{line}")),
                None => Some(path.clone()),
            },
            Some(path) => Some(path.clone()),
            None => None,
        };
        match hint {
            Some(hint) => format!("{text} ({hint})"),
            None => text,
        }
    }

    /// Assemble the minimal "fix" prompt for this comment: a single instruction
    /// line, the `file:line` pointer, the (bot-stripped) comment text, and the
    /// GitHub-provided diff hunk.
    ///
    /// Deliberately carries **no file contents** — the agent already has the
    /// repo checked out and opens what it needs. This minimal context is the
    /// single biggest token lever (plan token principle #3). The `diff_hunk` is
    /// free: GitHub returns it per inline comment, so including it costs no
    /// extra fetch.
    pub fn fix_prompt(&self) -> String {
        format!(
            "Address this PR review comment.\n{}",
            self.fix_prompt_body()
        )
    }

    /// The per-comment context block shared by the single-comment [`fix_prompt`]
    /// and the combined-batch prompt ([`combined_fix_prompt`]): the `file:line`
    /// pointer, the (bot-stripped) comment text, and the GitHub diff hunk — with
    /// no leading instruction line and no file contents.
    ///
    /// [`fix_prompt`]: Self::fix_prompt
    pub(super) fn fix_prompt_body(&self) -> String {
        let mut out = String::new();

        if let Some(path) = &self.path {
            match self.line {
                Some(line) if !self.file_level => out.push_str(&format!("File: {path}:{line}")),
                _ => out.push_str(&format!("File: {path}")),
            }
            if self.file_level {
                out.push_str("  (comment on the whole file)");
            } else if self.outdated {
                out.push_str("  (comment is on a line that has since changed)");
            }
            out.push('\n');
        }

        out.push_str(&format!(
            "Comment (@{}): {}\n",
            self.author,
            self.agent_text().trim()
        ));

        match self.prompt_hunk() {
            Some(hunk) => {
                out.push_str("Diff hunk:\n");
                out.push_str(hunk.trim_end());
                out.push('\n');
            }
            // A whole-file-sized hunk is withheld rather than injected: say so,
            // so the agent knows to open the file instead of assuming there was
            // no context to give.
            None if self.hunk_suppressed() => {
                out.push_str(
                    "Diff hunk omitted (it covers effectively the whole file) — \
                     open the file for context.\n",
                );
            }
            None => {}
        }

        out.trim_end().to_string()
    }

    /// How a reply to this comment is posted to GitHub. Inline review comments
    /// reply into their thread (via the thread's root comment id); everything
    /// else (conversation comments, review summaries) posts as a new top-level
    /// conversation comment.
    pub fn reply_target(&self) -> ReplyTarget {
        match self.kind {
            CommentKind::Inline => ReplyTarget::InlineThread {
                root_comment_id: self
                    .in_reply_to
                    .unwrap_or(self.github_id.unwrap_or(self.id)),
            },
            CommentKind::Conversation | CommentKind::ReviewSummary { .. } => {
                ReplyTarget::Conversation
            }
        }
    }

    /// Replies to this comment within `all`, in fetch order. GitHub inline
    /// replies always target the thread's root comment directly (see
    /// [`PrComment::reply_target`]), so there's no multi-level chain to walk —
    /// filtering `in_reply_to == Some(self.id)` finds every reply in the
    /// thread, however it was posted (AMF's own `R`/`n` flow, or some other
    /// actor — e.g. an agent using `gh` directly — that never went through
    /// AMF's reply dialog and so left no local triage record).
    pub fn replies_in<'a>(&self, all: &'a [PrComment]) -> Vec<&'a PrComment> {
        all.iter()
            .filter(|c| c.in_reply_to == Some(self.id))
            .collect()
    }

    /// Whether this comment was posted by an AMF-owned workflow.
    ///
    /// Exact attribution footers are the durable signal: author login is not
    /// sufficient because AMF posts as the user's own GitHub account, and a
    /// loose text search would misclassify ordinary comments that merely
    /// mention AMF.
    pub fn is_amf_authored(&self) -> bool {
        let body = self.body.trim_end();
        body.ends_with(AMF_ATTRIBUTION_FOOTER)
            || body.ends_with(AI_ATTRIBUTION_FOOTER)
            || body.ends_with(AI_REVIEW_ATTRIBUTION_FOOTER)
    }

    /// Whether this is a follow-up reply posted through PR Triage's reply flow.
    ///
    /// The parent relationship is essential: a standalone AMF-authored finding
    /// is still review work even when it carries an attribution footer.
    pub fn is_amf_followup_reply(&self) -> bool {
        self.in_reply_to.is_some() && reply_posted_via_amf(self)
    }

    /// Incoming feedback and standalone findings can be fixed, batched,
    /// replied to, and counted as open work. AMF's own follow-up replies stay
    /// inspectable as thread history but must not become a fresh task.
    pub fn is_actionable(&self) -> bool {
        !self.is_amf_followup_reply()
    }
}

/// Whether `reply` carries either exact AMF attribution footer — the local
/// signal distinguishing a reply AMF posted itself from one some other actor
/// posted directly.
pub fn reply_posted_via_amf(reply: &PrComment) -> bool {
    let body = reply.body.trim_end();
    body.ends_with(AMF_ATTRIBUTION_FOOTER) || body.ends_with(AI_ATTRIBUTION_FOOTER)
}

/// Where a reply is delivered on GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyTarget {
    /// Reply into an inline review thread, appended under its root comment.
    InlineThread { root_comment_id: u64 },
    /// Post a new top-level comment on the PR conversation timeline.
    Conversation,
}

/// Correlates one agent-written reply draft with the exact fix injection that
/// requested it. A fresh UUID is generated every time `f`/`B` opens a confirm
/// dialog; SQLite accepts a returned draft only after that dialog is injected
/// and only while this request remains the comment's latest one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplyDraftRequest {
    pub comment_id: u64,
    pub request_id: String,
    pub base_head_sha: String,
}

impl ReplyDraftRequest {
    pub(super) fn new(comment_id: u64, base_head_sha: &str) -> Self {
        Self {
            comment_id,
            request_id: uuid::Uuid::new_v4().to_string(),
            base_head_sha: base_head_sha.to_string(),
        }
    }
}

/// The contextual replies the pane posts — each tied to a triage decision
/// rather than free-form. A reply is never arbitrary: it reports a fix,
/// explains why one isn't needed, or relays a read-only investigation's
/// findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyKind {
    /// "Done in `<sha>`." after a completed fix → marks the comment `Done`.
    Done,
    /// "Not needed because…" when declining a fix → marks the comment `Skipped`
    /// and keeps the explanation as its local note.
    NotNeeded,
    /// A reply carrying a read-only investigation's findings, from the `a`
    /// action menu. Informational: posting marks the comment `Replied` (not
    /// done/skipped) since the operator may still act on it.
    Investigation,
}

impl ReplyKind {
    /// The kinds the reply-kind picker (`R`) lists. `Investigation` is absent —
    /// it is only reachable from the investigation action menu.
    pub const ALL: [ReplyKind; 2] = [ReplyKind::Done, ReplyKind::NotNeeded];

    /// Short label for the reply dialog title.
    pub fn title(self) -> &'static str {
        match self {
            ReplyKind::Done => "Reply · mark done",
            ReplyKind::NotNeeded => "Reply · not needed",
            ReplyKind::Investigation => "Reply · investigation findings",
        }
    }

    /// Row label for the reply-kind picker.
    pub fn menu_label(self) -> &'static str {
        match self {
            ReplyKind::Done => "Done — report a completed fix",
            ReplyKind::NotNeeded => "Not needed — explain why",
            ReplyKind::Investigation => "Investigation findings",
        }
    }
}

/// Which comment-state action the `m` "Mark" picker offers. `Done` and
/// `Skip` are local-only triage bookkeeping (no GitHub write, no agent
/// tokens); `ResolveOnGitHub` is the one row that actually writes to
/// GitHub (the review thread's resolved state) — kept clearly labeled as
/// such so it isn't mistaken for another local toggle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkAction {
    /// Toggle local `Done` triage.
    Done,
    /// Toggle local `Skipped` triage.
    Skip,
    /// Toggle the GitHub review thread's resolved state.
    ResolveOnGitHub,
}

impl MarkAction {
    /// The three actions, in the order the `m` picker lists them.
    pub const ALL: [MarkAction; 3] = [
        MarkAction::Done,
        MarkAction::Skip,
        MarkAction::ResolveOnGitHub,
    ];

    /// Row label for the picker, reflecting the selected comment's current
    /// state so the toggle direction is visible before pressing `⏎`.
    pub fn menu_label(self, comment: Option<&PrComment>) -> String {
        match self {
            MarkAction::Done => match comment.map(|c| c.triage) {
                Some(TriageState::Done) => "Done (local) — press to clear".to_string(),
                _ => "Done (local)".to_string(),
            },
            MarkAction::Skip => match comment.map(|c| c.triage) {
                Some(TriageState::Skipped) => "Skip (local) — press to clear".to_string(),
                _ => "Skip (local)".to_string(),
            },
            MarkAction::ResolveOnGitHub => match comment.map(|c| c.is_resolved) {
                Some(true) => "Reopen thread on GitHub (currently resolved)".to_string(),
                _ => "Resolve thread on GitHub".to_string(),
            },
        }
    }
}

/// Rough token estimate for a prompt preview (~4 chars/token, the usual
/// English-text heuristic). Approximate by design — it backs the "~N tokens"
/// hint in the fix-confirmation dialog, not a billing figure.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

/// Assemble **one** combined prompt that addresses every comment in `comments`
/// — the "fix all of these, then I'll come back" batch. A single shared
/// preamble is followed by a numbered entry per comment, each carrying the same
/// minimal context as [`PrComment::fix_prompt`] (`file:line` pointer,
/// bot-stripped text, diff hunk) and, like it, **no file contents** (token
/// principle #3): the preamble and any repeated file context are paid once
/// across the whole set instead of once per comment. Injected once into the
/// dedicated triage session so the agent works the list autonomously.
pub fn combined_fix_prompt(comments: &[&PrComment]) -> String {
    let mut out = String::from(
        "Address these PR review comments. Work through each one in order; \
         open the referenced files yourself as needed.\n",
    );
    for (i, comment) in comments.iter().enumerate() {
        out.push_str(&format!(
            "\nComment {}:\n{}\n",
            i + 1,
            comment.fix_prompt_body()
        ));
    }
    out.trim_end().to_string()
}

/// A fully normalized PR review: the resolved PR plus every triageable comment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrReview {
    pub pr: PrRef,
    pub comments: Vec<PrComment>,
    pub fetched_at: DateTime<Local>,
}

impl PrReview {
    /// Whether an AMF follow-up reply is already represented beneath its
    /// fetched root comment's detail view. Such replies stay in the normalized
    /// model/cache, but do not need a duplicate row in the actionable list.
    ///
    /// An orphaned reply whose root was not fetched remains a list row so it is
    /// still accessible rather than silently disappearing.
    pub fn is_collated_amf_reply(&self, comment: &PrComment) -> bool {
        comment.is_amf_followup_reply()
            && comment.in_reply_to.is_some_and(|root_id| {
                self.comments
                    .iter()
                    .any(|candidate| candidate.id == root_id)
            })
    }

    /// Number of unresolved incoming comments and standalone findings. AMF
    /// follow-up replies are thread history, so they never inflate this count.
    pub fn open_count(&self) -> usize {
        self.comments
            .iter()
            .filter(|c| c.is_actionable() && !c.is_resolved)
            .count()
    }
}
