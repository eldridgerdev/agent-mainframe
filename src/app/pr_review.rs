//! PR comment-review model and normalization (feature-specific).
//!
//! The generic `gh` access lives in [`crate::github`]; this module turns those
//! raw GitHub payloads into a single triage-ready [`PrReview`] and owns the
//! token-saving transforms (bot-boilerplate stripping, one-line snippets,
//! thread-resolution merge). See `docs/backlog/pr-comment-review-plan.md`.

// Some helpers (token estimate for the confirm dialog, the loading-state probe)
// are consumed by later epics; keep them until those land.
#![allow(dead_code)]

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::Result;
use chrono::{DateTime, Local};
use regex::Regex;
use serde::{Deserialize, Serialize};

use super::*;
use crate::editor::TextEditor;
use crate::github::{
    GhCli, IssueComment, PrListEntry, PrRef, PrResolution, Review, ReviewComment, ReviewThread,
};
use crate::headless::HeadlessRunner;

/// Snippet length (chars) shown in the comment list.
const SNIPPET_LEN: usize = 80;

/// Label (and de-facto identity) of the dedicated PR-triage agent session. The
/// session is found-or-created by this label so the same window is reused for
/// every fix in a PR (plan token principle #4 — pay per-session overhead once).
pub(crate) const TRIAGE_SESSION_LABEL: &str = "PR Triage";

/// Label used before the feature was renamed to PR Triage. Keep recognizing it
/// so an upgrade reuses an already-running dedicated session instead of quietly
/// creating a second one.
const LEGACY_REVIEW_SESSION_LABEL: &str = "PR Review";

/// Soft ceilings for the combined-batch prompt (`B`). Past either, the confirm
/// dialog still opens but a warning toast fires so the user knows a single
/// prompt this large risks blowing the agent's context window (plan: "keep the
/// set bounded"). They gate a warning, not the action.
const BATCH_COMBINED_COMMENT_WARN: usize = 15;
const BATCH_COMBINED_TOKEN_WARN: usize = 6000;

/// Categories offered in the "add to memory" dialog (`Tab` cycles), matching
/// the examples in the review-memory doc's own header template. `General` is
/// the default and also what a blank category falls back to in
/// `review_memory::append_finding`.
pub(crate) const MEMORY_CATEGORIES: &[&str] = &[
    "General",
    "Concurrency",
    "Error handling",
    "Naming",
    "Tests",
    "Performance",
    "API design",
    "Style",
];

/// Practical ceiling for the "All" lookback depth. Not truly unbounded — a
/// repo's full closed-PR history could be thousands deep, and both the `gh`
/// fetch loop and the one-shot distill pass scale with it.
const BOOTSTRAP_ALL_LIMIT: u32 = 500;

/// How far back the review-memory lookback bootstrap (Epic E) looks when
/// seeding `review-memory.md` from history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapDepth {
    Twenty,
    Fifty,
    Hundred,
    All,
}

impl BootstrapDepth {
    pub const ALL: [BootstrapDepth; 4] = [Self::Twenty, Self::Fifty, Self::Hundred, Self::All];

    pub fn label(self) -> &'static str {
        match self {
            Self::Twenty => "20 PRs",
            Self::Fifty => "50 PRs",
            Self::Hundred => "100 PRs",
            Self::All => "All",
        }
    }

    /// The `gh pr list --limit` value this depth fetches.
    pub fn limit(self) -> u32 {
        match self {
            Self::Twenty => 20,
            Self::Fifty => 50,
            Self::Hundred => 100,
            Self::All => BOOTSTRAP_ALL_LIMIT,
        }
    }
}

impl Default for BootstrapDepth {
    /// Matches the plan's mockup, which highlights 50 PRs by default.
    fn default() -> Self {
        Self::Fifty
    }
}

/// Progress of the background lookback-bootstrap fetch + distill (`b` in the
/// PR picker). Two stages: the `gh` fetch loop (zero agent tokens) and the one
/// headless agent pass that clusters the gathered comments into findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapStage {
    FetchingComments,
    Distilling {
        pr_count: usize,
        token_estimate: usize,
    },
}

/// Stage of the review-memory compact pass's full-screen running view
/// (Epic E "prevent review-memory rot"). Mirrors [`BootstrapStage`]: a cheap
/// prep stage (reading the doc off disk), then the one paid pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactStage {
    ReadingDoc,
    Compacting { token_estimate: usize },
}

/// Outcome of a completed compact run: the doc was read, rewritten by one
/// headless agent pass, and the proposed replacement is awaiting the user's
/// review before anything is written to disk.
#[derive(Debug, Clone)]
pub struct CompactOutcome {
    /// Bullet count in the doc as it stood before compacting.
    pub original_findings: usize,
    /// Bullet count in the agent's proposed replacement.
    pub proposed_findings: usize,
    /// The full proposed replacement document text.
    pub proposed_content: String,
}

/// Messages sent back from the background compact thread. `Compacting` fires
/// once, right before the one headless agent call, so the running screen can
/// show a token estimate; `Done` fires exactly once at the end. `Ok(None)`
/// means there was nothing to compact (doc missing or has zero findings) —
/// distinct from an error, since it isn't one.
pub enum CompactProgress {
    Compacting { token_estimate: usize },
    Done(Result<Option<CompactOutcome>>),
}

/// Outcome of a completed bootstrap run.
#[derive(Debug, Clone, Copy)]
pub struct BootstrapOutcome {
    /// PRs whose comments/reviews contributed non-empty text to the prompt.
    pub pr_count: usize,
    /// Findings newly appended to the memory doc (dedup-aware — re-running
    /// the bootstrap over overlapping history won't double them up).
    pub appended: usize,
}

/// Messages sent back from the background bootstrap thread. `Distilling` fires
/// once, right before the one headless agent call, so the running screen can
/// show a token estimate for that call; `Done` fires exactly once at the end.
pub enum BootstrapProgress {
    Distilling {
        pr_count: usize,
        token_estimate: usize,
    },
    Done(Result<BootstrapOutcome>),
}

/// Parse a GitHub-provided hunk and retain only the lines immediately around
/// its comment anchor. The synthetic file headers let the regular unified-diff
/// parser do the fiddly line-kind/header work without maintaining a second
/// parser here.
fn window_github_hunk(text: &str, line: usize, old_side: bool, context: usize) -> Option<String> {
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
const AMF_ATTRIBUTION_FOOTER: &str = "— posted via AMF";

fn append_amf_attribution(body: &str) -> String {
    format!("{}\n\n{}", body.trim_end(), AMF_ATTRIBUTION_FOOTER)
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
}

impl FixTarget {
    /// Short human label for footers / toasts.
    pub fn label(self) -> &'static str {
        match self {
            FixTarget::DedicatedReview => "dedicated triage session",
            FixTarget::ExistingLive => "existing live session",
        }
    }

    /// Compact footer tag.
    pub fn tag(self) -> &'static str {
        match self {
            FixTarget::DedicatedReview => "dedicated",
            FixTarget::ExistingLive => "live",
        }
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
        FixTarget::ExistingLive => feature
            .sessions
            .iter()
            .position(|s| s.kind.is_agent_harness()),
        FixTarget::DedicatedReview => feature
            .sessions
            .iter()
            .position(|s| s.kind.is_agent_harness() && s.label == dedicated_label),
    }
}

/// Resolve the dedicated PR-triage session, preferring the current label while
/// retaining compatibility with sessions created under the old "PR Review"
/// label. Existing-live targeting is unchanged.
pub(crate) fn pr_triage_session_index(feature: &Feature, target: FixTarget) -> Option<usize> {
    fix_session_index(feature, target, TRIAGE_SESSION_LABEL).or_else(|| {
        (target == FixTarget::DedicatedReview)
            .then(|| fix_session_index(feature, target, LEGACY_REVIEW_SESSION_LABEL))
            .flatten()
    })
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
const WHOLE_FILE_HUNK_LINES: usize = 150;

/// Context retained on either side of a line-anchored review comment. GitHub's
/// `diff_hunk` can encompass an entire newly-added function even when the
/// comment itself points at one line; rendering or injecting all of it makes
/// the referenced code hard to spot and wastes prompt context.
const COMMENT_HUNK_CONTEXT_LINES: usize = 3;

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
    fn fix_prompt_body(&self) -> String {
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
}

/// Whether `reply` carries the "posted via AMF" channel-disclosure footer
/// ([`append_amf_attribution`]) — the only local signal distinguishing a
/// reply AMF posted itself from one some other actor (a headless agent
/// shelling out to `gh`, a human on GitHub) posted directly, since a reply
/// posted outside AMF's `R`/`n` dialog leaves no local triage record at all.
pub fn reply_posted_via_amf(reply: &PrComment) -> bool {
    reply.body.trim_end().ends_with(AMF_ATTRIBUTION_FOOTER)
}

/// Where a reply is delivered on GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyTarget {
    /// Reply into an inline review thread, appended under its root comment.
    InlineThread { root_comment_id: u64 },
    /// Post a new top-level comment on the PR conversation timeline.
    Conversation,
}

/// The two contextual replies the pane posts — both tied to a triage decision
/// rather than free-form. A reply is never arbitrary: it either reports a fix
/// or explains why one isn't needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyKind {
    /// "Done in `<sha>`." after a completed fix → marks the comment `Done`.
    Done,
    /// "Not needed because…" when declining a fix → marks the comment `Skipped`
    /// and keeps the explanation as its local note.
    NotNeeded,
}

impl ReplyKind {
    /// The two kinds, in the order the reply-kind picker (`R`) lists them.
    pub const ALL: [ReplyKind; 2] = [ReplyKind::Done, ReplyKind::NotNeeded];

    /// Short label for the reply dialog title.
    pub fn title(self) -> &'static str {
        match self {
            ReplyKind::Done => "Reply · mark done",
            ReplyKind::NotNeeded => "Reply · not needed",
        }
    }

    /// Row label for the reply-kind picker.
    pub fn menu_label(self) -> &'static str {
        match self {
            ReplyKind::Done => "Done — report a completed fix",
            ReplyKind::NotNeeded => "Not needed — explain why",
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

/// Short HEAD commit hash of `workdir`, used as the last-resort seed for a
/// "Done in `<sha>`." reply. `None` when the directory isn't a git repo or has
/// no commits yet.
fn latest_commit_short_sha(workdir: &Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Short hash of the most recent commit that touched `path` at `line`, via
/// `git log -L` (line-history search). `None` when the line has no history
/// (e.g. it predates the repo, or the lookup fails for any reason — an
/// outdated/shifted line number, a rename `git log` didn't follow, etc.); the
/// caller falls back to a file-level or bare-HEAD search.
fn commit_touching_line(workdir: &Path, path: &str, line: u32) -> Option<String> {
    let output = std::process::Command::new("git")
        .args([
            "log",
            "-L",
            &format!("{line},{line}:{path}"),
            "-1",
            "--format=%h",
            "--no-patch",
        ])
        .current_dir(workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Short hash of the most recent commit that touched `path` at all — the
/// file-level fallback when a line-anchored search isn't applicable (a
/// file-level comment) or comes up empty.
fn commit_touching_file(workdir: &Path, path: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["log", "-1", "--format=%h", "--", path])
        .current_dir(workdir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!sha.is_empty()).then_some(sha)
}

/// Best-effort commit for a "Done in `<sha>`" reply: search history for a
/// commit that plausibly addressed `comment` before falling back to bare
/// `HEAD`. Returns the sha alongside whether it's a confident match (the
/// caller adds a "(latest commit)" caveat when it isn't).
///
/// Order: line history (skipped for an outdated anchor, since the line number
/// no longer corresponds to the comment's original line) → file history →
/// bare HEAD.
fn commit_for_done_reply(workdir: &Path, comment: &PrComment) -> (Option<String>, bool) {
    if let Some(path) = &comment.path {
        if !comment.outdated
            && let Some(line) = comment.line
            && let Some(sha) = commit_touching_line(workdir, path, line)
        {
            return (Some(sha), true);
        }
        if let Some(sha) = commit_touching_file(workdir, path) {
            return (Some(sha), true);
        }
    }
    (latest_commit_short_sha(workdir), false)
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

/// Build a fresh fix-confirm dialog seeded with `prompt`. The editor opens with
/// the vim keymap when `vim` is set (the pane-level remembered preference) so
/// reopening the dialog for another comment keeps the user's chosen keymap.
/// Build a fresh fix-confirm dialog. `batch` is `None` for an ordinary
/// single-comment fix and `Some(ids)` for the combined-batch flow (`B`), where
/// injecting marks every listed comment `Fixing`.
fn new_fix_confirm(prompt: String, vim: bool, batch: Option<Vec<u64>>) -> FixConfirmState {
    FixConfirmState {
        editor: if vim {
            TextEditor::with_vim(prompt)
        } else {
            TextEditor::new(prompt)
        },
        editing: false,
        scroll: 0,
        // Seed the view scrolled to the cursor (end of the prompt for plain,
        // start for vim) so a tall prompt opens somewhere sensible.
        sync_to_cursor: true,
        batch,
    }
}

/// A fully normalized PR review: the resolved PR plus every triageable comment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrReview {
    pub pr: PrRef,
    pub comments: Vec<PrComment>,
    pub fetched_at: DateTime<Local>,
}

impl PrReview {
    /// Number of comments not yet resolved on GitHub.
    pub fn open_count(&self) -> usize {
        self.comments.iter().filter(|c| !c.is_resolved).count()
    }
}

/// Fetch every comment source for a resolved PR and normalize them into one
/// [`PrReview`]. This is the single entry point the UI/state layer calls to
/// (re)load a review; it runs entirely in Rust and spends zero agent tokens.
///
/// Run this off the UI thread — it makes four `gh` calls.
pub fn fetch_and_normalize(workdir: &Path, pr: PrRef) -> Result<PrReview> {
    let review_comments = GhCli::pr_review_comments(workdir, pr.number)?;
    let reviews = GhCli::pr_reviews(workdir, pr.number)?;
    let issue_comments = GhCli::issue_comments(workdir, pr.number)?;
    let threads = GhCli::review_threads(workdir, &pr.owner, &pr.repo, pr.number)?;
    Ok(normalize(
        pr,
        review_comments,
        reviews,
        issue_comments,
        threads,
    ))
}

/// Merge the raw `gh` payloads into a single triage-ready [`PrReview`].
///
/// Inline comments come first (they're the core use case), then non-empty
/// review summaries, then conversation comments. Resolution state is attached
/// from the GraphQL thread map; empty-body review summaries (bare approvals)
/// are dropped since there's nothing to triage.
pub fn normalize(
    pr: PrRef,
    review_comments: Vec<ReviewComment>,
    reviews: Vec<Review>,
    issue_comments: Vec<IssueComment>,
    threads: Vec<ReviewThread>,
) -> PrReview {
    let thread_index = index_threads(&threads);
    let mut comments = Vec::new();

    for c in review_comments {
        let is_bot = c.user.is_bot();
        let (thread_id, is_resolved) = match thread_index.get(&c.id) {
            Some((id, resolved)) => (Some(id.clone()), *resolved),
            None => (None, false),
        };
        let snippet = make_snippet(&c.body, is_bot);
        // A file-level comment has no line by definition — that's not the same
        // thing as an outdated line comment, so don't badge it as one.
        let file_level = c.subject_type.as_deref() == Some("file");
        comments.push(PrComment {
            id: c.id,
            kind: CommentKind::Inline,
            author: c.user.login,
            is_bot,
            path: c.path,
            line: c.line.or(c.original_line),
            side: c.side,
            outdated: c.line.is_none() && !file_level,
            file_level,
            diff_hunk: c.diff_hunk,
            body: c.body,
            snippet,
            in_reply_to: c.in_reply_to_id,
            thread_id,
            is_resolved,
            triage: TriageState::default(),
            local_note: None,
            github_id: None,
            github_review_id: c.pull_request_review_id,
        });
    }

    for r in reviews {
        // A review with no body is just an approve/comment action — nothing to
        // triage, so skip it.
        if r.body.trim().is_empty() {
            continue;
        }
        let is_bot = r.user.is_bot();
        let snippet = make_snippet(&r.body, is_bot);
        comments.push(PrComment {
            id: r.id,
            kind: CommentKind::ReviewSummary { state: r.state },
            author: r.user.login,
            is_bot,
            path: None,
            line: None,
            side: None,
            outdated: false,
            file_level: false,
            diff_hunk: None,
            body: r.body,
            snippet,
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::default(),
            local_note: None,
            github_id: None,
            github_review_id: Some(r.id),
        });
    }

    for c in issue_comments {
        let is_bot = c.user.is_bot();
        let snippet = make_snippet(&c.body, is_bot);
        comments.push(PrComment {
            id: c.id,
            kind: CommentKind::Conversation,
            author: c.user.login,
            is_bot,
            path: None,
            line: None,
            side: None,
            outdated: false,
            file_level: false,
            diff_hunk: None,
            body: c.body,
            snippet,
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::default(),
            local_note: None,
            github_id: None,
            github_review_id: None,
        });
    }

    PrReview {
        pr,
        comments,
        fetched_at: Local::now(),
    }
}

/// Build `comment_id -> (thread_node_id, is_resolved)` from the GraphQL threads.
fn index_threads(threads: &[ReviewThread]) -> HashMap<u64, (String, bool)> {
    let mut map = HashMap::new();
    for t in threads {
        for &cid in &t.comment_ids {
            map.insert(cid, (t.id.clone(), t.is_resolved));
        }
    }
    map
}

/// Produce a one-line list snippet, stripping bot boilerplate first so the
/// snippet reflects the actual content, not scaffolding.
fn make_snippet(body: &str, is_bot: bool) -> String {
    let cleaned = if is_bot {
        strip_bot_boilerplate(body)
    } else {
        body.to_string()
    };
    let first = cleaned
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    truncate_chars(first, SNIPPET_LEN)
}

/// Strip the heavy scaffolding bots (CodeRabbit, Copilot, …) wrap around their
/// actual point: `<details>` blocks, HTML comments, `<summary>` tags, markdown
/// image badges, fenced quoted-diff/suggestion blocks, and leading `> `
/// quoted-diff lines. Cheap and lossy-by-design — only the actionable prose
/// needs to survive for the agent prompt and snippet. The comment's own
/// `diff_hunk` (plus the checked-out repo) already gives the agent this
/// context, so a bot re-quoting the diff inline is pure repetition.
pub fn strip_bot_boilerplate(body: &str) -> String {
    static DETAILS: OnceLock<Regex> = OnceLock::new();
    static HTML_COMMENT: OnceLock<Regex> = OnceLock::new();
    static SUMMARY: OnceLock<Regex> = OnceLock::new();
    static IMAGE: OnceLock<Regex> = OnceLock::new();
    static QUOTED_DIFF_FENCE: OnceLock<Regex> = OnceLock::new();
    static QUOTED_LINES: OnceLock<Regex> = OnceLock::new();
    static BLANKS: OnceLock<Regex> = OnceLock::new();

    let details = DETAILS.get_or_init(|| Regex::new(r"(?is)<details>.*?</details>").unwrap());
    let html_comment = HTML_COMMENT.get_or_init(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
    let summary = SUMMARY.get_or_init(|| Regex::new(r"(?is)</?summary>").unwrap());
    let image = IMAGE.get_or_init(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
    // Fenced ```diff / ```suggestion blocks: bots paste the same hunk back as
    // a code fence, which repeats context the agent already gets for free
    // from `diff_hunk`.
    let quoted_diff_fence = QUOTED_DIFF_FENCE
        .get_or_init(|| Regex::new(r"(?ims)^```(?:diff|suggestion)\s*\n.*?\n```\s*$").unwrap());
    // Leading `> ` blockquote lines (bots sometimes quote the diff as a
    // blockquote instead of a fence).
    let quoted_lines = QUOTED_LINES.get_or_init(|| Regex::new(r"(?m)^>.*$\n?").unwrap());
    let blanks = BLANKS.get_or_init(|| Regex::new(r"\n{3,}").unwrap());

    let s = details.replace_all(body, "");
    let s = html_comment.replace_all(&s, "");
    let s = summary.replace_all(&s, "");
    let s = image.replace_all(&s, "");
    let s = quoted_diff_fence.replace_all(&s, "");
    let s = quoted_lines.replace_all(&s, "");
    let s = blanks.replace_all(&s, "\n\n");
    s.trim().to_string()
}

/// Truncate to `max` characters (not bytes), appending an ellipsis when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// Flatten one PR's review comments + review summaries into plain-text lines
/// for the lookback-bootstrap prompt (Epic E). Bot bodies are stripped like
/// everywhere else in this module; empty bodies (bare approvals, blank
/// comments) are dropped. Returns an empty string when the PR has nothing
/// worth feeding to the distiller.
fn bootstrap_pr_text(comments: &[ReviewComment], reviews: &[Review]) -> String {
    let mut lines = Vec::new();
    for c in comments {
        let text = if c.user.is_bot() {
            strip_bot_boilerplate(&c.body)
        } else {
            c.body.clone()
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let loc = match &c.path {
            Some(p) => match c.line.or(c.original_line) {
                Some(l) => format!("{p}:{l}"),
                None => p.clone(),
            },
            None => "general".to_string(),
        };
        lines.push(format!("- ({loc}) {}", text.replace('\n', " ")));
    }
    for r in reviews {
        let text = if r.user.is_bot() {
            strip_bot_boilerplate(&r.body)
        } else {
            r.body.clone()
        };
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        lines.push(format!("- (review) {}", text.replace('\n', " ")));
    }
    lines.join("\n")
}

/// Assemble the one distill prompt from every PR's gathered text. Instructs
/// the agent to output the same `## Category` / `- bullet` shape
/// [`review_memory::append_finding`] writes, so the response can be fed
/// straight back through [`review_memory::parse_findings_markdown`] with no
/// further parsing.
fn bootstrap_prompt(pr_bodies: &[(u32, String, String)]) -> String {
    let mut out = String::from(
        "You are distilling recurring code-review findings from a project's PR \
         history into a durable list of lessons for future reviews.\n\n\
         Below are review comments and review summaries from several recent, \
         already-merged/closed pull requests. Identify findings that recur across \
         multiple PRs, or that state a general rule the team clearly cares about — \
         not a one-off nitpick specific to a single PR's code. Ignore praise, \
         procedural comments (\"LGTM\", \"done\"), and anything that reads as already \
         resolved.\n\n\
         Output ONLY a Markdown list grouped under `## Category` headings (categories \
         like General, Concurrency, Error handling, Naming, Tests, Performance, API \
         design, Style), one finding per `- ` bullet, phrased as a general rule (not \
         tied to a specific file, PR, or person). No prose outside the headings and \
         bullets.\n\n---\n\n",
    );
    for (number, title, body) in pr_bodies {
        out.push_str(&format!("### PR #{number}: {title}\n{body}\n\n"));
    }
    out.trim_end().to_string()
}

/// Background body of the lookback bootstrap (Epic E): fetch comments/reviews
/// for every listed PR (zero agent tokens), then make **one** headless agent
/// pass to cluster them into findings and append the new ones to the memory
/// doc. Runs off the UI thread; progress and the final result are reported
/// over `tx`. A single PR's fetch failure is skipped rather than aborting the
/// whole run — one stale/deleted PR shouldn't sink the batch.
fn run_review_memory_bootstrap(
    workdir: PathBuf,
    memory_path: PathBuf,
    entries: Vec<PrListEntry>,
    model: Option<String>,
    tx: std::sync::mpsc::Sender<BootstrapProgress>,
) {
    let mut pr_bodies = Vec::new();
    for entry in &entries {
        let comments = GhCli::pr_review_comments(&workdir, entry.number).unwrap_or_default();
        let reviews = GhCli::pr_reviews(&workdir, entry.number).unwrap_or_default();
        let text = bootstrap_pr_text(&comments, &reviews);
        if !text.is_empty() {
            pr_bodies.push((entry.number, entry.title.clone(), text));
        }
    }

    if pr_bodies.is_empty() {
        let _ = tx.send(BootstrapProgress::Done(Ok(BootstrapOutcome {
            pr_count: 0,
            appended: 0,
        })));
        return;
    }

    let prompt = bootstrap_prompt(&pr_bodies);
    let _ = tx.send(BootstrapProgress::Distilling {
        pr_count: pr_bodies.len(),
        token_estimate: estimate_tokens(&prompt),
    });

    let result = HeadlessRunner::run(&AgentKind::Claude, &workdir, &prompt, model.as_deref())
        .and_then(|output| {
            let findings = review_memory::parse_findings_markdown(&output);
            let mut appended = 0;
            for (category, finding) in &findings {
                if review_memory::append_finding(&memory_path, category, finding)? {
                    appended += 1;
                }
            }
            Ok(BootstrapOutcome {
                pr_count: pr_bodies.len(),
                appended,
            })
        });
    let _ = tx.send(BootstrapProgress::Done(result));
}

/// Background body of the review-memory compact pass ("prevent review-memory
/// rot"): read the doc, make **one** headless agent pass to merge
/// near-duplicate findings and prune stale ones, and report the proposed
/// replacement for the user to review — nothing is written here. Runs off the
/// UI thread; progress and the final result are reported over `tx`.
fn run_review_memory_compact(
    workdir: PathBuf,
    memory_path: PathBuf,
    model: Option<String>,
    tx: std::sync::mpsc::Sender<CompactProgress>,
) {
    let contents = match std::fs::read_to_string(&memory_path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let _ = tx.send(CompactProgress::Done(Ok(None)));
            return;
        }
        Err(e) => {
            let _ = tx.send(CompactProgress::Done(Err(e.into())));
            return;
        }
    };

    let original_findings = review_memory::count_findings(&contents);
    if original_findings == 0 {
        let _ = tx.send(CompactProgress::Done(Ok(None)));
        return;
    }

    let prompt = review_memory::compact_prompt(&contents);
    let _ = tx.send(CompactProgress::Compacting {
        token_estimate: estimate_tokens(&prompt),
    });

    let result = HeadlessRunner::run(&AgentKind::Claude, &workdir, &prompt, model.as_deref()).map(
        |output| {
            let proposed_content = output.trim().to_string();
            let proposed_findings = review_memory::count_findings(&proposed_content);
            Some(CompactOutcome {
                original_findings,
                proposed_findings,
                proposed_content,
            })
        },
    );
    let _ = tx.send(CompactProgress::Done(result));
}

impl App {
    /// Open PR Triage for the selected feature's branch.
    ///
    /// Runs the `gh` preconditions and resolves the PR synchronously (cheap),
    /// then kicks the comment fetch onto a background thread. All of this
    /// spends zero agent tokens.
    pub fn open_pr_review(&mut self) {
        let Some((_project, feature)) = self.selected_feature() else {
            self.message = Some("Select a feature to review its PR".to_string());
            return;
        };
        let workdir = feature.workdir.clone();
        self.open_pr_review_for_workdir(workdir);
    }

    /// Open PR Triage for the feature behind the current `Viewing` session —
    /// the leader-key entry point (`leader+G`), peer to the dashboard's `G`.
    /// Lets the user jump straight into triage without first exiting to the
    /// dashboard and re-entering.
    pub fn open_pr_review_from_view(&mut self) {
        let AppMode::Viewing(view) = &self.mode else {
            return;
        };
        let Some(workdir) = self.feature_for_view(view).map(|f| f.workdir.clone()) else {
            self.message = Some("No active feature to review".to_string());
            return;
        };
        self.open_pr_review_for_workdir(workdir);
    }

    fn open_pr_review_for_workdir(&mut self, workdir: PathBuf) {
        if let Err(e) = GhCli::check_available() {
            self.show_error(e);
            return;
        }
        if let Err(e) = GhCli::check_auth() {
            self.show_error(e);
            return;
        }

        match GhCli::resolve_pr(&workdir) {
            Ok(PrResolution::Found(pr)) => self.enter_pr_review(workdir, pr),
            // No PR for this branch: offer a list of the repo's PRs to pick from
            // (the picker falls through to the number prompt on its own if the
            // list can't be fetched).
            Ok(PrResolution::NoPrForBranch) => self.open_pr_picker(workdir, None),
            Err(e) => self.show_error(e),
        }
    }

    /// Open the PR Triage pane for a resolved PR, preferring the SQLite cache.
    ///
    /// A cache hit (same `PR# + head SHA`) skips the four `gh` calls entirely and
    /// shows the stored comments instantly; a miss falls back to the background
    /// fetch. Either path spends zero agent tokens. Manual refresh
    /// ([`refresh_pr_review`](Self::refresh_pr_review)) bypasses the cache.
    fn enter_pr_review(&mut self, workdir: PathBuf, pr: PrRef) {
        if let Some(mut review) = self.load_cached_pr_review(&pr) {
            self.log_info(
                "pr_review",
                format!("cache hit for PR #{} @ {}", pr.number, pr.head_sha),
            );
            self.apply_persisted_triage(&mut review);
            let usage_baselines = self.pr_review_initial_usage_baselines(&workdir);
            let checked_out_branch =
                crate::worktree::WorktreeManager::current_branch(&workdir).unwrap_or(None);
            self.mode = AppMode::PrReview(PrReviewState {
                workdir,
                review,
                selected: 0,
                detail_scroll: 0,
                detail_content_lines: 0,
                hide_resolved: false,
                sort_mode: PrSortMode::default(),
                fix_target: FixTarget::default(),
                fix_target_picked: false,
                usage_baselines,
                review_harness: None,
                harness_pick: None,
                fix_confirm: None,
                fix_vim_enabled: false,
                mark_pick: None,
                reply_kind_pick: None,
                reply: None,
                memory_add: None,
                marked: std::collections::HashSet::new(),
                pending_batch: false,
                checked_out_branch,
            });
            return;
        }
        self.start_pr_review_fetch(workdir, pr);
    }

    /// Drop in-memory state that belongs to a known predecessor PR on the same
    /// feature.
    /// SQLite cache and triage rows are intentionally untouched: they remain
    /// keyed by PR number and are still available when the user explicitly
    /// chooses a closed PR from the picker.
    ///
    /// This is called when dashboard badge sync observes a PR-number transition.
    /// Naming the predecessor explicitly keeps an older PR chosen from the picker
    /// from invalidating live work that belongs to the current successor. It
    /// prevents `leader+P`, a late AI review result, or an old comment-fetch
    /// result from silently restoring the closed predecessor after the branch
    /// has been reused.
    pub(crate) fn invalidate_pr_context_for_transition(
        &mut self,
        workdir: &Path,
        predecessor_pr_number: u32,
    ) -> bool {
        let mut changed = false;

        if self.pr_review_return.as_ref().is_some_and(|stash| {
            stash.state.workdir == workdir && stash.state.review.pr.number == predecessor_pr_number
        }) {
            self.pr_review_return = None;
            changed = true;
        }

        if self.ai_review_pending.as_ref().is_some_and(|pending| {
            pending.workdir == workdir && pending.pr.number == predecessor_pr_number
        }) {
            self.ai_review_pending = None;
            self.ai_review_bg = None;
            self.ai_review_progress = None;
            changed = true;
        }

        let stale_loading = matches!(
            &self.mode,
            AppMode::PrReviewLoading(state)
                if state.workdir == workdir && state.pr.number == predecessor_pr_number
        );
        let stale_ai_run = matches!(
            &self.mode,
            AppMode::AiReviewRunning(state)
                if state.origin.workdir == workdir
                    && state.origin.pr.number == predecessor_pr_number
        );
        if stale_loading {
            self.pr_review_bg = None;
            self.mode = AppMode::Normal;
            changed = true;
        } else if stale_ai_run {
            self.ai_review_bg = None;
            self.ai_review_pending = None;
            self.ai_review_progress = None;
            self.mode = AppMode::Normal;
            changed = true;
        }

        changed
    }

    /// Look up a cached, normalized review for this PR's head SHA. Returns `None`
    /// on a miss, when there's no DB, or when the cache read fails (non-fatal —
    /// the caller just re-fetches).
    fn load_cached_pr_review(&self, pr: &PrRef) -> Option<PrReview> {
        self.db
            .as_ref()?
            .load_pr_review_cache(pr.number, &pr.head_sha)
            .ok()
            .flatten()
    }

    /// Persist a freshly-fetched review under its `PR# + head SHA` key so the
    /// next open is a cache hit. A write failure is non-fatal (logged, not shown).
    fn cache_pr_review(&mut self, review: &PrReview) {
        let result = match self.db.as_ref() {
            Some(db) => db.save_pr_review_cache(review),
            None => return,
        };
        if let Err(e) = result {
            self.log_warn("pr_review", format!("cache write failed: {e}"));
        }
    }

    /// Overlay the persisted local triage (`Fixing`/`Done`/skip notes) onto a
    /// freshly-loaded review. The `pr_comment_triage` table — keyed by
    /// `PR# + comment id` (not the head SHA, so marks survive a push that moves
    /// the PR's head) — is authoritative for local triage, so it wins over
    /// whatever the cache blob happened to serialize. A read failure (or no DB)
    /// is non-fatal: comments just stay [`TriageState::Untriaged`].
    fn apply_persisted_triage(&mut self, review: &mut PrReview) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let triage = match db.load_pr_comment_triage(review.pr.number) {
            Ok(map) => map,
            Err(e) => {
                self.log_warn("pr_review", format!("triage load failed: {e}"));
                return;
            }
        };
        for comment in &mut review.comments {
            if let Some((state, note)) = triage.get(&comment.id) {
                comment.triage = *state;
                comment.local_note = note.clone();
            }
        }
    }

    /// Persist one comment's triage state (with an optional note) to SQLite. A
    /// write failure is non-fatal (logged, not surfaced).
    fn persist_triage(
        &mut self,
        pr_number: u32,
        head_sha: &str,
        comment_id: u64,
        state: TriageState,
        note: Option<&str>,
    ) {
        let result = match self.db.as_ref() {
            Some(db) => db.save_pr_comment_triage(pr_number, head_sha, comment_id, state, note),
            None => return,
        };
        if let Err(e) = result {
            self.log_warn("pr_review", format!("triage persist failed: {e}"));
        }
    }

    /// Set the selected comment's triage state in-memory and persist it. The
    /// comment keeps its existing `local_note`. No-op outside PR Triage or
    /// with no selection.
    fn pr_review_set_triage(&mut self, state: TriageState) {
        let Some((pr_number, head_sha, comment_id, note)) = ({
            let AppMode::PrReview(s) = &mut self.mode else {
                return;
            };
            s.review.comments.get_mut(s.selected).map(|c| {
                c.triage = state;
                (
                    s.review.pr.number,
                    s.review.pr.head_sha.clone(),
                    c.id,
                    c.local_note.clone(),
                )
            })
        }) else {
            return;
        };
        self.persist_triage(pr_number, &head_sha, comment_id, state, note.as_deref());
    }

    /// Open the "Mark" picker (`m`): a three-row choice between local `Done`,
    /// local `Skip`, and toggling the GitHub thread's resolved state.
    /// Replaces the old separate `m`/`s`/`x` top-level keys with one entry
    /// point. No-op (with a hint) if nothing is selected or another dialog
    /// is already open.
    pub fn pr_review_open_mark_pick(&mut self) {
        let ready = match &self.mode {
            AppMode::PrReview(state)
                if state.reply.is_none()
                    && state.fix_confirm.is_none()
                    && state.reply_kind_pick.is_none()
                    && state.mark_pick.is_none() =>
            {
                state.selected_comment().is_some()
            }
            _ => return,
        };
        if !ready {
            self.message = Some("No comment selected".into());
            return;
        }
        if let AppMode::PrReview(state) = &mut self.mode {
            state.mark_pick = Some(MarkPickState { selected: 0 });
        }
    }

    /// Whether the "Mark" picker is currently open over PR Triage.
    pub fn pr_review_mark_pick_picking(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state) if state.mark_pick.is_some()
        )
    }

    /// Move the "Mark"-picker highlight (`+1`/`-1`, wrapping).
    pub fn pr_review_mark_pick_move(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(pick) = &mut state.mark_pick
        {
            let len = MarkAction::ALL.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Confirm the "Mark" picker: close it and apply the chosen action
    /// immediately (reusing the existing done/skip/resolve flows as-is) —
    /// no further confirm step, matching the original single-key behavior.
    pub fn pr_review_mark_pick_confirm(&mut self) {
        let chosen = match &self.mode {
            AppMode::PrReview(state) => state
                .mark_pick
                .as_ref()
                .map(|pick| MarkAction::ALL[pick.selected]),
            _ => return,
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            state.mark_pick = None;
        }
        match chosen {
            Some(MarkAction::Done) => self.pr_review_mark_done(),
            Some(MarkAction::Skip) => self.pr_review_skip(),
            Some(MarkAction::ResolveOnGitHub) => self.pr_review_toggle_resolve(),
            None => {}
        }
    }

    /// Cancel the "Mark" picker without choosing.
    pub fn pr_review_mark_pick_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.mark_pick = None;
        }
    }

    /// Mark the selected comment done (toggles back to untriaged if it already
    /// is). Manual, with **no auto-advance** — the user stays on the comment so
    /// they can review the agent's work before moving on (plan: Epic B).
    /// Reached via the "Mark" picker (`m`), or called directly by
    /// tests/internal flows.
    pub fn pr_review_mark_done(&mut self) {
        let next = match self.pr_review_selected_triage() {
            Some(TriageState::Done) => TriageState::Untriaged,
            Some(_) => TriageState::Done,
            None => return,
        };
        self.pr_review_set_triage(next);
        let msg = match next {
            TriageState::Done => "Marked done",
            _ => "Cleared — back to untriaged",
        };
        self.push_toast_success(msg.to_string());
    }

    /// Skip the selected comment locally (toggles back to untriaged if already
    /// skipped). Local-only — no GitHub write, no agent tokens.
    pub fn pr_review_skip(&mut self) {
        let next = match self.pr_review_selected_triage() {
            Some(TriageState::Skipped) => TriageState::Untriaged,
            Some(_) => TriageState::Skipped,
            None => return,
        };
        self.pr_review_set_triage(next);
        let msg = match next {
            TriageState::Skipped => "Skipped",
            _ => "Cleared — back to untriaged",
        };
        self.push_toast_success(msg.to_string());
    }

    /// Triage state of the currently-selected comment, if any.
    fn pr_review_selected_triage(&self) -> Option<TriageState> {
        match &self.mode {
            AppMode::PrReview(s) => s.selected_comment().map(|c| c.triage),
            _ => None,
        }
    }

    /// Toggle whether the selected comment is marked for a batch fix (`space`).
    /// Marks are kept by comment id, so they survive the hide-resolved filter
    /// shifting the visible rows. No-op with no selection.
    pub fn pr_review_toggle_mark(&mut self) {
        let AppMode::PrReview(state) = &mut self.mode else {
            return;
        };
        let Some(id) = state.selected_comment().map(|c| c.id) else {
            return;
        };
        let now_marked = if state.marked.remove(&id) {
            false
        } else {
            state.marked.insert(id);
            true
        };
        let count = state.marked.len();
        self.message = Some(if now_marked {
            format!("Marked for batch fix ({count} marked)")
        } else if count == 0 {
            "Unmarked — nothing marked".to_string()
        } else {
            format!("Unmarked ({count} still marked)")
        });
    }

    /// Open the PR picker: a selectable list of the repo's PRs. `seed_number`
    /// pre-highlights that PR when present (e.g. the branch's auto-detected one,
    /// or the PR already open in the pane). Lists open PRs by default. If `gh pr
    /// list` fails outright, falls back to the manual number prompt so the user
    /// is never stuck. Zero agent tokens.
    pub fn open_pr_picker(&mut self, workdir: PathBuf, seed_number: Option<u32>) {
        match GhCli::list_prs(&workdir, false) {
            Ok(entries) => {
                let selected = seed_number
                    .and_then(|n| entries.iter().position(|e| e.number == n))
                    .unwrap_or(0);
                let current_user = self.resolve_gh_current_user(&workdir);
                self.mode = AppMode::PrPicker(PrPickerState {
                    workdir,
                    entries,
                    selected,
                    include_closed: false,
                    error: None,
                    bootstrap_pick: None,
                    compact_confirm: None,
                    current_user,
                });
            }
            Err(e) => {
                self.log_warn("pr_review", format!("pr list failed: {e}"));
                self.prompt_pr_number(workdir, Some(e.to_string()));
            }
        }
    }

    /// Resolve the authenticated `gh` user's login, memoized in
    /// [`App::gh_current_user`] for the session so the PR picker doesn't
    /// repeat the `gh api user` call on every open/refresh. A failed
    /// resolution (e.g. `gh` unauthenticated) is cached too, rather than
    /// retried on every call.
    pub(crate) fn resolve_gh_current_user(&mut self, workdir: &Path) -> Option<String> {
        if let Some(cached) = &self.gh_current_user {
            return cached.clone();
        }
        let resolved = match GhCli::current_user(workdir) {
            Ok(login) => Some(login),
            Err(e) => {
                self.log_warn("pr_review", format!("could not resolve gh user: {e}"));
                None
            }
        };
        self.gh_current_user = Some(resolved.clone());
        resolved
    }

    /// Open the PR picker from PR Triage (the `g` key), seeded on the
    /// PR currently being reviewed so it starts highlighted.
    pub fn open_pr_picker_from_pane(&mut self) {
        let (workdir, current) = match &self.mode {
            AppMode::PrReview(state) => (state.workdir.clone(), Some(state.review.pr.number)),
            AppMode::PrReviewLoading(state) => (state.workdir.clone(), Some(state.pr.number)),
            _ => return,
        };
        self.open_pr_picker(workdir, current);
    }

    /// Move the picker highlight down one row (clamped).
    pub fn pr_picker_select_next(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode
            && !state.entries.is_empty()
        {
            state.selected = (state.selected + 1).min(state.entries.len() - 1);
        }
    }

    /// Move the picker highlight up one row (clamped).
    pub fn pr_picker_select_prev(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.selected = state.selected.saturating_sub(1);
        }
    }

    /// Toggle whether the picker list includes closed/merged PRs, re-fetching
    /// with the new filter. Keeps the highlight on the same PR number when it
    /// survives the toggle.
    pub fn pr_picker_toggle_closed(&mut self) {
        let (workdir, include_closed, current) = match &self.mode {
            AppMode::PrPicker(state) => (
                state.workdir.clone(),
                !state.include_closed,
                state.entries.get(state.selected).map(|e| e.number),
            ),
            _ => return,
        };
        match GhCli::list_prs(&workdir, include_closed) {
            Ok(entries) => {
                let selected = current
                    .and_then(|n| entries.iter().position(|e| e.number == n))
                    .unwrap_or(0);
                if let AppMode::PrPicker(state) = &mut self.mode {
                    state.entries = entries;
                    state.selected = selected;
                    state.include_closed = include_closed;
                    state.error = None;
                }
            }
            Err(e) => {
                if let AppMode::PrPicker(state) = &mut self.mode {
                    state.error = Some(e.to_string());
                }
            }
        }
    }

    /// Resolve the highlighted PR (by number) and open it for review. On a
    /// resolve failure the picker stays open with an inline error.
    pub fn pr_picker_choose(&mut self) {
        let (workdir, number) = match &self.mode {
            AppMode::PrPicker(state) => match state.entries.get(state.selected) {
                Some(entry) => (state.workdir.clone(), entry.number),
                None => return,
            },
            _ => return,
        };
        match GhCli::fetch_pr_by_number(&workdir, number) {
            Ok(pr) => self.enter_pr_review(workdir, pr),
            Err(e) => {
                if let AppMode::PrPicker(state) = &mut self.mode {
                    state.error = Some(e.to_string());
                }
            }
        }
    }

    /// Switch from the picker to the manual PR-number prompt (the `#` key), so
    /// "pick a PR" and "type a number" live behind one entry point.
    pub fn pr_picker_to_number_prompt(&mut self) {
        let workdir = match &self.mode {
            AppMode::PrPicker(state) => state.workdir.clone(),
            _ => return,
        };
        self.prompt_pr_number(workdir, None);
    }

    /// Open the manual PR-number override prompt. Used when the branch has no
    /// auto-detectable open PR; `error` seeds an inline message after a failed
    /// resolve so the user can correct the number and retry.
    fn prompt_pr_number(&mut self, workdir: PathBuf, error: Option<String>) {
        self.mode = AppMode::PrNumberPrompt(PrNumberPromptState {
            workdir,
            input: String::new(),
            error,
        });
    }

    /// Append a digit to the PR-number prompt (non-digits are ignored).
    pub fn pr_number_prompt_push(&mut self, c: char) {
        if let AppMode::PrNumberPrompt(state) = &mut self.mode
            && c.is_ascii_digit()
        {
            state.input.push(c);
        }
    }

    /// Delete the last digit from the PR-number prompt.
    pub fn pr_number_prompt_backspace(&mut self) {
        if let AppMode::PrNumberPrompt(state) = &mut self.mode {
            state.input.pop();
        }
    }

    /// Resolve the typed PR number and, on success, start the comment fetch.
    /// On failure the prompt stays open with an inline error so the user can
    /// retry. Spends zero agent tokens (one `gh pr view <n>` call).
    pub fn submit_pr_number(&mut self) {
        let AppMode::PrNumberPrompt(state) = &self.mode else {
            return;
        };
        let workdir = state.workdir.clone();
        let Ok(number) = state.input.trim().parse::<u32>() else {
            self.prompt_pr_number(workdir, Some("Enter a PR number, e.g. 321".to_string()));
            return;
        };

        match GhCli::fetch_pr_by_number(&workdir, number) {
            Ok(pr) => self.enter_pr_review(workdir, pr),
            Err(e) => self.prompt_pr_number(workdir, Some(e.to_string())),
        }
    }

    /// Re-fetch the currently-open PR, bypassing the cache. Re-resolves the PR
    /// first so a new head SHA (e.g. after pushing fixes) is picked up, then
    /// fetches fresh comments and overwrites the cache row. Zero agent tokens.
    pub fn refresh_pr_review(&mut self) {
        let (workdir, number) = match &self.mode {
            AppMode::PrReview(state) => (state.workdir.clone(), state.review.pr.number),
            _ => return,
        };
        self.log_info("pr_review", format!("refreshing PR #{number}"));
        match GhCli::fetch_pr_by_number(&workdir, number) {
            Ok(pr) => self.start_pr_review_fetch(workdir, pr),
            Err(e) => self.show_error(e),
        }
    }

    /// Spawn the off-thread comment fetch and enter the loading mode.
    fn start_pr_review_fetch(&mut self, workdir: PathBuf, pr: PrRef) {
        self.log_info(
            "pr_review",
            format!("fetching comments for PR #{}", pr.number),
        );

        let (tx, rx) = std::sync::mpsc::channel();
        self.pr_review_bg = Some(rx);

        let usage_baselines = match &self.mode {
            AppMode::PrReview(state) if state.review.pr.number == pr.number => {
                state.usage_baselines.clone()
            }
            _ => self.pr_review_initial_usage_baselines(&workdir),
        };

        let thread_workdir = workdir.clone();
        let thread_pr = pr.clone();
        std::thread::spawn(move || {
            let _ = tx.send(fetch_and_normalize(&thread_workdir, thread_pr));
        });

        self.mode = AppMode::PrReviewLoading(PrReviewLoadState {
            workdir,
            pr,
            usage_baselines,
        });
    }

    /// Whether a PR comment fetch is in flight.
    pub fn pr_review_loading(&self) -> bool {
        matches!(self.mode, AppMode::PrReviewLoading(_))
    }

    /// Poll the background PR fetch. On completion, transition to the review
    /// pane (or report the error and return to the dashboard). Returns `true`
    /// when state changed and a redraw is warranted.
    pub fn poll_pr_review_bg(&mut self) -> bool {
        let Some(rx) = self.pr_review_bg.as_ref() else {
            return false;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.pr_review_bg = None;
                // If the user navigated away from the loading screen, drop it.
                let AppMode::PrReviewLoading(state) = &self.mode else {
                    return false;
                };
                let workdir = state.workdir.clone();
                let usage_baselines = state.usage_baselines.clone();
                match result {
                    Ok(mut review) => {
                        self.log_info(
                            "pr_review",
                            format!("loaded {} comments", review.comments.len()),
                        );
                        self.cache_pr_review(&review);
                        self.apply_persisted_triage(&mut review);
                        let checked_out_branch =
                            crate::worktree::WorktreeManager::current_branch(&workdir)
                                .unwrap_or(None);
                        self.mode = AppMode::PrReview(PrReviewState {
                            workdir,
                            review,
                            selected: 0,
                            detail_scroll: 0,
                            detail_content_lines: 0,
                            hide_resolved: false,
                            sort_mode: PrSortMode::default(),
                            fix_target: FixTarget::default(),
                            fix_target_picked: false,
                            usage_baselines,
                            review_harness: None,
                            harness_pick: None,
                            fix_confirm: None,
                            fix_vim_enabled: false,
                            mark_pick: None,
                            reply_kind_pick: None,
                            reply: None,
                            memory_add: None,
                            marked: std::collections::HashSet::new(),
                            pending_batch: false,
                            checked_out_branch,
                        });
                    }
                    Err(e) => {
                        self.mode = AppMode::Normal;
                        self.show_error(e);
                    }
                }
                true
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => false,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pr_review_bg = None;
                if matches!(self.mode, AppMode::PrReviewLoading(_)) {
                    self.mode = AppMode::Normal;
                    self.message = Some("PR fetch failed unexpectedly".to_string());
                    return true;
                }
                false
            }
        }
    }

    /// Close PR Triage / cancel a pending load and return to the dashboard.
    pub fn close_pr_review(&mut self) {
        self.pr_review_bg = None;
        self.mode = AppMode::Normal;
    }

    pub fn pr_review_select_next(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            let visible = state.visible_indices();
            let next = match visible.iter().position(|&i| i == state.selected) {
                Some(pos) => visible.get(pos + 1).copied(),
                None => visible.first().copied(),
            };
            if let Some(next) = next {
                state.selected = next;
                state.detail_scroll = 0;
            }
        }
    }

    pub fn pr_review_select_prev(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            let visible = state.visible_indices();
            let prev = match visible.iter().position(|&i| i == state.selected) {
                Some(0) => None,
                Some(pos) => visible.get(pos - 1).copied(),
                None => visible.last().copied(),
            };
            if let Some(prev) = prev {
                state.selected = prev;
                state.detail_scroll = 0;
            }
        }
    }

    /// Toggle hiding GitHub-resolved comments. When the current selection
    /// becomes hidden, snap to its nearest remaining visible neighbor in sort
    /// order (falling back to the closest one before it, then the first
    /// visible comment).
    pub fn pr_review_toggle_resolved(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.hide_resolved = !state.hide_resolved;
            let visible = state.visible_indices();
            if visible.is_empty() {
                return;
            }
            if !visible.contains(&state.selected) {
                let order = state.all_sorted_indices();
                let pos = order.iter().position(|&i| i == state.selected);
                let snapped = pos
                    .and_then(|p| order[p..].iter().find(|i| visible.contains(i)))
                    .or_else(|| {
                        pos.and_then(|p| order[..p].iter().rev().find(|i| visible.contains(i)))
                    })
                    .copied()
                    .unwrap_or(visible[0]);
                state.selected = snapped;
                state.detail_scroll = 0;
            }
        }
    }

    /// Cycle the comment list's sort order (`o`): fetch order → by file → by
    /// author → humans-first → back to fetch order. Independent of the
    /// `hide_resolved` filter.
    pub fn pr_review_cycle_sort(&mut self) {
        let label = {
            let AppMode::PrReview(state) = &mut self.mode else {
                return;
            };
            state.sort_mode = state.sort_mode.next();
            state.sort_mode.label()
        };
        self.push_toast_success(format!("Sort: {label}"));
    }

    /// Set `fix_target`, marking the fix-target picker resolved for the rest
    /// of this pane visit, and snapshot the newly-targeted session's current
    /// usage as a baseline if it doesn't already have one — so the "this
    /// visit" tally starts from zero for the just-selected target rather than
    /// including whatever that session had accrued before this pane opened.
    fn pr_review_set_fix_target(&mut self, target: FixTarget) {
        let workdir = match &self.mode {
            AppMode::PrReview(state) => state.workdir.clone(),
            _ => return,
        };
        let baseline = self.fix_session_usage_for(&workdir, target);
        if let AppMode::PrReview(state) = &mut self.mode {
            state.fix_target = target;
            state.fix_target_picked = true;
            if let Some(usage) = baseline {
                state
                    .usage_baselines
                    .entry(usage.source.clone())
                    .or_insert(usage);
            }
        }
    }

    /// Open the fix confirm/edit dialog for the selected comment. Assembles the
    /// minimal fix prompt and shows it for review (with a `~N tokens` preview)
    /// before anything reaches the agent — nothing is injected until the user
    /// confirms. Editing is opt-in (`e`) from the dialog.
    ///
    /// The first fix/batch of a pane visit first opens the fix-target picker
    /// (existing live session, or a dedicated session on a chosen harness) —
    /// the fix confirm follows once the user picks. Subsequent fixes (target
    /// already chosen, or a dedicated session already exists) go straight to
    /// the dialog.
    pub fn pr_review_open_fix_confirm(&mut self) {
        if self.pr_review_needs_harness_pick() {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.pending_batch = false;
            }
            self.pr_review_open_harness_pick();
            return;
        }
        self.pr_review_show_fix_confirm();
    }

    /// Build and open the single-comment fix confirm dialog for the selected
    /// comment. Assumes any harness pick has already happened (callers gate it),
    /// so it never re-opens the picker.
    fn pr_review_show_fix_confirm(&mut self) {
        let AppMode::PrReview(state) = &mut self.mode else {
            return;
        };
        state.pending_batch = false;
        let Some(comment) = state.selected_comment() else {
            self.message = Some("No comment selected".into());
            return;
        };
        let prompt = comment.fix_prompt();
        let vim = state.fix_vim_enabled;
        state.fix_confirm = Some(new_fix_confirm(prompt, vim, None));
    }

    /// Open the **combined-batch** confirm dialog (`B`): assemble one numbered
    /// prompt from every marked, not-yet-resolved comment and show it (with a
    /// `~N tokens` preview and editing) before injecting it once into the
    /// dedicated triage session — the "fix all of these, then I'll come back"
    /// flow. Requires a non-empty marked set (`space` to mark); like a single
    /// fix, the first fix of a dedicated-review PR picks the harness first.
    pub fn pr_review_open_batch_confirm(&mut self) {
        // A marked, not-all-resolved set is required before we touch the harness
        // picker or build anything.
        let valid = match &self.mode {
            AppMode::PrReview(state) => {
                if state.marked.is_empty() {
                    self.message = Some("No comments marked — press space to mark".into());
                    return;
                }
                state
                    .review
                    .comments
                    .iter()
                    .filter(|c| state.marked.contains(&c.id) && !c.is_resolved)
                    .count()
            }
            _ => return,
        };
        if valid == 0 {
            self.message = Some("Marked comments are all resolved — nothing to batch".into());
            return;
        }
        // The dedicated-review target picks a harness before the first fix, same
        // as a single fix; route the picker's continuation back to the batch.
        if self.pr_review_needs_harness_pick() {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.pending_batch = true;
            }
            self.pr_review_open_harness_pick();
            return;
        }
        self.pr_review_show_batch_confirm();
    }

    /// Build and open the combined-batch confirm dialog. Assumes the marked set
    /// was already validated and any harness pick has happened.
    fn pr_review_show_batch_confirm(&mut self) {
        let built = match &self.mode {
            AppMode::PrReview(state) => {
                let selected: Vec<&PrComment> = state
                    .review
                    .comments
                    .iter()
                    .filter(|c| state.marked.contains(&c.id) && !c.is_resolved)
                    .collect();
                (!selected.is_empty()).then(|| {
                    let ids: Vec<u64> = selected.iter().map(|c| c.id).collect();
                    (combined_fix_prompt(&selected), ids)
                })
            }
            _ => return,
        };
        let Some((prompt, ids)) = built else {
            self.message = Some("Marked comments are all resolved — nothing to batch".into());
            return;
        };

        // Keep the set bounded: warn (but don't block) past the soft ceilings so
        // the user knows a single prompt this large may exceed the context window.
        let count = ids.len();
        let tokens = estimate_tokens(&prompt);
        if count > BATCH_COMBINED_COMMENT_WARN || tokens > BATCH_COMBINED_TOKEN_WARN {
            self.push_toast_warning(format!(
                "Large batch: {count} comments (~{tokens} tokens) in one prompt — may exceed the agent's context window"
            ));
        }

        if let AppMode::PrReview(state) = &mut self.mode {
            state.pending_batch = false;
            let vim = state.fix_vim_enabled;
            state.fix_confirm = Some(new_fix_confirm(prompt, vim, Some(ids)));
        }
    }

    /// Whether the first `f`/`B` of this pane visit should pick a fix target
    /// before injecting: skipped once the target's already been picked (or a
    /// dedicated session already exists — a cache re-open inherits the
    /// running session's harness, so don't ask again).
    fn pr_review_needs_harness_pick(&self) -> bool {
        let AppMode::PrReview(state) = &self.mode else {
            return false;
        };
        if state.fix_target_picked || state.review_harness.is_some() {
            return false;
        }
        match self.feature_indices_for_workdir(&state.workdir) {
            Some((pi, fi)) => {
                let feature = &self.store.projects[pi].features[fi];
                pr_triage_session_index(feature, FixTarget::DedicatedReview).is_none()
            }
            // No feature resolved yet — let the inject path surface the error.
            None => false,
        }
    }

    /// Open the single-select fix-target picker: an "existing live session"
    /// row plus one "dedicated session" row per allowed harness, highlighting
    /// the project's preferred agent's dedicated row by default (matching
    /// `FixTarget::default()`). No-op if no harnesses are available for a
    /// dedicated session — falls back to the existing-live-less default
    /// (dedicated, no explicit harness) and skips straight to the confirm
    /// dialog, since there'd be nothing to choose between anyway.
    fn pr_review_open_harness_pick(&mut self) {
        let workdir = match &self.mode {
            AppMode::PrReview(state) => state.workdir.clone(),
            _ => return,
        };
        let agents = self.allowed_agents_for_project_path(&workdir);
        if agents.is_empty() {
            return self.pr_review_skip_harness_pick();
        }
        let preferred = self
            .feature_indices_for_workdir(&workdir)
            .map(|(pi, _)| self.store.projects[pi].preferred_agent.clone());
        let dedicated_default = preferred
            .and_then(|p| agents.iter().position(|a| *a == p))
            .unwrap_or(0);
        let existing_live_label =
            self.feature_indices_for_workdir(&workdir)
                .and_then(|(pi, fi)| {
                    let feature = &self.store.projects[pi].features[fi];
                    pr_triage_session_index(feature, FixTarget::ExistingLive)
                        .map(|idx| feature.sessions[idx].label.clone())
                });
        let mut rows = vec![FixTargetPickRow::ExistingLive(existing_live_label)];
        rows.extend(agents.into_iter().map(FixTargetPickRow::Dedicated));
        // +1: rows[0] is the ExistingLive row, so the dedicated default shifts by one.
        let selected = dedicated_default + 1;
        if let AppMode::PrReview(state) = &mut self.mode {
            state.harness_pick = Some(HarnessPickState { rows, selected });
        }
    }

    /// Skip the fix-target picker (e.g. no harnesses available): continue
    /// straight to the confirm dialog, leaving `fix_target`/`review_harness`
    /// at their defaults, but still marking the pick resolved so it isn't
    /// re-offered on the next fix.
    fn pr_review_skip_harness_pick(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.harness_pick = None;
            state.fix_target_picked = true;
        }
        self.pr_review_continue_after_harness();
    }

    /// After the fix target is chosen (or skipped), open the dialog the
    /// pending action wanted: the combined-batch confirm for the `B` flow,
    /// otherwise the single-comment fix confirm. Neither re-checks the pick,
    /// so this can't loop back into the picker.
    fn pr_review_continue_after_harness(&mut self) {
        let batch = matches!(&self.mode, AppMode::PrReview(state) if state.pending_batch);
        if batch {
            self.pr_review_show_batch_confirm();
        } else {
            self.pr_review_show_fix_confirm();
        }
    }

    /// Whether the fix-target picker is currently open over PR Triage.
    pub fn pr_review_harness_picking(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state) if state.harness_pick.is_some()
        )
    }

    /// Move the fix-target-picker highlight (`+1`/`-1`, wrapping).
    pub fn pr_review_harness_pick_move(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(pick) = &mut state.harness_pick
            && !pick.rows.is_empty()
        {
            let len = pick.rows.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Confirm the fix-target picker: remember the choice (and, for a
    /// dedicated row, the harness) for the rest of this pane visit, and
    /// continue into the fix confirm dialog.
    pub fn pr_review_harness_pick_confirm(&mut self) {
        let chosen = match &self.mode {
            AppMode::PrReview(state) => state
                .harness_pick
                .as_ref()
                .and_then(|p| p.rows.get(p.selected).cloned()),
            _ => return,
        };
        let Some(row) = chosen else {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.harness_pick = None;
            }
            return;
        };
        match &row {
            FixTargetPickRow::ExistingLive(_) => {
                self.pr_review_set_fix_target(FixTarget::ExistingLive);
                if let AppMode::PrReview(state) = &mut self.mode {
                    state.harness_pick = None;
                    state.review_harness = None;
                }
                self.push_toast_success("Fixes target the existing live session".to_string());
            }
            FixTargetPickRow::Dedicated(agent) => {
                self.pr_review_set_fix_target(FixTarget::DedicatedReview);
                if let AppMode::PrReview(state) = &mut self.mode {
                    state.harness_pick = None;
                    state.review_harness = Some(agent.clone());
                }
                self.push_toast_success(format!(
                    "Triage session will run {}",
                    agent.display_name()
                ));
            }
        }
        // Continue into the dialog the pending action wanted (single or batch).
        self.pr_review_continue_after_harness();
    }

    /// Cancel the fix-target picker without choosing — aborts this fix; the
    /// user can press `f`/`B` again. Nothing is marked picked, so the picker
    /// reappears, and any pending batch is discarded.
    pub fn pr_review_harness_pick_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.harness_pick = None;
            state.pending_batch = false;
        }
    }

    /// Whether the fix confirm/edit dialog is currently open, and if so whether
    /// it is in edit mode. `None` means no dialog is open.
    pub fn pr_review_fix_editing(&self) -> Option<bool> {
        match &self.mode {
            AppMode::PrReview(state) => state.fix_confirm.as_ref().map(|c| c.editing),
            _ => None,
        }
    }

    /// Close the fix confirm/edit dialog without injecting anything.
    pub fn pr_review_cancel_fix(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.fix_confirm = None;
        }
    }

    /// Switch the open fix dialog into edit mode so keystrokes flow to the
    /// prompt editor. No-op when the dialog is closed or already editing.
    pub fn pr_review_fix_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
        {
            confirm.editing = true;
        }
    }

    /// Leave edit mode, returning to the confirm view (the prompt is kept).
    pub fn pr_review_fix_stop_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
        {
            confirm.editing = false;
        }
    }

    /// Forward a key to the open fix-prompt editor (only meaningful in edit
    /// mode). Returns `true` when a dialog editor consumed the key. Requests a
    /// cursor-follow scroll when the edit moved the cursor or changed the text.
    pub fn pr_review_fix_editor_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
            && confirm.editing
        {
            let outcome = confirm.editor.handle_key(key);
            if outcome.text_changed || outcome.cursor_moved {
                confirm.sync_to_cursor = true;
            }
            return true;
        }
        false
    }

    /// Toggle the vim keymap on the open fix-prompt editor, remembering the
    /// choice on the pane so reopening the dialog keeps it. No-op when closed.
    pub fn pr_review_fix_toggle_vim(&mut self) {
        let AppMode::PrReview(state) = &mut self.mode else {
            return;
        };
        let Some(confirm) = &mut state.fix_confirm else {
            return;
        };
        confirm.editor.toggle_vim();
        confirm.sync_to_cursor = true;
        let on = confirm.editor.vim_mode().is_some();
        state.fix_vim_enabled = on;
        self.message = Some(if on {
            "Vim mode enabled".into()
        } else {
            "Vim mode disabled".into()
        });
    }

    /// Scroll the fix-prompt editor by `delta` visual rows (positive = down).
    /// Clears cursor-follow so the user can scroll away from the cursor; the
    /// final clamp to content happens during rendering.
    pub fn pr_review_fix_scroll(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(confirm) = &mut state.fix_confirm
        {
            confirm.scroll = confirm.scroll.saturating_add_signed(delta);
            confirm.sync_to_cursor = false;
        }
    }

    /// The vim mode of the open fix-prompt editor, or `None` when the dialog is
    /// closed or the editor is in plain (non-vim) mode. Drives `Esc` handling
    /// (vim consumes `Esc` for Insert→Normal) and the dialog's mode label.
    pub fn pr_review_fix_vim_mode(&self) -> Option<crate::editor::VimMode> {
        match &self.mode {
            AppMode::PrReview(state) => {
                state.fix_confirm.as_ref().and_then(|c| c.editor.vim_mode())
            }
            _ => None,
        }
    }

    /// Confirm the dialog: inject the (possibly edited) prompt into the chosen
    /// agent session and switch the user into that session to watch it (no
    /// auto-advance). The dedicated triage session is spun up on first use and
    /// reused thereafter; the existing-live target reuses the feature's running
    /// agent session. Delivery goes through the shared compose / prompt-library
    /// seam: pasted without sending so the user reviews before it runs.
    ///
    /// Handles both a single-comment fix and a **combined batch** (the `B` flow,
    /// `FixConfirmState::batch`): the batch injects one numbered prompt and marks
    /// every included comment `Fixing`, then clears the marked set.
    pub fn pr_review_inject_fix(&mut self) -> Result<()> {
        let (prompt, pr_number, head_sha, fixing_ids, is_batch) = match &self.mode {
            AppMode::PrReview(state) => {
                let pr_number = state.review.pr.number;
                let head_sha = state.review.pr.head_sha.clone();
                let (prompt, ids, is_batch) = match &state.fix_confirm {
                    // Confirming the open dialog uses its edited buffer. A batch
                    // dialog carries every included comment id; a single one
                    // targets the current selection.
                    Some(confirm) => {
                        let prompt = confirm.editor.text().trim().to_string();
                        match &confirm.batch {
                            Some(batch_ids) => (prompt, batch_ids.clone(), true),
                            None => (
                                prompt,
                                state.selected_comment().map(|c| c.id).into_iter().collect(),
                                false,
                            ),
                        }
                    }
                    // No dialog open (e.g. empty pane): fall back to the selection.
                    None => match state.selected_comment() {
                        Some(c) => (c.fix_prompt(), vec![c.id], false),
                        None => {
                            self.message = Some("No comment selected".into());
                            return Ok(());
                        }
                    },
                };
                (prompt, pr_number, head_sha, ids, is_batch)
            }
            _ => return Ok(()),
        };

        if prompt.is_empty() {
            self.message = Some("Nothing to inject — the prompt is empty".into());
            return Ok(());
        }

        let (pi, fi, si) = match self.resolve_fix_session() {
            Ok(target) => target,
            Err(e) => {
                self.show_error(e);
                return Ok(());
            }
        };

        // The fix is committed: mark every targeted comment `Fixing` and persist
        // before we leave the pane, so re-opening the review (cache hit) shows
        // the state.
        for id in &fixing_ids {
            if let AppMode::PrReview(state) = &mut self.mode
                && let Some(c) = state.review.comments.iter_mut().find(|c| c.id == *id)
            {
                c.triage = TriageState::Fixing;
            }
            self.persist_triage(pr_number, &head_sha, *id, TriageState::Fixing, None);
        }
        // A batch consumes the marked set once it's committed.
        if is_batch {
            if let AppMode::PrReview(state) = &mut self.mode {
                state.marked.clear();
            }
            self.push_toast_success(format!(
                "Injected a combined fix for {} comments",
                fixing_ids.len()
            ));
        }

        // Stash the pane's exact state so leader+P can jump straight back to
        // it without re-fetching — the same mechanism the `P` toggle uses.
        // Leaving the pane is still intentional (the user watches the agent),
        // but the round trip back to triage the next comment no longer has to
        // go through the dashboard and a re-resolve. The confirm dialog has
        // already served its purpose (the prompt above was read from it), so
        // clear it before stashing — otherwise returning would reopen the
        // same "inject fix" dialog instead of the plain comment list.
        let AppMode::PrReview(mut state) = std::mem::replace(&mut self.mode, AppMode::Normal)
        else {
            return Ok(());
        };
        state.fix_confirm = None;
        let feature = &self.store.projects[pi].features[fi];
        self.pr_review_return = Some(PrReviewReturn {
            session: feature.tmux_session.clone(),
            window: feature.sessions[si].tmux_window.clone(),
            state,
        });

        // Switch into the target session, then deliver the prompt via the
        // shared seam (seeds the compose box when interception is on, else
        // pastes without sending).
        self.selection = Selection::Session(pi, fi, si);
        self.enter_view_without_auto_compose()?;
        let AppMode::Viewing(view) = &self.mode else {
            return Ok(());
        };
        let view = view.clone();
        self.deliver_prompt(prompt, Some(view))
    }

    /// Jump from PR Triage straight into the linked fix session (`P`),
    /// stashing the pane's exact state (selection, scroll, open dialogs) so
    /// `pr_review_return_to_pane` can pop back to it without re-fetching.
    /// Unlike `f`, this never spins up the dedicated session — it only jumps
    /// to one that already exists, so a quick "peek at the agent" doesn't
    /// have the side effect of starting a triage session on its own.
    pub fn pr_review_toggle_to_session(&mut self) -> Result<()> {
        let state = match std::mem::replace(&mut self.mode, AppMode::Normal) {
            AppMode::PrReview(state) => state,
            other => {
                self.mode = other;
                return Ok(());
            }
        };

        let Some((pi, fi)) = self.feature_indices_for_workdir(&state.workdir) else {
            self.mode = AppMode::PrReview(state);
            self.push_toast_warning("Could not find the feature for this PR");
            return Ok(());
        };
        let feature = &self.store.projects[pi].features[fi];
        let Some(si) = pr_triage_session_index(feature, state.fix_target) else {
            self.mode = AppMode::PrReview(state);
            self.push_toast_warning("No triage session yet — press f to start one");
            return Ok(());
        };
        let session = feature.tmux_session.clone();
        let window = feature.sessions[si].tmux_window.clone();

        self.selection = Selection::Session(pi, fi, si);
        self.pr_review_return = Some(PrReviewReturn {
            session,
            window,
            state,
        });
        self.enter_view_without_auto_compose()
    }

    /// Jump back from a Viewing session to the review pane stashed by
    /// `pr_review_toggle_to_session` (`leader+P`), restoring the exact prior
    /// state — no re-fetch. Only restores when the current session is the one
    /// the stash was jumped from; a stash left behind after navigating
    /// elsewhere is not popped into an unrelated session's view.
    pub fn pr_review_return_to_pane(&mut self) {
        let Some(stash) = &self.pr_review_return else {
            self.push_toast_warning("No PR Triage pane to return to");
            return;
        };
        let matches_current = matches!(
            &self.mode,
            AppMode::Viewing(view) if view.session == stash.session && view.window == stash.window
        );
        if !matches_current {
            self.push_toast_warning("No PR Triage pane linked to this session");
            return;
        }
        if let Some(stash) = self.pr_review_return.take() {
            self.mode = AppMode::PrReview(stash.state);
        }
    }

    /// Open the reply-kind picker (`R`): a two-row choice between a "Done in
    /// `<sha>`" report and a "not needed" explanation, shown before the
    /// actual reply dialog. Replaces the old separate `R`/`n` top-level keys
    /// with one entry point. No-op (with a hint) if nothing is selected or
    /// another dialog is already open.
    pub fn pr_review_open_reply_pick(&mut self) {
        let ready = match &self.mode {
            AppMode::PrReview(state)
                if state.reply.is_none()
                    && state.fix_confirm.is_none()
                    && state.reply_kind_pick.is_none() =>
            {
                state.selected_comment().is_some()
            }
            _ => return,
        };
        if !ready {
            self.message = Some("No comment selected".into());
            return;
        }
        if let AppMode::PrReview(state) = &mut self.mode {
            state.reply_kind_pick = Some(ReplyKindPickState { selected: 0 });
        }
    }

    /// Whether the reply-kind picker is currently open over PR Triage.
    pub fn pr_review_reply_pick_picking(&self) -> bool {
        matches!(
            &self.mode,
            AppMode::PrReview(state) if state.reply_kind_pick.is_some()
        )
    }

    /// Move the reply-kind-picker highlight (`+1`/`-1`, wrapping).
    pub fn pr_review_reply_pick_move(&mut self, delta: isize) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(pick) = &mut state.reply_kind_pick
        {
            let len = ReplyKind::ALL.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Confirm the reply-kind picker: close it and open the corresponding
    /// reply dialog (reusing the existing `Done`/`NotNeeded` flows as-is).
    pub fn pr_review_reply_pick_confirm(&mut self) {
        let chosen = match &self.mode {
            AppMode::PrReview(state) => state
                .reply_kind_pick
                .as_ref()
                .map(|pick| ReplyKind::ALL[pick.selected]),
            _ => return,
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            state.reply_kind_pick = None;
        }
        match chosen {
            Some(ReplyKind::Done) => self.pr_review_open_reply_done(),
            Some(ReplyKind::NotNeeded) => self.pr_review_open_reply_not_needed(),
            None => {}
        }
    }

    /// Cancel the reply-kind picker without choosing.
    pub fn pr_review_reply_pick_cancel(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.reply_kind_pick = None;
        }
    }

    /// Open a **"Done in `<sha>`"** reply for the selected comment, seeded from
    /// the most recent commit that plausibly fixed it — a commit touching the
    /// comment's file/line — falling back to bare `HEAD` (flagged "latest
    /// commit") when history search comes up empty. Editable before posting;
    /// posting marks the comment `Done`. Reached via the reply-kind picker
    /// (`R`), or called directly by tests/internal flows.
    pub fn pr_review_open_reply_done(&mut self) {
        let (workdir, comment) = match &self.mode {
            AppMode::PrReview(state) if state.reply.is_none() && state.fix_confirm.is_none() => {
                (state.workdir.clone(), state.selected_comment().cloned())
            }
            _ => return,
        };
        let seed = match comment {
            Some(comment) => match commit_for_done_reply(&workdir, &comment) {
                (Some(sha), true) => format!("Done in `{sha}`."),
                (Some(sha), false) => format!("Done in `{sha}` (latest commit)."),
                (None, _) => "Done.".to_string(),
            },
            // No comment selected: `open_reply` below reports "No comment
            // selected" — the seed is unused in that path.
            None => String::new(),
        };
        self.open_reply(ReplyKind::Done, seed);
    }

    /// Open a **"not needed"** reply for the selected comment: an empty editor
    /// for the user to explain *why* a fix isn't needed. Posting marks the
    /// comment `Skipped` and stores the explanation as its local note.
    pub fn pr_review_open_reply_not_needed(&mut self) {
        self.open_reply(ReplyKind::NotNeeded, String::new());
    }

    /// Shared entry: open the reply dialog for the selected comment with a kind
    /// and a seeded body. No-op if a fix/reply dialog is already open or nothing
    /// is selected.
    fn open_reply(&mut self, kind: ReplyKind, seed: String) {
        let comment_id = match &self.mode {
            AppMode::PrReview(state) if state.reply.is_none() && state.fix_confirm.is_none() => {
                state.selected_comment().map(|c| c.id)
            }
            _ => return,
        };
        let Some(comment_id) = comment_id else {
            self.message = Some("No comment selected".into());
            return;
        };
        // Not-needed replies start in edit mode (the user must type a reason);
        // the done template is post-ready, so it opens in the confirm view.
        let editing = matches!(kind, ReplyKind::NotNeeded);
        if let AppMode::PrReview(state) = &mut self.mode {
            state.reply = Some(ReplyState {
                comment_id,
                kind,
                editor: TextEditor::new(seed),
                editing,
            });
        }
    }

    /// Enter edit mode so keystrokes flow to the reply editor.
    pub fn pr_review_reply_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(reply) = &mut state.reply
        {
            reply.editing = true;
        }
    }

    /// Leave edit mode, returning to the confirm view (the body is kept).
    pub fn pr_review_reply_stop_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(reply) = &mut state.reply
        {
            reply.editing = false;
        }
    }

    /// Forward a key to the open reply editor (only meaningful in edit mode).
    pub fn pr_review_reply_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(reply) = &mut state.reply
            && reply.editing
        {
            reply.editor.handle_key(key);
        }
    }

    /// Close the reply dialog without posting.
    pub fn pr_review_cancel_reply(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.reply = None;
        }
    }

    /// Reply-dialog status for the key handler: `None` when closed, else whether
    /// it is currently in edit mode.
    pub fn pr_review_reply_view(&self) -> Option<bool> {
        match &self.mode {
            AppMode::PrReview(state) => state.reply.as_ref().map(|r| r.editing),
            _ => None,
        }
    }

    /// Post the (possibly edited) reply to GitHub and close the dialog. Inline
    /// comments reply into their thread; conversation comments and review
    /// summaries post a new conversation comment. On success the comment is
    /// marked by the reply's kind — `Done` for a "done in `<sha>`" reply,
    /// `Skipped` (with the body kept as the local note) for a "not needed" one.
    /// The GitHub write runs only on the user's explicit confirm.
    pub fn pr_review_post_reply(&mut self) -> Result<()> {
        let prep = match &self.mode {
            AppMode::PrReview(state) => state.reply.as_ref().and_then(|reply| {
                let comment = state
                    .review
                    .comments
                    .iter()
                    .find(|c| c.id == reply.comment_id)?;
                Some((
                    state.workdir.clone(),
                    state.review.pr.clone(),
                    comment.reply_target(),
                    reply.kind,
                    reply.comment_id,
                    reply.editor.text().trim().to_string(),
                ))
            }),
            _ => return Ok(()),
        };
        let Some((workdir, pr, target, kind, comment_id, body)) = prep else {
            return Ok(());
        };

        if body.is_empty() {
            let hint = match kind {
                ReplyKind::NotNeeded => "Explain why a fix isn't needed, or esc to cancel",
                ReplyKind::Done => "Reply is empty — type something or esc to cancel",
            };
            self.message = Some(hint.into());
            return Ok(());
        }

        // The posted body carries the "posted via AMF" disclosure; the local
        // note (kept below for `NotNeeded`) stays the user's unmarked text —
        // it's AMF's own record, not something read on GitHub.
        let posted_body = append_amf_attribution(&body);
        let result = match target {
            ReplyTarget::InlineThread { root_comment_id } => GhCli::reply_to_review_comment(
                &workdir,
                &pr.owner,
                &pr.repo,
                pr.number,
                root_comment_id,
                &posted_body,
            ),
            ReplyTarget::Conversation => {
                GhCli::post_issue_comment(&workdir, &pr.owner, &pr.repo, pr.number, &posted_body)
            }
        };
        if let Err(e) = result {
            self.show_error(e);
            return Ok(());
        }

        // Apply the triage outcome for this reply kind and close the dialog.
        let (triage, note) = match kind {
            ReplyKind::Done => (TriageState::Done, None),
            ReplyKind::NotNeeded => (TriageState::Skipped, Some(body.clone())),
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            if let Some(c) = state
                .review
                .comments
                .iter_mut()
                .find(|c| c.id == comment_id)
            {
                c.triage = triage;
                c.local_note = note.clone();
            }
            state.reply = None;
        }
        self.persist_triage(pr.number, &pr.head_sha, comment_id, triage, note.as_deref());
        // Posting can flip a thread's resolution (e.g. GitHub auto-resolves, or
        // the reviewer resolved meanwhile), so re-pull thread state to keep the
        // `✓` marker honest. Zero agent tokens — one GraphQL call.
        self.refresh_thread_resolution();
        let toast = match kind {
            ReplyKind::Done => "Posted reply · marked done",
            ReplyKind::NotNeeded => "Posted reply · marked skipped",
        };
        self.push_toast_success(toast.to_string());
        Ok(())
    }

    /// Open the "add to memory" dialog for the selected comment, seeded from
    /// [`PrComment::memory_finding_seed`] and defaulting to the `General`
    /// category. Editable before it's appended. No-op if a fix/reply/memory
    /// dialog is already open or nothing is selected.
    pub fn pr_review_open_memory_add(&mut self) {
        let seed = match &self.mode {
            AppMode::PrReview(state)
                if state.reply.is_none()
                    && state.fix_confirm.is_none()
                    && state.memory_add.is_none() =>
            {
                state
                    .selected_comment()
                    .map(|c| (c.id, c.memory_finding_seed()))
            }
            _ => return,
        };
        let Some((comment_id, seed)) = seed else {
            self.message = Some("No comment selected".into());
            return;
        };
        if let AppMode::PrReview(state) = &mut self.mode {
            state.memory_add = Some(MemoryAddState {
                comment_id,
                category: 0,
                editor: TextEditor::new(seed),
                editing: false,
            });
        }
    }

    /// Enter edit mode so keystrokes flow to the finding editor.
    pub fn pr_review_memory_add_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
        {
            memory_add.editing = true;
        }
    }

    /// Leave edit mode, returning to the confirm view (the text is kept).
    pub fn pr_review_memory_add_stop_edit(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
        {
            memory_add.editing = false;
        }
    }

    /// Forward a key to the open finding editor (only meaningful in edit mode).
    pub fn pr_review_memory_add_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
            && memory_add.editing
        {
            memory_add.editor.handle_key(key);
        }
    }

    /// Cycle the category (confirm view only) through [`MEMORY_CATEGORIES`].
    pub fn pr_review_cycle_memory_category(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode
            && let Some(memory_add) = &mut state.memory_add
        {
            memory_add.category = (memory_add.category + 1) % MEMORY_CATEGORIES.len();
        }
    }

    /// Close the "add to memory" dialog without appending.
    pub fn pr_review_cancel_memory_add(&mut self) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.memory_add = None;
        }
    }

    /// Memory-add dialog status for the key handler: `None` when closed, else
    /// whether it is currently in edit mode.
    pub fn pr_review_memory_add_view(&self) -> Option<bool> {
        match &self.mode {
            AppMode::PrReview(state) => state.memory_add.as_ref().map(|m| m.editing),
            _ => None,
        }
    }

    /// Append the (possibly edited) finding to the review-memory doc and close
    /// the dialog. Whitespace/newlines in the finding text are collapsed to a
    /// single line first, since the doc stores each finding as one bullet.
    /// Dedup-aware and append-only (`review_memory::append_finding`) — never
    /// touches existing prose. Zero agent tokens; a local file write only.
    pub fn pr_review_append_memory(&mut self) -> Result<()> {
        let prep = match &self.mode {
            AppMode::PrReview(state) => state.memory_add.as_ref().map(|memory_add| {
                (
                    state.workdir.clone(),
                    MEMORY_CATEGORIES[memory_add.category],
                    memory_add.editor.text(),
                )
            }),
            _ => return Ok(()),
        };
        let Some((workdir, category, finding)) = prep else {
            return Ok(());
        };

        let finding = finding.split_whitespace().collect::<Vec<_>>().join(" ");
        if finding.is_empty() {
            self.message = Some("Finding is empty — type something or esc to cancel".into());
            return Ok(());
        }

        let repo = self.repo_for_project_path(&workdir);
        let path = review_memory::review_memory_path(
            &repo,
            self.configured_review_memory_path(&repo).as_deref(),
        );
        let appended = review_memory::append_finding(&path, category, &finding)?;

        if let AppMode::PrReview(state) = &mut self.mode {
            state.memory_add = None;
        }
        let toast = if appended {
            format!("Added to memory · {category}")
        } else {
            "Already in memory · skipped".to_string()
        };
        self.push_toast_success(toast);
        Ok(())
    }

    /// Toggle GitHub resolution of the selected comment's review thread via the
    /// GraphQL `resolveReviewThread` / `unresolveReviewThread` mutation. Only
    /// inline comments that belong to a thread can be resolved; conversation
    /// comments and review summaries have no thread, so this is a no-op with a
    /// hint. Independent of replying (the user may resolve without commenting).
    ///
    /// On success the new state is applied to every comment in that thread and
    /// the SQLite cache is refreshed so a later cache-hit re-open reflects it.
    /// Zero agent tokens.
    pub fn pr_review_toggle_resolve(&mut self) {
        let info = match &self.mode {
            AppMode::PrReview(state) => state
                .selected_comment()
                .map(|c| (state.workdir.clone(), c.thread_id.clone(), c.is_resolved)),
            _ => return,
        };
        let Some((workdir, thread_id, is_resolved)) = info else {
            self.message = Some("No comment selected".into());
            return;
        };
        let Some(thread_id) = thread_id else {
            self.message = Some("This comment has no resolvable review thread".into());
            return;
        };

        let desired = !is_resolved;
        let now_resolved = match GhCli::set_thread_resolved(&workdir, &thread_id, desired) {
            Ok(state) => state,
            Err(e) => {
                self.show_error(e);
                return;
            }
        };

        if let AppMode::PrReview(state) = &mut self.mode {
            for c in &mut state.review.comments {
                if c.thread_id.as_deref() == Some(thread_id.as_str()) {
                    c.is_resolved = now_resolved;
                }
            }
        }
        self.recache_current_review();
        let msg = if now_resolved {
            "Thread resolved"
        } else {
            "Thread reopened"
        };
        self.push_toast_success(msg.to_string());
    }

    /// Re-fetch GitHub review-thread resolution state and apply it to the
    /// in-memory review (and cache). One GraphQL call, zero agent tokens. A
    /// failure is non-fatal — the existing markers just stay as they were.
    fn refresh_thread_resolution(&mut self) {
        let (workdir, pr) = match &self.mode {
            AppMode::PrReview(state) => (state.workdir.clone(), state.review.pr.clone()),
            _ => return,
        };
        let threads = match GhCli::review_threads(&workdir, &pr.owner, &pr.repo, pr.number) {
            Ok(threads) => threads,
            Err(e) => {
                self.log_warn("pr_review", format!("thread refresh failed: {e}"));
                return;
            }
        };
        let index = index_threads(&threads);
        if let AppMode::PrReview(state) = &mut self.mode {
            for c in &mut state.review.comments {
                let github_id = c.github_id.unwrap_or(c.id);
                if let Some((tid, resolved)) = index.get(&github_id) {
                    c.thread_id = Some(tid.clone());
                    c.is_resolved = *resolved;
                }
            }
        }
        self.recache_current_review();
    }

    /// Re-persist the current in-memory review to the SQLite cache so a later
    /// cache-hit re-open reflects resolution changes made in the pane.
    fn recache_current_review(&mut self) {
        let review = match &self.mode {
            AppMode::PrReview(state) => state.review.clone(),
            _ => return,
        };
        self.cache_pr_review(&review);
    }

    /// Resolve (and, for the dedicated strategy, lazily create) the agent
    /// window that fix prompts target. Returns `(project, feature, session)`
    /// indices. Ensures the feature's tmux session is running first.
    fn resolve_fix_session(&mut self) -> Result<(usize, usize, usize)> {
        let (workdir, target, harness) = match &self.mode {
            AppMode::PrReview(state) => (
                state.workdir.clone(),
                state.fix_target,
                state.review_harness.clone(),
            ),
            _ => anyhow::bail!("not reviewing a PR"),
        };
        let (pi, fi) = self
            .feature_indices_for_workdir(&workdir)
            .ok_or_else(|| anyhow::anyhow!("could not find the feature for this PR"))?;

        self.ensure_feature_running_for_new_session(pi, fi)?;

        let feature = &self.store.projects[pi].features[fi];
        if let Some(si) = pr_triage_session_index(feature, target) {
            return Ok((pi, fi, si));
        }

        match target {
            FixTarget::DedicatedReview => {
                let si =
                    self.create_dedicated_review_session(pi, fi, TRIAGE_SESSION_LABEL, harness)?;
                Ok((pi, fi, si))
            }
            FixTarget::ExistingLive => {
                anyhow::bail!("no live agent session to reuse — switch to the dedicated target (t)")
            }
        }
    }

    /// Find the `(project, feature)` indices of the feature whose workdir
    /// matches `workdir`.
    pub(crate) fn feature_indices_for_workdir(&self, workdir: &Path) -> Option<(usize, usize)> {
        self.store.projects.iter().enumerate().find_map(|(pi, p)| {
            p.features
                .iter()
                .position(|f| f.workdir == workdir)
                .map(|fi| (pi, fi))
        })
    }

    fn fix_session_usage_for(
        &self,
        workdir: &Path,
        target: FixTarget,
    ) -> Option<crate::token_tracking::SessionTokenUsage> {
        let (pi, fi) = self.feature_indices_for_workdir(workdir)?;
        let feature = &self.store.projects[pi].features[fi];
        let si = pr_triage_session_index(feature, target)?;
        feature.sessions[si].token_usage.clone()
    }

    fn pr_review_initial_usage_baselines(
        &self,
        workdir: &Path,
    ) -> HashMap<crate::token_tracking::TokenUsageSource, crate::token_tracking::SessionTokenUsage>
    {
        self.fix_session_usage_for(workdir, FixTarget::default())
            .map(|usage| [(usage.source.clone(), usage)].into_iter().collect())
            .unwrap_or_default()
    }

    /// Token usage for the session the pane's current fix target resolves to,
    /// for a header display. Read-only — unlike [`App::resolve_fix_session`] it
    /// never creates a session, so this is safe to call on every frame just to
    /// render a number. `None` before any fix has spun up the target session.
    pub(crate) fn pr_review_fix_session_usage(
        &self,
    ) -> Option<crate::token_tracking::SessionTokenUsage> {
        let AppMode::PrReview(state) = &self.mode else {
            return None;
        };
        let (pi, fi) = self.feature_indices_for_workdir(&state.workdir)?;
        let feature = &self.store.projects[pi].features[fi];
        let si = pr_triage_session_index(feature, state.fix_target)?;
        feature.sessions[si].token_usage.clone()
    }

    /// Whether the dedicated PR-triage session exists and is actively
    /// thinking or running a tool. Claude and Codex activity is keyed by the
    /// AMF feature-session ID supplied by hooks/plugins; OpenCode and Pi reuse
    /// their existing sidebar and marker-based activity signals.
    pub(crate) fn pr_review_dedicated_session_working(&self) -> Option<bool> {
        let AppMode::PrReview(state) = &self.mode else {
            return None;
        };
        self.dedicated_review_session_working_for_workdir(&state.workdir)
    }

    /// Same as [`Self::pr_review_dedicated_session_working`] but for an
    /// arbitrary feature workdir rather than the currently open PR Triage
    /// pane — used by the ambient status badge shown while `Viewing` a
    /// session whose feature has an active PR.
    pub(crate) fn dedicated_review_session_working_for_workdir(
        &self,
        workdir: &Path,
    ) -> Option<bool> {
        let (pi, fi) = self.feature_indices_for_workdir(workdir)?;
        let feature = &self.store.projects[pi].features[fi];
        let si = pr_triage_session_index(feature, FixTarget::DedicatedReview)?;
        let session = &feature.sessions[si];
        Some(match session.kind {
            SessionKind::Opencode => self
                .opencode_sidebar_cache
                .get(&feature.tmux_session)
                .filter(|sidebar| {
                    session
                        .token_usage_source
                        .as_ref()
                        .filter(|source| {
                            source.provider == crate::token_tracking::TokenUsageProvider::Opencode
                        })
                        .is_none_or(|source| source.id == sidebar.session_id)
                })
                .and_then(super::sync::opencode_sidebar_thinking_state)
                .unwrap_or(false),
            SessionKind::Pi => Self::is_session_marked_thinking(&feature.tmux_session),
            _ => {
                self.ipc_thinking_feature_sessions.contains(&session.id)
                    || self.ipc_tool_feature_sessions.contains(&session.id)
            }
        })
    }

    /// Usage added to the selected fix target since this visit to the PR pane
    /// began. Existing sessions are snapshotted on entry (or when selected via
    /// `t`); a dedicated session created by the first fix starts from zero.
    pub(crate) fn pr_review_triage_session_usage(
        &self,
    ) -> Option<crate::token_tracking::SessionTokenUsage> {
        let AppMode::PrReview(state) = &self.mode else {
            return None;
        };
        let current = self.pr_review_fix_session_usage()?;
        let delta = state
            .usage_baselines
            .get(&current.source)
            .map(|baseline| crate::token_tracking::token_usage_delta(&current, baseline))
            .unwrap_or(current);
        (delta.input_tokens > 0
            || delta.output_tokens > 0
            || delta.cache_read_tokens > 0
            || delta.cache_write_tokens > 0
            || delta.reasoning_tokens > 0
            || delta.total_tokens > 0)
            .then_some(delta)
    }

    pub fn pr_review_scroll_detail_up(&mut self, amount: usize) {
        if let AppMode::PrReview(state) = &mut self.mode {
            state.detail_scroll = state.detail_scroll.saturating_sub(amount);
        }
    }

    pub fn pr_review_scroll_detail_down(&mut self, amount: usize) {
        if let AppMode::PrReview(state) = &mut self.mode {
            // The renderer records how many lines it last drew; clamp against
            // that so scrolling can't run past the rendered detail content.
            let max_scroll = state.detail_content_lines.saturating_sub(1);
            state.detail_scroll = (state.detail_scroll + amount).min(max_scroll);
        }
    }

    /// Open the lookback-bootstrap depth picker (`b` in the PR picker): an
    /// overlay on the picker, not a separate mode, mirroring how the fix
    /// harness picker overlays the review pane.
    pub fn open_review_memory_bootstrap_pick(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.bootstrap_pick = Some(BootstrapPickState {
                selected: BootstrapDepth::ALL
                    .iter()
                    .position(|d| *d == BootstrapDepth::default())
                    .unwrap_or(0),
            });
        }
    }

    /// Whether the bootstrap depth picker is currently open over the PR picker.
    pub fn review_memory_bootstrap_picking(&self) -> bool {
        matches!(&self.mode, AppMode::PrPicker(state) if state.bootstrap_pick.is_some())
    }

    /// Move the depth-picker highlight (`+1`/`-1`, wrapping).
    pub fn review_memory_bootstrap_pick_move(&mut self, delta: isize) {
        if let AppMode::PrPicker(state) = &mut self.mode
            && let Some(pick) = &mut state.bootstrap_pick
        {
            let len = BootstrapDepth::ALL.len() as isize;
            pick.selected = ((pick.selected as isize + delta).rem_euclid(len)) as usize;
        }
    }

    /// Close the depth picker without running anything, staying on the PR
    /// picker.
    pub fn review_memory_bootstrap_pick_cancel(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.bootstrap_pick = None;
        }
    }

    /// Confirm the chosen depth: resolve the recent closed/merged PRs
    /// synchronously (one cheap `gh` call), then hand the heavy work — the
    /// per-PR comment fetch loop and the one distill pass — to a background
    /// thread and switch to the full-screen running view.
    pub fn review_memory_bootstrap_pick_confirm(&mut self) {
        let (workdir, depth, mut origin) = match &self.mode {
            AppMode::PrPicker(state) => {
                let Some(pick) = &state.bootstrap_pick else {
                    return;
                };
                let depth = BootstrapDepth::ALL[pick.selected];
                (state.workdir.clone(), depth, state.clone())
            }
            _ => return,
        };
        origin.bootstrap_pick = None;

        let entries = match GhCli::list_recent_closed_prs(&workdir, depth.limit()) {
            Ok(entries) => entries,
            Err(e) => {
                self.mode = AppMode::PrPicker(origin);
                self.show_error(e);
                return;
            }
        };
        if entries.is_empty() {
            self.mode = AppMode::PrPicker(origin);
            self.message = Some("No merged/closed PRs found to learn from".into());
            return;
        }

        let repo = self.repo_for_project_path(&workdir);
        let memory_path = review_memory::review_memory_path(
            &repo,
            self.configured_review_memory_path(&repo).as_deref(),
        );

        self.log_info(
            "pr_review",
            format!(
                "bootstrapping review memory from {} PRs (depth: {})",
                entries.len(),
                depth.label()
            ),
        );

        let model = self.config.review_model.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.review_memory_bootstrap_bg = Some(rx);
        let thread_workdir = workdir.clone();
        std::thread::spawn(move || {
            run_review_memory_bootstrap(thread_workdir, memory_path, entries, model, tx);
        });

        self.mode = AppMode::ReviewMemoryBootstrapRunning(BootstrapRunState {
            origin,
            depth,
            stage: BootstrapStage::FetchingComments,
        });
    }

    /// Poll the background bootstrap. Progress messages update the running
    /// screen's stage; `Done` always surfaces a toast (success) or error (the
    /// run has a real side effect — tokens spent, findings written — even if
    /// the user already navigated away), and restores the PR picker only if
    /// the running screen is still showing. An error is also written onto the
    /// restored picker's own inline `error` field so it's visible immediately
    /// on return, not just logged. Returns `true` when a redraw is warranted.
    pub fn poll_review_memory_bootstrap_bg(&mut self) -> bool {
        let Some(rx) = self.review_memory_bootstrap_bg.as_ref() else {
            return false;
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(BootstrapProgress::Distilling {
                    pr_count,
                    token_estimate,
                }) => {
                    if let AppMode::ReviewMemoryBootstrapRunning(state) = &mut self.mode {
                        state.stage = BootstrapStage::Distilling {
                            pr_count,
                            token_estimate,
                        };
                    }
                    changed = true;
                }
                Ok(BootstrapProgress::Done(result)) => {
                    self.review_memory_bootstrap_bg = None;
                    // Capture the origin before any mode-mutating side effect
                    // below: `show_error` unconditionally resets `self.mode` to
                    // `Normal` for any non-Normal/Help/Viewing mode, which would
                    // otherwise clobber the running screen's stashed picker
                    // before we get a chance to restore it.
                    let mut origin = match &self.mode {
                        AppMode::ReviewMemoryBootstrapRunning(state) => Some(state.origin.clone()),
                        _ => None,
                    };
                    match result {
                        Ok(outcome) => {
                            self.push_toast_success(format!(
                                "Bootstrapped review memory from {} PR{} · {} new finding{}",
                                outcome.pr_count,
                                if outcome.pr_count == 1 { "" } else { "s" },
                                outcome.appended,
                                if outcome.appended == 1 { "" } else { "s" },
                            ));
                        }
                        Err(e) => {
                            // `show_error` only logs and surfaces via the
                            // dashboard's status bar, which the PR picker's
                            // full-screen render doesn't draw — also set the
                            // picker's own inline `error` (the same field
                            // `pr_picker_choose` uses) so the failure is
                            // actually visible on return, not just logged.
                            let detail = e.to_string();
                            if let Some(origin) = &mut origin {
                                origin.error = Some(detail.clone());
                            }
                            self.show_error(e);
                        }
                    }
                    if let Some(origin) = origin {
                        self.mode = AppMode::PrPicker(origin);
                    }
                    changed = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.review_memory_bootstrap_bg = None;
                    if let AppMode::ReviewMemoryBootstrapRunning(state) = &self.mode {
                        self.mode = AppMode::PrPicker(state.origin.clone());
                        self.message = Some("Bootstrap failed unexpectedly".to_string());
                        changed = true;
                    }
                    break;
                }
            }
        }
        changed
    }

    /// Cancel the running screen (`esc`/`q`): return to the PR picker. The
    /// background thread isn't aborted — if it finishes later,
    /// [`App::poll_review_memory_bootstrap_bg`] still surfaces the result.
    pub fn cancel_review_memory_bootstrap(&mut self) {
        if let AppMode::ReviewMemoryBootstrapRunning(state) = &self.mode {
            self.mode = AppMode::PrPicker(state.origin.clone());
        }
    }

    /// Open the review-memory compact confirm overlay (`c` in the PR picker):
    /// a synchronous local file read to show how many findings are currently
    /// in the doc before spending an agent pass on them (Epic E "prevent
    /// review-memory rot"). A no-op with a message if the doc is missing or
    /// empty — nothing to compact.
    pub fn open_review_memory_compact_confirm(&mut self) {
        let workdir = match &self.mode {
            AppMode::PrPicker(state) => state.workdir.clone(),
            _ => return,
        };
        let repo = self.repo_for_project_path(&workdir);
        let path = review_memory::review_memory_path(
            &repo,
            self.configured_review_memory_path(&repo).as_deref(),
        );
        let existing_findings = std::fs::read_to_string(&path)
            .map(|contents| review_memory::count_findings(&contents))
            .unwrap_or(0);
        if existing_findings == 0 {
            self.message = Some("Review memory doc is empty — nothing to compact".into());
            return;
        }
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.compact_confirm = Some(CompactConfirmState { existing_findings });
        }
    }

    /// Whether the compact confirm overlay is currently open over the picker.
    pub fn review_memory_compact_confirming(&self) -> bool {
        matches!(&self.mode, AppMode::PrPicker(state) if state.compact_confirm.is_some())
    }

    /// Close the overlay without running anything, staying on the PR picker.
    pub fn review_memory_compact_confirm_cancel(&mut self) {
        if let AppMode::PrPicker(state) = &mut self.mode {
            state.compact_confirm = None;
        }
    }

    /// Confirm the overlay: hand the doc read + one agent pass to a
    /// background thread and switch to the full-screen running view.
    pub fn review_memory_compact_confirm_run(&mut self) {
        let (workdir, mut origin) = match &self.mode {
            AppMode::PrPicker(state) if state.compact_confirm.is_some() => {
                (state.workdir.clone(), state.clone())
            }
            _ => return,
        };
        origin.compact_confirm = None;

        let repo = self.repo_for_project_path(&workdir);
        let memory_path = review_memory::review_memory_path(
            &repo,
            self.configured_review_memory_path(&repo).as_deref(),
        );

        self.log_info("pr_review", "compacting review memory doc".to_string());

        let model = self.config.review_model.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        self.review_memory_compact_bg = Some(rx);
        let thread_workdir = workdir.clone();
        let thread_memory_path = memory_path.clone();
        std::thread::spawn(move || {
            run_review_memory_compact(thread_workdir, thread_memory_path, model, tx);
        });

        let run_state = CompactRunState {
            origin,
            path: memory_path,
            stage: CompactStage::ReadingDoc,
        };
        self.review_memory_compact_pending = Some(run_state.clone());
        self.mode = AppMode::ReviewMemoryCompactRunning(run_state);
    }

    /// Poll the background compact pass. `Compacting` updates the running
    /// screen's token estimate; `Done` transitions to the full-screen review
    /// dialog on a successful rewrite (nothing is written yet), surfaces a
    /// message and returns to the picker when there was nothing to compact,
    /// or restores the picker with an inline error on failure — same
    /// restore-before-`show_error` ordering as
    /// [`App::poll_review_memory_bootstrap_bg`], for the same reason.
    pub fn poll_review_memory_compact_bg(&mut self) -> bool {
        let Some(rx) = self.review_memory_compact_bg.as_ref() else {
            return false;
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(CompactProgress::Compacting { token_estimate }) => {
                    if let AppMode::ReviewMemoryCompactRunning(state) = &mut self.mode {
                        state.stage = CompactStage::Compacting { token_estimate };
                    }
                    changed = true;
                }
                Ok(CompactProgress::Done(result)) => {
                    self.review_memory_compact_bg = None;
                    let Some(pending) = self.review_memory_compact_pending.take() else {
                        // Invariant: always set alongside `review_memory_compact_bg`
                        // in `review_memory_compact_confirm_run`. If it's ever
                        // missing there's nowhere safe to land the proposal.
                        changed = true;
                        break;
                    };
                    // Only auto-open the review dialog (or bounce a `None`/error
                    // back to the picker) if the user is still on the running
                    // screen. If they cancelled (`esc`) to the picker — or
                    // navigated anywhere else — nothing was written (unlike the
                    // bootstrap, which writes as it goes), so there's nowhere
                    // live to land a full-screen editable proposal without
                    // yanking the user out of whatever they're doing now; just
                    // surface that it finished.
                    let still_watching =
                        matches!(&self.mode, AppMode::ReviewMemoryCompactRunning(_));
                    match result {
                        Ok(Some(outcome)) => {
                            if still_watching {
                                self.mode =
                                    AppMode::ReviewMemoryCompactReview(CompactReviewState {
                                        origin: pending.origin,
                                        path: pending.path,
                                        original_findings: outcome.original_findings,
                                        proposed_findings: outcome.proposed_findings,
                                        editor: TextEditor::new(outcome.proposed_content),
                                        editing: false,
                                        scroll: 0,
                                        sync_to_cursor: false,
                                        error: None,
                                    });
                            } else {
                                self.push_toast_info(
                                    "Review memory compact finished after you navigated away \
                                     — press c to re-run and review it"
                                        .to_string(),
                                );
                            }
                        }
                        Ok(None) => {
                            self.message =
                                Some("Review memory doc is empty — nothing to compact".into());
                            if still_watching {
                                self.mode = AppMode::PrPicker(pending.origin);
                            }
                        }
                        Err(e) => {
                            if still_watching {
                                let mut origin = pending.origin;
                                origin.error = Some(e.to_string());
                                self.show_error(e);
                                self.mode = AppMode::PrPicker(origin);
                            } else {
                                self.show_error(e);
                            }
                        }
                    }
                    changed = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.review_memory_compact_bg = None;
                    self.review_memory_compact_pending = None;
                    if let AppMode::ReviewMemoryCompactRunning(state) = &self.mode {
                        self.mode = AppMode::PrPicker(state.origin.clone());
                        self.message = Some("Compact failed unexpectedly".to_string());
                        changed = true;
                    }
                    break;
                }
            }
        }
        changed
    }

    /// Cancel the running screen (`esc`/`q`): return to the PR picker. The
    /// background thread isn't aborted — if it finishes later,
    /// [`App::poll_review_memory_compact_bg`] still notices (it doesn't
    /// auto-open the review dialog once the user isn't watching the running
    /// screen anymore, since nothing was written to land it against).
    pub fn cancel_review_memory_compact(&mut self) {
        if let AppMode::ReviewMemoryCompactRunning(state) = &self.mode {
            self.mode = AppMode::PrPicker(state.origin.clone());
        }
    }

    /// Enter edit mode so keystrokes flow to the proposed-doc editor.
    pub fn pr_review_compact_review_edit(&mut self) {
        if let AppMode::ReviewMemoryCompactReview(state) = &mut self.mode {
            state.editing = true;
        }
    }

    /// Leave edit mode, returning to the confirm view (the text is kept).
    pub fn pr_review_compact_review_stop_edit(&mut self) {
        if let AppMode::ReviewMemoryCompactReview(state) = &mut self.mode {
            state.editing = false;
        }
    }

    /// Whether the compact review dialog is in edit mode. `None` when the
    /// dialog isn't open.
    pub fn pr_review_compact_review_editing(&self) -> Option<bool> {
        match &self.mode {
            AppMode::ReviewMemoryCompactReview(state) => Some(state.editing),
            _ => None,
        }
    }

    /// Forward a key to the proposed-doc editor (only meaningful in edit mode).
    pub fn pr_review_compact_review_editor_key(&mut self, key: crossterm::event::KeyEvent) {
        if let AppMode::ReviewMemoryCompactReview(state) = &mut self.mode
            && state.editing
        {
            state.editor.handle_key(key);
            state.sync_to_cursor = true;
        }
    }

    /// Scroll the proposed-doc view (confirm view only).
    pub fn pr_review_compact_review_scroll(&mut self, delta: isize) {
        if let AppMode::ReviewMemoryCompactReview(state) = &mut self.mode {
            state.scroll = state.scroll.saturating_add_signed(delta);
            state.sync_to_cursor = false;
        }
    }

    /// Write the (possibly edited) proposed replacement to the review-memory
    /// doc and return to the PR picker. This is the one place the compact
    /// flow writes anything — the background pass only ever produces a
    /// proposal (see [`run_review_memory_compact`]). A write failure keeps
    /// the dialog open with the error shown inline, same as
    /// [`App::pr_review_post_ai_review`]'s recoverable-error handling.
    pub fn pr_review_compact_write(&mut self) -> Result<()> {
        let AppMode::ReviewMemoryCompactReview(state) = &mut self.mode else {
            return Ok(());
        };
        let content = state.editor.text().to_string();
        match std::fs::write(&state.path, &content) {
            Ok(()) => {
                let (original, proposed) = (state.original_findings, state.proposed_findings);
                let origin = state.origin.clone();
                self.mode = AppMode::PrPicker(origin);
                self.push_toast_success(format!(
                    "Review memory compacted · {original} \u{2192} {proposed} findings"
                ));
            }
            Err(e) => {
                state.error = Some(e.to_string());
            }
        }
        Ok(())
    }

    /// Discard the proposed replacement without writing, returning to the PR
    /// picker.
    pub fn pr_review_compact_discard(&mut self) {
        if let AppMode::ReviewMemoryCompactReview(state) = &self.mode {
            self.mode = AppMode::PrPicker(state.origin.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::GhUser;

    fn pr() -> PrRef {
        PrRef {
            number: 1,
            head_sha: "sha".into(),
            url: "https://github.com/o/r/pull/1".into(),
            owner: "o".into(),
            repo: "r".into(),
            head_ref: "main".into(),
        }
    }

    fn user(login: &str, kind: &str) -> GhUser {
        GhUser {
            login: login.into(),
            kind: kind.into(),
        }
    }

    fn sample_comment(id: u64, author: &str, is_bot: bool) -> PrComment {
        PrComment {
            id,
            kind: CommentKind::Inline,
            author: author.to_string(),
            is_bot,
            path: Some("src/lib.rs".to_string()),
            line: Some(10),
            side: None,
            outdated: false,
            file_level: false,
            diff_hunk: None,
            body: "example".to_string(),
            snippet: "example".to_string(),
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::default(),
            local_note: None,
            github_id: None,
            github_review_id: None,
        }
    }

    #[test]
    fn strips_details_comments_and_images() {
        let body = "Real point here.\n\n<details>\n<summary>Prompt for AI agents</summary>\n\
            lots of tokens\n</details>\n<!-- internal note -->\n![badge](http://x/y.png)";
        let out = strip_bot_boilerplate(body);
        assert_eq!(out, "Real point here.");
    }

    #[test]
    fn strips_quoted_diff_fence() {
        let body = "This can race with the poller.\n\n```diff\n@@ -40,6 +40,7 @@\n-old\n+new\n```\n\nGuard it behind the lock.";
        let out = strip_bot_boilerplate(body);
        assert_eq!(
            out,
            "This can race with the poller.\n\nGuard it behind the lock."
        );
    }

    #[test]
    fn strips_quoted_suggestion_fence() {
        let body = "Consider this:\n\n```suggestion\nlet x = 1;\n```\n\nSaves a line.";
        let out = strip_bot_boilerplate(body);
        assert_eq!(out, "Consider this:\n\nSaves a line.");
    }

    #[test]
    fn strips_leading_quoted_diff_lines() {
        let body = "> -old line\n> +new line\n\nActual comment text.";
        let out = strip_bot_boilerplate(body);
        assert_eq!(out, "Actual comment text.");
    }

    #[test]
    fn leaves_non_diff_fences_untouched() {
        let body = "Use this instead:\n\n```rust\nlet x = 1;\n```\n\nCleaner.";
        let out = strip_bot_boilerplate(body);
        assert_eq!(out, body);
    }

    /// Init a throwaway git repo at `dir`, writing `contents` for `rel_path`
    /// across one commit per entry in `contents` (so later entries are more
    /// recent history). Returns the short sha of each commit, oldest first.
    fn git_repo_with_history(dir: &Path, rel_path: &str, contents: &[&str]) -> Vec<String> {
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git command");
            assert!(
                output.status.success(),
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        let file = dir.join(rel_path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let mut shas = Vec::new();
        for (i, body) in contents.iter().enumerate() {
            std::fs::write(&file, body).unwrap();
            git(&["add", "."]);
            git(&["commit", "-q", "-m", &format!("commit {i}")]);
            shas.push(git(&["rev-parse", "--short", "HEAD"]));
        }
        shas
    }

    #[test]
    fn commit_touching_line_finds_the_commit_that_last_changed_it() {
        let repo = tempfile::TempDir::new().unwrap();
        let shas = git_repo_with_history(
            repo.path(),
            "src/file.rs",
            &["line1\nline2\nline3\n", "line1\nCHANGED\nline3\n"],
        );

        // Line 2 was touched by the second commit only.
        assert_eq!(
            commit_touching_line(repo.path(), "src/file.rs", 2),
            Some(shas[1].clone())
        );
        // Line 1 has never changed since the first commit.
        assert_eq!(
            commit_touching_line(repo.path(), "src/file.rs", 1),
            Some(shas[0].clone())
        );
    }

    #[test]
    fn commit_touching_line_is_none_for_a_line_outside_the_file() {
        let repo = tempfile::TempDir::new().unwrap();
        git_repo_with_history(repo.path(), "src/file.rs", &["one line\n"]);

        assert_eq!(commit_touching_line(repo.path(), "src/file.rs", 999), None);
    }

    #[test]
    fn commit_touching_file_returns_the_latest_commit_on_that_path() {
        let repo = tempfile::TempDir::new().unwrap();
        let shas = git_repo_with_history(repo.path(), "src/file.rs", &["a\n", "b\n", "c\n"]);

        assert_eq!(
            commit_touching_file(repo.path(), "src/file.rs"),
            Some(shas[2].clone())
        );
    }

    #[test]
    fn commit_touching_file_is_none_for_an_untracked_path() {
        let repo = tempfile::TempDir::new().unwrap();
        git_repo_with_history(repo.path(), "src/file.rs", &["a\n"]);

        assert_eq!(commit_touching_file(repo.path(), "src/other.rs"), None);
    }

    #[test]
    fn commit_for_done_reply_prefers_line_history_when_the_line_is_current() {
        let repo = tempfile::TempDir::new().unwrap();
        let shas = git_repo_with_history(
            repo.path(),
            "src/file.rs",
            &["line1\nline2\n", "line1\nfixed\n"],
        );
        let mut comment = inline_comment("needs a fix", false);
        comment.path = Some("src/file.rs".into());
        comment.line = Some(2);
        comment.outdated = false;

        assert_eq!(
            commit_for_done_reply(repo.path(), &comment),
            (Some(shas[1].clone()), true)
        );
    }

    #[test]
    fn commit_for_done_reply_skips_line_search_for_an_outdated_anchor() {
        let repo = tempfile::TempDir::new().unwrap();
        // The comment's remembered line (2) hasn't changed since the first
        // commit; only the file as a whole was touched again afterward.
        let shas = git_repo_with_history(
            repo.path(),
            "src/file.rs",
            &["line1\nline2\n", "line1\nline2\nline3\n"],
        );
        let mut comment = inline_comment("stale anchor", false);
        comment.path = Some("src/file.rs".into());
        comment.line = Some(2);
        comment.outdated = true;

        // Falls straight to file history (the most recent commit) rather
        // than trusting the outdated line number.
        assert_eq!(
            commit_for_done_reply(repo.path(), &comment),
            (Some(shas[1].clone()), true)
        );
    }

    #[test]
    fn commit_for_done_reply_falls_back_to_head_with_a_caveat() {
        let repo = tempfile::TempDir::new().unwrap();
        let shas = git_repo_with_history(repo.path(), "src/other.rs", &["x\n"]);
        let mut comment = inline_comment("unrelated file", false);
        comment.path = Some("src/not-tracked.rs".into());
        comment.line = Some(1);
        comment.outdated = false;

        // Neither line nor file history exists for this path; falls back to
        // bare HEAD, flagged as an unconfident match.
        assert_eq!(
            commit_for_done_reply(repo.path(), &comment),
            (Some(shas[0].clone()), false)
        );
    }

    #[test]
    fn commit_for_done_reply_none_outside_a_git_repo() {
        let dir = tempfile::TempDir::new().unwrap();
        let comment = inline_comment("no repo here", false);

        assert_eq!(commit_for_done_reply(dir.path(), &comment), (None, false));
    }

    #[test]
    fn snippet_truncates_with_ellipsis() {
        let long = "x".repeat(200);
        let s = truncate_chars(&long, SNIPPET_LEN);
        assert_eq!(s.chars().count(), SNIPPET_LEN);
        assert!(s.ends_with('…'));
    }

    #[test]
    fn snippet_skips_leading_blank_lines() {
        assert_eq!(make_snippet("\n\n  hello world  \n", false), "hello world");
    }

    #[test]
    fn normalize_attaches_resolution_and_outdated() {
        let comments = vec![
            ReviewComment {
                id: 11,
                path: Some("a.rs".into()),
                line: None, // outdated
                original_line: Some(7),
                side: Some("RIGHT".into()),
                diff_hunk: Some("@@".into()),
                subject_type: None,
                body: "race condition".into(),
                user: user("alice", "User"),
                in_reply_to_id: None,
                pull_request_review_id: Some(99),
            },
            ReviewComment {
                id: 12,
                path: Some("b.rs".into()),
                line: Some(3),
                original_line: Some(3),
                side: Some("RIGHT".into()),
                diff_hunk: None,
                subject_type: None,
                body: "nit".into(),
                user: user("coderabbitai", "Bot"),
                in_reply_to_id: None,
                pull_request_review_id: None,
            },
        ];
        let threads = vec![ReviewThread {
            id: "T1".into(),
            is_resolved: true,
            comment_ids: vec![11],
        }];

        let review = normalize(pr(), comments, vec![], vec![], threads);
        assert_eq!(review.comments.len(), 2);

        let c11 = &review.comments[0];
        assert_eq!(c11.line, Some(7)); // fell back to original_line
        assert!(c11.outdated);
        assert!(c11.is_resolved);
        assert_eq!(c11.thread_id.as_deref(), Some("T1"));
        assert!(!c11.is_bot);

        let c12 = &review.comments[1];
        assert!(!c12.outdated);
        assert!(!c12.is_resolved);
        assert!(c12.is_bot);

        assert_eq!(review.open_count(), 1); // only c12 is unresolved
    }

    #[test]
    fn normalize_marks_file_level_comments_and_not_as_outdated() {
        // A file-level comment has no `line` by definition — that must not be
        // mistaken for an outdated line comment.
        let comments = vec![ReviewComment {
            id: 21,
            path: Some("src/big.rs".into()),
            line: None,
            original_line: None,
            side: None,
            diff_hunk: Some("@@ -1,400 +1,420 @@\n+ enormous".into()),
            subject_type: Some("file".into()),
            body: "This module does too much.".into(),
            user: user("alice", "User"),
            in_reply_to_id: None,
            pull_request_review_id: Some(99),
        }];

        let review = normalize(pr(), comments, vec![], vec![], vec![]);
        let c = &review.comments[0];
        assert!(c.file_level);
        assert!(!c.outdated);
        assert_eq!(c.prompt_hunk(), None);
    }

    #[test]
    fn normalize_drops_empty_review_summaries() {
        let reviews = vec![
            Review {
                id: 1,
                body: "".into(),
                state: "APPROVED".into(),
                user: user("bob", "User"),
            },
            Review {
                id: 2,
                body: "Please add a test.".into(),
                state: "CHANGES_REQUESTED".into(),
                user: user("bob", "User"),
            },
        ];
        let review = normalize(pr(), vec![], reviews, vec![], vec![]);
        assert_eq!(review.comments.len(), 1);
        assert_eq!(
            review.comments[0].kind,
            CommentKind::ReviewSummary {
                state: "CHANGES_REQUESTED".into()
            }
        );
    }

    fn inline_comment(body: &str, is_bot: bool) -> PrComment {
        PrComment {
            id: 1,
            kind: CommentKind::Inline,
            author: "alice".into(),
            is_bot,
            path: Some("src/app/sync.rs".into()),
            line: Some(42),
            side: Some("RIGHT".into()),
            outdated: false,
            file_level: false,
            diff_hunk: Some("@@ -38,4 +38,5 @@\n  poll = 250;\n+ self.sync();".into()),
            body: body.into(),
            snippet: String::new(),
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::Untriaged,
            local_note: None,
            github_id: None,
            github_review_id: None,
        }
    }

    #[test]
    fn fix_prompt_includes_file_line_comment_and_hunk() {
        let c = inline_comment("Guard this behind the lock.", false);
        let prompt = c.fix_prompt();
        assert!(prompt.starts_with("Address this PR review comment."));
        assert!(prompt.contains("File: src/app/sync.rs:42"));
        assert!(prompt.contains("Comment (@alice): Guard this behind the lock."));
        assert!(prompt.contains("Diff hunk:"));
        assert!(prompt.contains("+ self.sync();"));
        // No file contents are ever injected — only the comment + hunk.
        assert!(!prompt.contains("fn "));
    }

    #[test]
    fn fix_prompt_omits_hunk_for_file_level_comment() {
        let mut c = inline_comment("Split this module up.", false);
        c.file_level = true;
        c.line = None;
        // GitHub hands a file-level comment the entire file diff as its hunk.
        c.diff_hunk = Some("@@ -1,400 +1,420 @@\n+ a\n+ b".into());

        assert_eq!(c.prompt_hunk(), None);
        assert!(c.hunk_suppressed());

        let prompt = c.fix_prompt();
        assert!(prompt.contains("File: src/app/sync.rs  (comment on the whole file)"));
        assert!(!prompt.contains("Diff hunk:"));
        assert!(!prompt.contains("+ a"));
        // The agent is told the hunk was withheld, not that there was none.
        assert!(prompt.contains("Diff hunk omitted"));
    }

    #[test]
    fn whole_file_sized_hunk_is_dropped_but_ordinary_ones_are_kept() {
        let hunk_of = |n: usize| {
            Some(
                std::iter::repeat_n("+ line", n)
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        };

        // Real line comments run to ~90 hunk lines; those keep their context.
        let mut c = inline_comment("This block is wrong.", false);
        c.diff_hunk = hunk_of(93);
        assert!(c.prompt_hunk().is_some());
        assert!(!c.hunk_suppressed());
        assert!(c.fix_prompt().contains("Diff hunk:"));

        // Only a pathological, whole-file-sized hunk trips the backstop.
        c.diff_hunk = hunk_of(WHOLE_FILE_HUNK_LINES + 1);
        assert_eq!(c.prompt_hunk(), None);
        let prompt = c.fix_prompt();
        // Still line-anchored, so the pointer keeps its line — only the wall of
        // diff is dropped, and not as a "whole file" comment.
        assert!(prompt.contains("File: src/app/sync.rs:42"));
        assert!(!prompt.contains("(comment on the whole file)"));
        assert!(!prompt.contains("Diff hunk:"));
        assert!(prompt.contains("Diff hunk omitted"));
    }

    #[test]
    fn line_comment_windows_githubs_large_hunk_around_its_anchor() {
        let mut c = inline_comment("Only this line is relevant.", false);
        c.line = Some(20);
        c.outdated = true;
        c.diff_hunk = Some(
            std::iter::once("@@ -1,0 +1,40 @@".to_string())
                .chain((1..=40).map(|i| format!("+line{i}")))
                .collect::<Vec<_>>()
                .join("\n"),
        );

        let hunk = c.prompt_hunk().expect("the anchor is in the hunk");
        let lines: Vec<_> = hunk.lines().collect();
        assert_eq!(lines.len(), COMMENT_HUNK_CONTEXT_LINES * 2 + 2);
        assert!(hunk.contains("+line20"));
        assert!(!hunk.contains("+line1\n"));
        assert!(!hunk.contains("+line40"));
    }

    #[test]
    fn combined_fix_prompt_drops_whole_file_hunks() {
        let ordinary = inline_comment("Guard this behind the lock.", false);
        let mut file_level = inline_comment("Split this module up.", false);
        file_level.path = Some("src/big.rs".into());
        file_level.file_level = true;
        file_level.line = None;
        file_level.diff_hunk = Some("@@ -1,400 +1,420 @@\n+ enormous".into());

        let prompt = combined_fix_prompt(&[&ordinary, &file_level]);

        // The line-anchored comment keeps its (small) hunk...
        assert!(prompt.contains("+ self.sync();"));
        // ...while the whole-file hunk never lands in the shared prompt, where
        // several of them would otherwise compound.
        assert!(!prompt.contains("+ enormous"));
        assert!(prompt.contains("File: src/big.rs  (comment on the whole file)"));
    }

    #[test]
    fn fix_prompt_strips_bot_boilerplate() {
        let c = inline_comment("<details>noise</details>Real point.", true);
        let prompt = c.fix_prompt();
        assert!(prompt.contains("Comment (@alice): Real point."));
        assert!(!prompt.contains("<details>"));
    }

    #[test]
    fn combined_fix_prompt_numbers_comments_under_one_preamble() {
        let mut a = inline_comment("Guard this behind the lock.", false);
        a.path = Some("src/a.rs".into());
        a.line = Some(10);
        let mut b = inline_comment("Rename this field.", false);
        b.path = Some("src/b.rs".into());
        b.line = Some(20);

        let prompt = combined_fix_prompt(&[&a, &b]);

        // One shared preamble, not repeated per comment.
        assert!(prompt.starts_with("Address these PR review comments."));
        assert!(!prompt.contains("Address this PR review comment."));
        assert_eq!(prompt.matches("Address these").count(), 1);

        // Each comment appears as a numbered entry with its own file:line + text.
        assert!(prompt.contains("Comment 1:"));
        assert!(prompt.contains("Comment 2:"));
        assert!(prompt.contains("File: src/a.rs:10"));
        assert!(prompt.contains("Guard this behind the lock."));
        assert!(prompt.contains("File: src/b.rs:20"));
        assert!(prompt.contains("Rename this field."));

        // Still no file contents — only the comment text + diff hunks.
        assert!(!prompt.contains("fn "));
    }

    #[test]
    fn reply_target_inline_uses_thread_root() {
        // A reply (in_reply_to set) targets the thread root, not its own id.
        let mut leaf = inline_comment("thanks", false);
        leaf.id = 55;
        leaf.in_reply_to = Some(40);
        assert_eq!(
            leaf.reply_target(),
            ReplyTarget::InlineThread {
                root_comment_id: 40
            }
        );

        // A root inline comment (no in_reply_to) replies to itself.
        let root = inline_comment("nit", false);
        assert_eq!(
            root.reply_target(),
            ReplyTarget::InlineThread { root_comment_id: 1 }
        );
    }

    #[test]
    fn reply_target_conversation_and_summary_post_issue_comment() {
        let mut conv = inline_comment("hi", false);
        conv.kind = CommentKind::Conversation;
        assert_eq!(conv.reply_target(), ReplyTarget::Conversation);

        let mut summary = inline_comment("changes", false);
        summary.kind = CommentKind::ReviewSummary {
            state: "CHANGES_REQUESTED".into(),
        };
        assert_eq!(summary.reply_target(), ReplyTarget::Conversation);
    }

    #[test]
    fn replies_in_finds_only_comments_targeting_this_ones_id() {
        let root = sample_comment(1, "alice", false);
        let mut reply = sample_comment(2, "bob", false);
        reply.in_reply_to = Some(1);
        let mut unrelated = sample_comment(3, "carol", false);
        unrelated.in_reply_to = Some(99);
        let all = vec![root.clone(), reply.clone(), unrelated];

        let found = root.replies_in(&all);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, 2);
        assert_eq!(found[0].author, "bob");

        // A comment with no replies finds none.
        assert!(reply.replies_in(&all).is_empty());
    }

    #[test]
    fn reply_posted_via_amf_detects_the_channel_disclosure_footer() {
        let mut reply = sample_comment(2, "amf-user", false);
        reply.body = format!("Done in `abc123`.\n\n{}", AMF_ATTRIBUTION_FOOTER);
        assert!(reply_posted_via_amf(&reply));

        // A reply posted through some other channel (a headless agent using
        // `gh` directly, a human on GitHub) has no such footer.
        reply.body = "Done in `abc123`.".to_string();
        assert!(!reply_posted_via_amf(&reply));
    }

    #[test]
    fn fix_prompt_marks_outdated_and_omits_missing_pieces() {
        let mut c = inline_comment("Still relevant?", false);
        c.outdated = true;
        c.diff_hunk = None;
        let prompt = c.fix_prompt();
        assert!(prompt.contains("(comment is on a line that has since changed)"));
        assert!(!prompt.contains("Diff hunk:"));

        // Conversation/summary comments have no path: no File line at all.
        c.path = None;
        c.line = None;
        c.outdated = false;
        let prompt = c.fix_prompt();
        assert!(!prompt.contains("File:"));
        assert!(prompt.contains("Comment (@alice): Still relevant?"));
    }

    #[test]
    fn estimate_tokens_rounds_up() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abc"), 1);
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("abcde"), 2);
    }

    #[test]
    fn triage_state_db_str_roundtrips() {
        for state in [
            TriageState::Untriaged,
            TriageState::Fixing,
            TriageState::Done,
            TriageState::Skipped,
            TriageState::Replied,
        ] {
            assert_eq!(TriageState::from_db_str(state.as_db_str()), state);
        }
        // Unknown tokens degrade to untriaged rather than erroring.
        assert_eq!(TriageState::from_db_str("garbage"), TriageState::Untriaged);
    }

    #[test]
    fn triage_state_labels_and_markers() {
        assert_eq!(TriageState::Untriaged.label(), None);
        assert_eq!(TriageState::Untriaged.marker(), ' ');
        assert_eq!(TriageState::Done.label(), Some("done"));
        assert_eq!(TriageState::Done.marker(), 'x');
        assert_eq!(TriageState::Skipped.marker(), '-');
        assert_eq!(TriageState::Fixing.marker(), '~');
    }

    #[test]
    fn fix_target_defaults_to_dedicated_and_has_tags() {
        assert_eq!(FixTarget::default(), FixTarget::DedicatedReview);
        assert_eq!(FixTarget::DedicatedReview.tag(), "dedicated");
        assert_eq!(FixTarget::ExistingLive.tag(), "live");
    }

    #[test]
    fn fix_target_pick_row_labels_existing_live_and_dedicated() {
        assert_eq!(
            FixTargetPickRow::ExistingLive(None).label(),
            "Existing live session"
        );
        assert_eq!(
            FixTargetPickRow::ExistingLive(Some("Claude 2".to_string())).label(),
            "Existing live session (Claude 2)"
        );
        assert_eq!(
            FixTargetPickRow::Dedicated(AgentKind::Claude).label(),
            "Dedicated triage session (Claude)"
        );
    }

    #[test]
    fn reply_kind_menu_labels_are_distinct() {
        assert_eq!(ReplyKind::ALL.len(), 2);
        assert_eq!(
            ReplyKind::Done.menu_label(),
            "Done — report a completed fix"
        );
        assert_eq!(
            ReplyKind::NotNeeded.menu_label(),
            "Not needed — explain why"
        );
    }

    #[test]
    fn mark_action_menu_label_reflects_current_state() {
        let mut comment = sample_comment(1, "alice", false);
        comment.triage = TriageState::Untriaged;
        comment.is_resolved = false;

        assert_eq!(MarkAction::Done.menu_label(Some(&comment)), "Done (local)");
        assert_eq!(MarkAction::Skip.menu_label(Some(&comment)), "Skip (local)");
        assert_eq!(
            MarkAction::ResolveOnGitHub.menu_label(Some(&comment)),
            "Resolve thread on GitHub"
        );

        comment.triage = TriageState::Done;
        assert_eq!(
            MarkAction::Done.menu_label(Some(&comment)),
            "Done (local) — press to clear"
        );

        comment.triage = TriageState::Skipped;
        assert_eq!(
            MarkAction::Skip.menu_label(Some(&comment)),
            "Skip (local) — press to clear"
        );

        comment.is_resolved = true;
        assert_eq!(
            MarkAction::ResolveOnGitHub.menu_label(Some(&comment)),
            "Reopen thread on GitHub (currently resolved)"
        );

        // No selection: falls back to the untoggled label rather than panicking.
        assert_eq!(MarkAction::Done.menu_label(None), "Done (local)");
    }

    #[test]
    fn fix_session_index_prefers_dedicated_else_creates() {
        use crate::project::{AgentKind, Feature, SessionKind, VibeMode};
        let mut feature = Feature::new(
            "feat".into(),
            "branch".into(),
            std::path::PathBuf::from("/tmp/wd"),
            false,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );

        // Nothing running yet: both strategies report "must create / nothing to
        // reuse".
        assert_eq!(
            pr_triage_session_index(&feature, FixTarget::DedicatedReview),
            None
        );
        assert_eq!(
            pr_triage_session_index(&feature, FixTarget::ExistingLive),
            None
        );

        // A regular live agent session satisfies existing-live but not dedicated.
        feature.add_session_named(SessionKind::Claude, "Claude".into());
        assert_eq!(
            pr_triage_session_index(&feature, FixTarget::ExistingLive),
            Some(0)
        );
        assert_eq!(
            pr_triage_session_index(&feature, FixTarget::DedicatedReview),
            None
        );

        // An already-running session created before the rename is still reused.
        feature.add_session_named(SessionKind::Claude, LEGACY_REVIEW_SESSION_LABEL.into());
        assert_eq!(
            pr_triage_session_index(&feature, FixTarget::DedicatedReview),
            Some(1)
        );

        // The current label wins when both exist, while existing-live still
        // resolves to the first agent session.
        feature.add_session_named(SessionKind::Claude, TRIAGE_SESSION_LABEL.into());
        assert_eq!(
            pr_triage_session_index(&feature, FixTarget::DedicatedReview),
            Some(2)
        );
        assert_eq!(
            pr_triage_session_index(&feature, FixTarget::ExistingLive),
            Some(0)
        );
    }

    #[test]
    fn fix_session_index_ignores_non_agent_sessions() {
        use crate::project::{AgentKind, Feature, SessionKind, VibeMode};
        let mut feature = Feature::new(
            "feat".into(),
            "branch".into(),
            std::path::PathBuf::from("/tmp/wd"),
            false,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Claude,
            false,
            false,
        );
        // A terminal window is not an agent harness, so it is never a fix target.
        feature.add_session_named(SessionKind::Terminal, "Terminal".into());
        assert_eq!(
            pr_triage_session_index(&feature, FixTarget::ExistingLive),
            None
        );
        assert_eq!(
            pr_triage_session_index(&feature, FixTarget::DedicatedReview),
            None
        );
    }

    #[test]
    fn agent_text_strips_only_for_bots() {
        let human = PrComment {
            id: 1,
            kind: CommentKind::Inline,
            author: "alice".into(),
            is_bot: false,
            path: None,
            line: None,
            side: None,
            outdated: false,
            file_level: false,
            github_id: None,
            github_review_id: None,
            diff_hunk: None,
            body: "<details>keep?</details>plain".into(),
            snippet: String::new(),
            in_reply_to: None,
            thread_id: None,
            is_resolved: false,
            triage: TriageState::Untriaged,
            local_note: None,
        };
        let mut bot = human.clone();
        bot.is_bot = true;
        assert!(human.agent_text().contains("<details>"));
        assert_eq!(bot.agent_text(), "plain");
    }

    fn review_comment(id: u64, path: Option<&str>, line: Option<u32>, body: &str) -> ReviewComment {
        bot_review_comment(id, path, line, body, false)
    }

    fn bot_review_comment(
        id: u64,
        path: Option<&str>,
        line: Option<u32>,
        body: &str,
        is_bot: bool,
    ) -> ReviewComment {
        ReviewComment {
            id,
            path: path.map(String::from),
            line,
            original_line: line,
            side: Some("RIGHT".into()),
            diff_hunk: None,
            subject_type: None,
            body: body.to_string(),
            user: if is_bot {
                user("coderabbitai", "Bot")
            } else {
                user("alice", "User")
            },
            in_reply_to_id: None,
            pull_request_review_id: None,
        }
    }

    fn review(id: u64, body: &str, is_bot: bool) -> Review {
        Review {
            id,
            body: body.to_string(),
            state: "COMMENTED".into(),
            user: if is_bot {
                user("coderabbitai", "Bot")
            } else {
                user("alice", "User")
            },
        }
    }

    #[test]
    fn bootstrap_depth_default_is_fifty() {
        assert_eq!(BootstrapDepth::default(), BootstrapDepth::Fifty);
        assert_eq!(BootstrapDepth::Fifty.limit(), 50);
        assert_eq!(BootstrapDepth::Twenty.limit(), 20);
        assert_eq!(BootstrapDepth::Hundred.limit(), 100);
        assert!(BootstrapDepth::All.limit() > 100);
    }

    #[test]
    fn bootstrap_pr_text_includes_location_and_review_lines() {
        let comments = vec![review_comment(
            1,
            Some("src/app/sync.rs"),
            Some(42),
            "Guard this behind the lock.",
        )];
        let reviews = vec![review(2, "Looks solid overall.", false)];
        let text = bootstrap_pr_text(&comments, &reviews);
        assert_eq!(
            text,
            "- (src/app/sync.rs:42) Guard this behind the lock.\n- (review) Looks solid overall."
        );
    }

    #[test]
    fn bootstrap_pr_text_strips_bot_boilerplate_and_skips_empty() {
        let comments = vec![
            bot_review_comment(
                1,
                Some("a.rs"),
                None,
                "<details><summary>Prompt for AI agents</summary>noise</details>Real point.",
                true,
            ),
            review_comment(2, None, None, "   "),
        ];
        let text = bootstrap_pr_text(&comments, &[]);
        assert_eq!(text, "- (a.rs) Real point.");
    }

    #[test]
    fn bootstrap_prompt_lists_every_pr_and_instructs_category_format() {
        let bodies = vec![
            (
                1,
                "Fix race".to_string(),
                "- (a.rs:1) Guard the lock".to_string(),
            ),
            (
                2,
                "Add tests".to_string(),
                "- (review) Needs tests".to_string(),
            ),
        ];
        let prompt = bootstrap_prompt(&bodies);
        assert!(prompt.contains("## Category"));
        assert!(prompt.contains("### PR #1: Fix race"));
        assert!(prompt.contains("- (a.rs:1) Guard the lock"));
        assert!(prompt.contains("### PR #2: Add tests"));
        assert!(prompt.contains("- (review) Needs tests"));
    }

    #[test]
    fn append_amf_attribution_uses_a_distinct_channel_disclosure() {
        assert_eq!(
            append_amf_attribution("Done in `abc123`."),
            "Done in `abc123`.\n\n— posted via AMF"
        );
        assert_eq!(
            append_amf_attribution("Trailing newline.\n\n"),
            "Trailing newline.\n\n— posted via AMF"
        );
    }
}
