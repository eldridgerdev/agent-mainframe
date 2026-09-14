use crate::app::{
    DiffScope, DiffViewerFocus, DiffViewerLayout, ReviewDestinationPickState,
    TriageFeatureSetupState, ViewState,
};
use crate::editor::TextEditor;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Child;
use std::time::Instant;
/// Severity tag on a line comment or file rejection, conventional-comments
/// style. Drives three things: the GitHub review *event* (any `Blocker` →
/// `REQUEST_CHANGES`), the agent prompt's mandatory-vs-optional framing, and
/// the "blockers only" file filter. Defaults to `Suggestion` — a change worth
/// making that isn't blocking — so older progress files (which carried no
/// severity) deserialize to a sane middle ground.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    /// Must be addressed before merge.
    Blocker,
    /// Should change, but not blocking (the default).
    #[default]
    Suggestion,
    /// Minor / optional polish.
    Nit,
    /// A question for the author, not a demand.
    Question,
    /// Positive note; no action needed.
    Praise,
}

impl Severity {
    /// Cycle Blocker → Suggestion → Nit → Question → Praise → Blocker, for the
    /// editor's Ctrl+E toggle.
    pub fn next(self) -> Self {
        match self {
            Severity::Blocker => Severity::Suggestion,
            Severity::Suggestion => Severity::Nit,
            Severity::Nit => Severity::Question,
            Severity::Question => Severity::Praise,
            Severity::Praise => Severity::Blocker,
        }
    }

    /// The conventional-comments label — also the prefix rendered into the
    /// feedback file and the PR comment body.
    pub fn label(self) -> &'static str {
        match self {
            Severity::Blocker => "blocker",
            Severity::Suggestion => "suggestion",
            Severity::Nit => "nit",
            Severity::Question => "question",
            Severity::Praise => "praise",
        }
    }

    pub fn is_blocker(self) -> bool {
        matches!(self, Severity::Blocker)
    }
}

/// Per-file verdict in a final review. Absence of an entry means the file
/// was skipped (neither approved nor rejected).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewDecision {
    Approve,
    Reject {
        feedback: String,
        /// How blocking the rejection is. Defaulted (`Suggestion`) so older
        /// progress files load, and so an auto-rejection implied by line
        /// comments carries a neutral verdict severity — the real severities
        /// live on its line comments.
        #[serde(default)]
        severity: Severity,
    },
}

/// How many addressable lines of context are captured on each side of a
/// comment's anchor for re-location. Small enough to stay cheap and to tolerate
/// nearby edits, large enough to disambiguate repeated lines.
pub const ANCHOR_CONTEXT_RADIUS: usize = 2;

/// A snapshot of a commented line's text plus a few neighbours, captured when
/// the comment was anchored. Lets the re-anchor pass re-locate a comment when
/// the exact `DiffLineLocation` no longer exists after the diff is refreshed
/// (the agent edited the code, or the reviewer changed the base ref). Lines are
/// the diff-prefix-stripped `addressable_line_texts()`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentAnchorContext {
    /// Text of the anchored line itself.
    pub line: String,
    /// Up to `ANCHOR_CONTEXT_RADIUS` addressable-line texts immediately before,
    /// in ascending order (closest neighbour last).
    pub before: Vec<String>,
    /// Up to `ANCHOR_CONTEXT_RADIUS` addressable-line texts immediately after,
    /// in ascending order (closest neighbour first).
    pub after: Vec<String>,
}

impl CommentAnchorContext {
    /// Capture the context around `idx` in a file's `addressable_line_texts()`.
    /// Returns `None` if `idx` is out of range.
    pub fn capture(texts: &[String], idx: usize) -> Option<Self> {
        let line = texts.get(idx)?.clone();
        let before = texts[idx.saturating_sub(ANCHOR_CONTEXT_RADIUS)..idx].to_vec();
        let after = texts
            .get(idx + 1..(idx + 1 + ANCHOR_CONTEXT_RADIUS).min(texts.len()))
            .unwrap_or(&[])
            .to_vec();
        Some(Self {
            line,
            before,
            after,
        })
    }

    /// Best-effort re-location of this context within `texts`. Considers every
    /// index whose (trimmed) line text matches the anchor line, scoring each by
    /// how many trimmed neighbours also agree, and returns the single best
    /// candidate. A blank anchor line, no line match, or an ambiguous tie for
    /// the top score all yield `None` (the comment is then treated as lost —
    /// the conservative direction, never a silently wrong re-anchor).
    pub fn best_match(&self, texts: &[String]) -> Option<usize> {
        let target = self.line.trim();
        if target.is_empty() {
            return None;
        }
        let mut best: Option<(usize, usize)> = None; // (score, idx)
        let mut tied = false;
        for (idx, text) in texts.iter().enumerate() {
            if text.trim() != target {
                continue;
            }
            let score = self.neighbour_score(texts, idx);
            match best {
                Some((best_score, _)) if score < best_score => {}
                Some((best_score, _)) if score == best_score => tied = true,
                _ => {
                    best = Some((score, idx));
                    tied = false;
                }
            }
        }
        match best {
            Some((_, idx)) if !tied => Some(idx),
            _ => None,
        }
    }

    /// How many of the captured neighbours (trimmed) still surround `idx`.
    fn neighbour_score(&self, texts: &[String], idx: usize) -> usize {
        let mut score = 0;
        // `before` is ascending, so its last entry is the immediate predecessor.
        for (offset, want) in self.before.iter().rev().enumerate() {
            let Some(pos) = idx.checked_sub(offset + 1) else {
                break;
            };
            if texts.get(pos).map(|t| t.trim()) == Some(want.trim()) {
                score += 1;
            }
        }
        for (offset, want) in self.after.iter().enumerate() {
            if texts.get(idx + offset + 1).map(|t| t.trim()) == Some(want.trim()) {
                score += 1;
            }
        }
        score
    }
}

/// A reviewer comment anchored to a diff line (or a span of lines) during a
/// final review. `location` is the end anchor (GitHub's `line`); `start`, when
/// set, is the first line of a multi-line span (GitHub's `start_line`). A `None`
/// start is a single-line comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineComment {
    pub location: crate::diff::DiffLineLocation,
    /// Start of a multi-line span. `None` for a single-line comment. Defaulted
    /// so older single-line progress files deserialize unchanged.
    #[serde(default)]
    pub start: Option<crate::diff::DiffLineLocation>,
    pub text: String,
    /// True while this is an AI co-reviewer *draft* the human has not yet
    /// accepted. Draft comments render distinctly and are excluded from the
    /// finished feedback file / PR review until accepted. Defaulted so older
    /// progress files (all human comments) deserialize unchanged.
    #[serde(default)]
    pub draft: bool,
    /// A suggested replacement for the commented line/span (GitHub-style
    /// "suggestion"). `None` for a plain comment. Rendered as a fenced
    /// ```suggestion block in the feedback file and PR review, and fed to the
    /// agent as a verbatim patch. Defaulted so older progress files load.
    #[serde(default)]
    pub suggestion: Option<String>,
    /// Conventional-comments severity for this comment. Chosen in the comment
    /// editor (Ctrl+E cycles it); defaults to `Suggestion` so older progress
    /// files load unchanged.
    #[serde(default)]
    pub severity: Severity,
    /// Context snapshot around `location`, captured for re-anchoring after a
    /// diff refresh. `None` until the next progress persist captures it (and for
    /// older progress files, which simply can't be re-anchored).
    #[serde(default)]
    pub anchor_context: Option<CommentAnchorContext>,
    /// Context snapshot around `start` (range comments only). `None` for a
    /// single-line comment.
    #[serde(default)]
    pub start_anchor_context: Option<CommentAnchorContext>,
    /// Set by the re-anchor pass when the comment could not be re-located in a
    /// refreshed diff. Such a comment is surfaced as "anchor lost — possibly
    /// addressed" rather than silently dropped. Cleared whenever it resolves.
    #[serde(default)]
    pub anchor_lost: bool,
    /// Thread state: `true` once the reviewer has marked this conversation
    /// settled (`R` on the cursored comment). A resolved thread stays visible so
    /// it can be un-resolved, but is withheld from the feedback file, the PR
    /// review, the `Unresolved` filter and the auto-reject rule. Defaulted so
    /// older progress files load as open threads — the conservative direction.
    #[serde(default)]
    pub resolved: bool,
    /// `true` when this comment was carried in from a *previous* finished review
    /// round rather than authored in this session. Drives the "(unresolved from a
    /// previous round)" tag in the feedback file and keeps carried threads from
    /// making a fresh re-review read as work-in-progress. Defaulted so older
    /// progress files load as freshly-authored.
    #[serde(default)]
    pub carried: bool,
}

impl LineComment {
    /// Whether this comment spans more than one line.
    pub fn is_range(&self) -> bool {
        self.start.is_some()
    }

    /// An *open thread*: a comment the human kept (not an unadjudicated AI draft)
    /// and has not yet marked resolved. Open threads are what a review round
    /// actually sends to the agent, and what a re-review counts and filters on.
    pub fn is_open_thread(&self) -> bool {
        !self.draft && !self.resolved
    }

    /// The inclusive range of indices into a file's `addressable_lines()` that
    /// this comment covers, best-effort located by line number. `None` when the
    /// end anchor can no longer be found in the diff (e.g. after a refresh that
    /// dropped the line). For a single-line comment this is `idx..=idx`.
    pub fn covered_indices(
        &self,
        locs: &[crate::diff::DiffLineLocation],
    ) -> Option<std::ops::RangeInclusive<usize>> {
        let end = locs.iter().position(|l| *l == self.location)?;
        match self.start.and_then(|s| locs.iter().position(|l| *l == s)) {
            Some(start) => Some(start.min(end)..=start.max(end)),
            None => Some(end..=end),
        }
    }
}

/// A reviewer comment anchored to a whole file, independent of that file's
/// approve/reject verdict. Unlike a line comment it never auto-rejects the
/// file: its severity communicates priority without changing the verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileComment {
    pub text: String,
    #[serde(default)]
    pub severity: Severity,
    #[serde(default)]
    pub resolved: bool,
    #[serde(default)]
    pub carried: bool,
}

impl FileComment {
    pub fn is_open_thread(&self) -> bool {
        !self.resolved
    }
}

/// Which files the review file-list shows. Lets a reviewer narrow a large
/// changeset to the work that still needs attention. Only meaningful in review
/// mode; the read-only viewer always behaves as `All`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileFilter {
    /// Every changed file (default).
    #[default]
    All,
    /// Files with no verdict yet.
    Undecided,
    /// Files marked as needing revision.
    Rejected,
    /// Files that carry a `Blocker`-severity rejection or line comment, so a
    /// reviewer can focus on the must-fix items in a large changeset.
    Blockers,
    /// Files carrying an open whole-file comment.
    FileComments,
    /// Files carrying at least one unresolved thread (a kept, non-draft line
    /// comment the reviewer hasn't settled). Empty when nothing is open, so the
    /// cycle skips it unless an open thread exists.
    Unresolved,
    /// Files whose diff changed since the last finished review round (the
    /// re-review loop). Empty on a first review, so the cycle skips it unless a
    /// prior snapshot exists.
    Changed,
}

impl FileFilter {
    /// Cycle All → Undecided → Rejected → Blockers → File comments →
    /// Unresolved → Changed → All.
    /// Steps with nothing to show are skipped by the caller (see
    /// `diff_review_cycle_file_filter`): `Changed` without a prior review
    /// snapshot, `Unresolved` without an open thread.
    pub fn next(self) -> Self {
        match self {
            FileFilter::All => FileFilter::Undecided,
            FileFilter::Undecided => FileFilter::Rejected,
            FileFilter::Rejected => FileFilter::Blockers,
            FileFilter::Blockers => FileFilter::FileComments,
            FileFilter::FileComments => FileFilter::Unresolved,
            FileFilter::Unresolved => FileFilter::Changed,
            FileFilter::Changed => FileFilter::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FileFilter::All => "all",
            FileFilter::Undecided => "undecided",
            FileFilter::Rejected => "rejected",
            FileFilter::Blockers => "blockers",
            FileFilter::FileComments => "file comments",
            FileFilter::Unresolved => "unresolved",
            FileFilter::Changed => "changed",
        }
    }
}

/// One rendered row of the changed-file tree: either a directory header or a
/// file beneath it. Produced by `DiffViewerState::file_tree_rows`, which is the
/// single source of truth for both the file-list rendering and the `j`/`k` row
/// cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileTreeRow {
    Dir {
        /// Full directory path from the repo root, e.g. `src/app`. Also the key
        /// used in `collapsed_dirs` / `tree_cursor_dir`.
        path: String,
        /// Just this level's segment, e.g. `app` — what the row displays.
        label: String,
        depth: usize,
        collapsed: bool,
        /// Visible files anywhere beneath this directory.
        files: usize,
    },
    File {
        /// Index into `DiffViewerState::files` — the selection everything else
        /// in the viewer is keyed by.
        index: usize,
        depth: usize,
        /// Basename only; the path's directories are shown by the rows above.
        name: String,
    },
}

/// Every ancestor directory of `path`, shallowest first (`src`, `src/app`, …).
/// Empty for a repo-root file.
pub fn ancestor_dirs(path: &str) -> Vec<String> {
    let Some(dir_end) = path.rfind('/') else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    for (i, _) in path[..dir_end].match_indices('/') {
        dirs.push(path[..i].to_string());
    }
    dirs.push(path[..dir_end].to_string());
    dirs
}

/// A reply the feature's agent wrote back under a review item in the previous
/// round. Parsed out of `.claude/final-review-feedback.md` on re-review (from the
/// `**Agent:**` blocks `REVIEW_FEEDBACK_PROMPT` asks the agent to append) and
/// surfaced beside the diff so the reviewer sees what the agent claimed to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentResponse {
    /// The item's anchor heading text, e.g. `src/foo.rs:42` or `src/foo.rs`.
    pub anchor: String,
    /// The agent's reply text (the `**Agent:**` block, marker stripped).
    pub response: String,
}

/// One finished final-review round loaded from the bounded live feedback log
/// (or, on demand, its archive). The markdown is kept intact so the history
/// browser can show everything the round recorded — verdict counts, comments,
/// suggestions, check output and agent replies — without inventing a second
/// persisted format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewHistoryRound {
    /// The `## Review — ...` heading text, used for the timeline's compact
    /// label. Falls back to `Review` for a malformed/legacy round.
    pub title: String,
    /// The complete self-contained round, including its heading.
    pub markdown: String,
    /// Number of unresolved comments explicitly carried into this round.
    pub carried_unresolved: usize,
}

/// Transient state for the read-only final-review timeline/history browser.
/// `rounds` is newest-first. It starts with only the bounded live feedback
/// file; older archived rounds are appended lazily when navigation reaches
/// past the loaded tail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewHistoryState {
    pub rounds: Vec<ReviewHistoryRound>,
    /// `0` is the live editable `Current` review; `1..` index `rounds`.
    pub selected: usize,
    pub scroll: usize,
    pub rendered_lines: usize,
    pub view_height: usize,
    pub archive_available: bool,
    pub archive_loaded: bool,
    pub error: Option<String>,
}

/// One row of the pre-finish summary list (`summary_items`): every verdict,
/// open comment and suggestion in the review, in file order. Built fresh from
/// `DiffViewerState` each time the modal is opened or navigated — nothing here
/// is persisted separately from the decisions/comments it's derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryItem {
    /// A file's verdict row (approved / needs work / skipped / no verdict).
    File { file_idx: usize },
    /// An open (kept, unresolved) line comment or suggestion, in the same
    /// order as the file's `line_comments` vec (already sorted by line).
    LineComment { file_idx: usize, comment_idx: usize },
    /// An open whole-file comment.
    FileComment { file_idx: usize },
    /// The overall (non-file) review feedback. Only present when non-empty.
    General,
}

pub struct DiffViewerState {
    pub from_view: ViewState,
    pub workdir: PathBuf,
    pub scope: DiffScope,
    pub branch: String,
    pub base_ref: String,
    pub base_commit: String,
    /// Reviewer-chosen base ref override. When set, the loader compares against
    /// this ref/commit instead of the auto-resolved base. Kept across refreshes.
    pub override_base_ref: Option<String>,
    /// True while the reviewer is typing a base ref in the prompt.
    pub editing_base_ref: bool,
    /// In-progress base-ref text for the prompt.
    pub base_ref_input: String,
    pub files: Vec<crate::diff::DiffFile>,
    pub selected_file: usize,
    pub patch_scroll: usize,
    pub focus: DiffViewerFocus,
    pub layout: DiffViewerLayout,
    pub error: Option<String>,
    /// When true the viewer is a final-review session: each file can be
    /// approved/rejected/skipped and feedback is collected on finish.
    pub review: bool,
    /// File path -> verdict. Skipped files have no entry.
    pub decisions: std::collections::HashMap<String, ReviewDecision>,
    /// Paths whose `Reject` entry in `decisions` was defaulted by storing a
    /// kept line comment (a commented file implicitly needs revision) rather
    /// than set explicitly. Removing the file's last kept comment clears an
    /// auto-set rejection; an explicit approve/skip/reject drops the path from
    /// this set so the reviewer's verdict sticks.
    pub auto_rejected: std::collections::HashSet<String>,
    /// File path -> line-level comments anchored to specific diff lines.
    pub line_comments: std::collections::HashMap<String, Vec<LineComment>>,
    /// File path -> verdict-free comment anchored to the whole file.
    pub file_comments: std::collections::HashMap<String, FileComment>,
    /// Active line-comment cursor: index into the current file's
    /// `addressable_lines()`. `None` when the line cursor is inactive.
    pub comment_cursor: Option<usize>,
    /// Selection anchor for an in-progress multi-line comment: the index into
    /// `addressable_lines()` where the reviewer started the range. The selected
    /// span is `min(anchor, cursor)..=max(anchor, cursor)`. `None` selects only
    /// the cursor line.
    pub comment_anchor: Option<usize>,
    /// True while typing a comment for the cursored line (reuses
    /// `feedback_editor`).
    pub editing_line_comment: bool,
    /// True while editing the current file's verdict-free whole-file comment.
    pub editing_file_comment: bool,
    /// True while typing a *suggested replacement* for the cursored line/span
    /// (also reuses `feedback_editor`; mutually exclusive with
    /// `editing_line_comment`). The editor content is the replacement code.
    pub editing_suggestion: bool,
    /// Opt-in toggle: apply every still-open suggested change directly to the
    /// worktree before the build/test gate runs and the review finishes. Kept
    /// separate from suggestion authoring so finishing never mutates source
    /// files unless the reviewer explicitly enables it.
    pub apply_suggestions_on_finish: bool,
    /// Human-readable anchors of suggestions successfully applied during this
    /// review (either individually or by the finish-time batch). Carried until
    /// finish so the summary can say exactly what AMF changed locally.
    pub applied_suggestions: Vec<String>,
    /// Finish-time application failures (`anchor: reason`). The affected
    /// suggestions remain open and are sent to the fixing agent normally.
    pub suggestion_apply_failures: Vec<String>,
    /// Severity being composed in the line-comment or rejection editor. Seeded
    /// when the editor opens (from an existing comment/rejection, else a sensible
    /// default) and cycled with Ctrl+E; read on submit. Transient — not
    /// persisted directly (the stored `LineComment` / `ReviewDecision` carries it).
    pub comment_severity: Severity,
    /// When true the next draw scrolls the patch to keep the comment cursor
    /// visible, mirroring `feedback_sync_to_cursor`.
    pub cursor_sync_to_view: bool,
    /// True while a finish attempt is awaiting confirmation because some files
    /// still have no verdict (set by `confirm_or_finish_review`).
    pub finish_confirm: bool,
    /// True while the user is typing rejection feedback for the current file.
    pub feedback_editing: bool,
    /// True while the user is typing general (non-file) review feedback.
    pub editing_general: bool,
    /// Session-scoped keymap preference for every final-review comment,
    /// suggestion, rejection, and general-feedback editor. Fresh review
    /// sessions start plain; the preference is intentionally not persisted.
    pub vim_enabled: bool,
    /// Active editor, shared by the per-file rejection editor and the
    /// general-feedback editor (only one is open at a time). Vim-capable so
    /// reviewers can write multi-paragraph / list feedback.
    pub feedback_editor: TextEditor,
    /// Scroll offset (in wrapped visual lines) for the feedback editor.
    pub feedback_scroll: usize,
    /// When true, the next draw scrolls the feedback editor to keep the cursor
    /// visible.
    pub feedback_sync_to_cursor: bool,
    /// Overall review feedback not tied to a specific file.
    pub general_feedback: String,
    /// File path -> developer note parsed from `.claude/review-notes.md`
    /// (written by review mode). Shown beside the diff during final review.
    pub review_notes: std::collections::HashMap<String, String>,
    /// File path -> walkthrough generated on demand (via headless Claude) for a
    /// file with no developer note. Cached so it survives file switches.
    pub generated_notes: std::collections::HashMap<String, String>,
    /// In-flight headless process generating a walkthrough (one at a time).
    pub walkthrough_child: Option<crate::headless::LeasedChild>,
    /// Path the in-flight walkthrough is being generated for, so the result is
    /// filed correctly even if the reviewer navigates to another file.
    pub walkthrough_file: Option<String>,
    /// In-flight headless AI co-review pass (one at a time). Separate slot from
    /// the walkthrough so the two can't clobber each other.
    pub co_review_child: Option<crate::headless::LeasedChild>,
    /// Path the in-flight co-review is being generated for, so draft comments
    /// land on the right file even if the reviewer navigates away.
    pub co_review_file: Option<String>,
    /// In-flight *batched* co-review of an oversized file: a worker thread
    /// hunk-splits it, reviews each slice, and sends back the concatenated
    /// `<line>|<comment>` lines plus a count of slices that could not be
    /// reviewed (or an error message). Used instead of `co_review_child` when
    /// the single-file prompt would overflow.
    pub co_review_bg:
        Option<std::sync::mpsc::Receiver<std::result::Result<(String, usize), String>>>,
    /// Cached on-demand whole-changeset overview / risk summary (headless,
    /// reviewer-triggered — see `changeset_overview_open`). Kept until the
    /// reviewer explicitly regenerates it so reopening the modal is free.
    pub changeset_overview: Option<String>,
    /// In-flight headless process generating the changeset overview.
    pub changeset_overview_child: Option<crate::headless::LeasedChild>,
    /// True while the changeset-overview modal is shown. Independent of
    /// generation state so a cached overview can be reopened without
    /// re-running the headless pass.
    pub changeset_overview_open: bool,
    pub changeset_overview_scroll: usize,
    /// Rendered (markdown-wrapped) line count / viewport height of the modal at
    /// the last draw, mirroring `notes_rendered_lines` / `notes_view_height` so
    /// scroll clamps to the real visual bottom.
    pub changeset_overview_rendered_lines: usize,
    pub changeset_overview_view_height: usize,
    /// When true the developer-notes panel takes the full patch column.
    pub notes_expanded: bool,
    pub notes_scroll: usize,
    /// Rendered (markdown-wrapped) line count of the current note, recorded by
    /// the renderer each frame so scroll clamping uses real visual lines.
    pub notes_rendered_lines: usize,
    /// Inner height of the notes panel at the last draw, used with
    /// `notes_rendered_lines` to clamp scroll to the visual bottom.
    pub notes_view_height: usize,
    /// Active file-list filter (review mode only). Narrows the file list to
    /// undecided / rejected / changed files for large changesets.
    pub file_filter: FileFilter,
    /// Paths whose diff fingerprint differs from (or is absent in) the last
    /// finished review snapshot — i.e. files that changed since the reviewer
    /// last looked. Drives the `Changed` filter and the file-list marker.
    /// Empty on a first review.
    pub changed_since_last: std::collections::HashSet<String>,
    /// Whether a prior review snapshot existed when this review opened. Lets the
    /// UI and the filter cycle distinguish a first review from a re-review.
    pub has_prior_review: bool,
    /// File path -> the feature agent's replies from the previous review round,
    /// parsed from `.claude/final-review-feedback.md` on open. Surfaced beside the
    /// diff so a re-review shows what the agent said it did per file. Empty on a
    /// first review or when the agent left no `**Agent:**` replies.
    pub prior_agent_responses: std::collections::HashMap<String, Vec<AgentResponse>>,
    /// Where a finished review's "address this feedback" prompt is dispatched:
    /// the feature's existing agent pane (the default), a fresh dedicated review
    /// session, another existing feature's session, or a brand-new companion
    /// feature. Chosen via the destination picker opened with `t`.
    pub fix_target: crate::app::pr_review::FixTarget,
    /// `Feature::id` the destination points at, for
    /// [`crate::app::pr_review::FixTarget::ExistingFeature`] (a feature the
    /// reviewer picked) and `NewFeature` (the companion feature created at
    /// picker-confirm time). `None` for the two in-feature targets.
    pub fix_target_feature_id: Option<String>,
    /// Harness chosen for the dedicated / companion review session, when the
    /// picker resolved one up front. `None` for the in-feature targets and for
    /// the dedicated target when the harness is still to be picked at finish.
    pub review_harness: Option<crate::project::AgentKind>,
    /// When `Some`, the destination picker is open over the review viewer (`t`):
    /// choosing where the finished review's "address the feedback" prompt is
    /// dispatched. A sub-state, like PR Triage's `harness_pick`.
    pub destination_pick: Option<ReviewDestinationPickState>,
    /// When `Some`, the compact companion-feature setup overlay is open: the
    /// reviewer picked "New feature…" in the destination picker and is choosing
    /// the companion's preset / harness / vibe mode / branch before it is
    /// created. Reuses PR Triage's [`TriageFeatureSetupState`].
    pub review_feature_setup: Option<TriageFeatureSetupState>,
    /// True while the reviewer is typing a diff search query in the prompt
    /// (opened with `/`). Takes precedence over every other key binding.
    pub editing_search: bool,
    /// Active diff search query — also the in-progress text while
    /// `editing_search`. Empty when no search is active. Matched
    /// case-insensitively as a substring of the current file's addressable line
    /// texts.
    pub search_query: String,
    /// Indices into the current file's `addressable_lines()` that match
    /// `search_query`, ascending. Recomputed whenever the query or selected file
    /// changes; empty when there is no match (or no query). Current-file only.
    pub search_matches: Vec<usize>,
    /// Position within `search_matches` of the current match (the one the line
    /// cursor sits on). `None` when there are no matches.
    pub search_match_pos: Option<usize>,
    /// In-flight background process running the project's configured
    /// `final_review_check_command` (a build/test gate), spawned by
    /// `finish_final_review` and polled to completion like
    /// `changeset_overview_child`. `None` when no check is configured or
    /// none is currently running.
    pub finish_check_child: Option<Child>,
    /// The command `finish_check_child` is running, kept so the result can
    /// be reported once it exits.
    pub finish_check_command: Option<String>,
    /// On-demand "since last review" diff for the current file (`I` in the
    /// final review), computed against the last review snapshot's saved
    /// content by `open_interdiff`. Recomputed on each open (a single cheap
    /// local `git diff --no-index`, not a headless pass) rather than kept
    /// across files like `changeset_overview`.
    pub interdiff_file: Option<crate::diff::DiffFile>,
    /// True while the interdiff modal is shown; takes full key precedence
    /// while open, mirroring `changeset_overview_open`.
    pub interdiff_open: bool,
    pub interdiff_scroll: usize,
    /// True while the pre-finish summary modal is shown: every verdict, open
    /// comment and suggestion in one navigable list, so `q` gives one last
    /// look before feedback is written and dispatched. Opened by
    /// `confirm_or_finish_review` once the undecided-files gate (if any) has
    /// been cleared; takes full key precedence while open, mirroring
    /// `changeset_overview_open`.
    pub summary_open: bool,
    /// Selected row in `summary_items()`, clamped to its length on navigation.
    pub summary_selected: usize,
    /// Read-only review-round timeline/history browser (`H`). `None` while
    /// closed. Historical rounds are loaded from the live feedback log first;
    /// the archive is read only when the reviewer navigates beyond that tail.
    pub review_history: Option<ReviewHistoryState>,
    /// Directory paths (repo-relative, no trailing slash) currently collapsed in
    /// the file tree. Purely a view concern: a collapsed directory hides its
    /// rows, but never its files from filters, counts or file-order navigation —
    /// landing on a file inside one re-expands its ancestors
    /// (`reveal_selected_file`) so the selection is always reachable.
    pub collapsed_dirs: std::collections::BTreeSet<String>,
    /// Set while the file-list row cursor is parked on a *directory* row rather
    /// than a file. The selected file (and therefore the patch panel) is left
    /// alone, so collapsing a tree never changes what's being diffed.
    pub tree_cursor_dir: Option<String>,
    /// When true the diff is loaded with `git diff -w`, so lines that differ
    /// only in whitespace don't show as changes. Toggling re-runs the loader
    /// (it changes what git emits, not just how it's drawn), so it survives via
    /// the same reload path as a base-ref change.
    pub ignore_whitespace: bool,
    /// File path -> how many context lines that file's hunks are currently
    /// rendered with (`usize::MAX` = the whole file). An absent entry is git's
    /// `--unified=3` default. Applied by rewriting the file's hunks, so every
    /// consumer — `addressable_lines()`, the renderers, comment anchors —
    /// agrees on what the reviewer is looking at. View state only: re-applied
    /// after a reload, never written to the progress file.
    pub context_expansion: std::collections::HashMap<String, usize>,
    /// Undo stack for explicit verdicts (approve / skip / typed rejection), most
    /// recent last. Session-only: an undo is a correction of the key you just
    /// pressed, so it deliberately doesn't survive a pause/resume the way the
    /// verdicts themselves do.
    pub verdict_undo: Vec<VerdictUndo>,
    /// True while the review-mode `?` help overlay is shown. The review key
    /// surface outgrew what two footer rows can teach, so the overlay lists it
    /// grouped by task. Read-only and takes full key precedence while open,
    /// mirroring `changeset_overview_open`.
    pub help_open: bool,
    pub help_scroll: usize,
    /// Rendered line count / viewport height of the help overlay at the last
    /// draw, mirroring `changeset_overview_rendered_lines` /
    /// `changeset_overview_view_height` so scroll clamps to the real bottom.
    pub help_rendered_lines: usize,
    pub help_view_height: usize,
}

/// One entry on the verdict undo stack: everything needed to put a file's
/// verdict back exactly as it was before the reviewer's last `a` / `s` / `r`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerdictUndo {
    pub path: String,
    /// The file's verdict before the undone action. `None` when it had none
    /// (undecided or previously skipped).
    pub previous: Option<ReviewDecision>,
    /// Whether that previous verdict was one the line-comment rule had set
    /// implicitly, so undoing restores the implicit/explicit distinction too.
    pub previous_auto_rejected: bool,
}

/// How many verdicts back `U` can walk. A bound only so a very long review
/// can't grow the stack without limit; deep undo is not the point.
pub const VERDICT_UNDO_LIMIT: usize = 50;

impl DiffViewerState {
    /// Replace the shared review editor with a fresh instance that follows the
    /// session keymap. Vim editors always enter Normal mode; either keymap gets
    /// fresh cursor and undo state while retaining the supplied text.
    pub(crate) fn reset_feedback_editor(&mut self, text: String) {
        self.feedback_editor = if self.vim_enabled {
            TextEditor::with_vim_normal(text)
        } else {
            TextEditor::new(text)
        };
    }

    /// Toggle Vim for this review session and immediately apply it to the
    /// active shared editor. Rebuilding is intentional: the text survives, but
    /// keymap-specific cursor, pending-command, register, and undo state do not
    /// cross the keymap boundary.
    pub(crate) fn toggle_feedback_vim(&mut self) -> bool {
        self.vim_enabled = !self.vim_enabled;
        let text = self.feedback_editor.text().to_string();
        self.reset_feedback_editor(text);
        self.feedback_scroll = 0;
        self.feedback_sync_to_cursor = true;
        self.vim_enabled
    }

    pub fn new(from_view: ViewState, workdir: PathBuf) -> Self {
        Self {
            from_view,
            workdir,
            scope: DiffScope::CurrentChanges,
            branch: String::new(),
            base_ref: String::new(),
            base_commit: String::new(),
            override_base_ref: None,
            editing_base_ref: false,
            base_ref_input: String::new(),
            files: Vec::new(),
            selected_file: 0,
            patch_scroll: 0,
            focus: DiffViewerFocus::FileList,
            layout: DiffViewerLayout::Unified,
            error: None,
            review: false,
            decisions: std::collections::HashMap::new(),
            auto_rejected: std::collections::HashSet::new(),
            line_comments: std::collections::HashMap::new(),
            file_comments: std::collections::HashMap::new(),
            comment_cursor: None,
            comment_anchor: None,
            editing_line_comment: false,
            editing_file_comment: false,
            editing_suggestion: false,
            apply_suggestions_on_finish: false,
            applied_suggestions: Vec::new(),
            suggestion_apply_failures: Vec::new(),
            comment_severity: Severity::default(),
            cursor_sync_to_view: false,
            finish_confirm: false,
            feedback_editing: false,
            editing_general: false,
            vim_enabled: false,
            feedback_editor: TextEditor::new(String::new()),
            feedback_scroll: 0,
            feedback_sync_to_cursor: true,
            general_feedback: String::new(),
            review_notes: std::collections::HashMap::new(),
            generated_notes: std::collections::HashMap::new(),
            walkthrough_child: None,
            walkthrough_file: None,
            co_review_child: None,
            co_review_file: None,
            co_review_bg: None,
            changeset_overview: None,
            changeset_overview_child: None,
            changeset_overview_open: false,
            changeset_overview_scroll: 0,
            changeset_overview_rendered_lines: 0,
            changeset_overview_view_height: 0,
            notes_expanded: false,
            notes_scroll: 0,
            notes_rendered_lines: 0,
            notes_view_height: 0,
            file_filter: FileFilter::All,
            changed_since_last: std::collections::HashSet::new(),
            has_prior_review: false,
            prior_agent_responses: std::collections::HashMap::new(),
            fix_target: crate::app::pr_review::FixTarget::ExistingLive,
            fix_target_feature_id: None,
            review_harness: None,
            destination_pick: None,
            review_feature_setup: None,
            editing_search: false,
            search_query: String::new(),
            search_matches: Vec::new(),
            search_match_pos: None,
            finish_check_child: None,
            finish_check_command: None,
            interdiff_file: None,
            interdiff_open: false,
            interdiff_scroll: 0,
            summary_open: false,
            summary_selected: 0,
            review_history: None,
            collapsed_dirs: std::collections::BTreeSet::new(),
            tree_cursor_dir: None,
            ignore_whitespace: false,
            context_expansion: std::collections::HashMap::new(),
            verdict_undo: Vec::new(),
            help_open: false,
            help_scroll: 0,
            help_rendered_lines: 0,
            help_view_height: 0,
        }
    }

    /// Drop any active diff search (query, matches and current-match position).
    /// Called when the search is cancelled/cleared and whenever the selected
    /// file changes, since matches are anchored to a single file.
    pub fn clear_search(&mut self) {
        self.editing_search = false;
        self.search_query.clear();
        self.search_matches.clear();
        self.search_match_pos = None;
    }

    /// Reset the per-file view state after the selected file changes (patch /
    /// notes scroll, and the line-comment cursor). Centralizes what several
    /// navigation paths previously duplicated.
    /// Re-apply the reviewer's per-file context expansion to a freshly loaded
    /// diff. Expansion is a view preference rather than part of the diff, so a
    /// refresh (or a base-ref change) must not silently collapse what was
    /// expanded. Files that dropped out of the changeset — or can no longer be
    /// expanded against the new blobs — fall back to the default and lose their
    /// entry.
    pub fn reapply_context_expansion(&mut self) {
        if self.context_expansion.is_empty() {
            return;
        }
        let levels = std::mem::take(&mut self.context_expansion);
        for file in self.files.iter_mut() {
            let Some(&level) = levels.get(&file.path) else {
                continue;
            };
            if let Some(hunks) = file.hunks_with_context(level) {
                file.hunks = hunks;
                self.context_expansion.insert(file.path.clone(), level);
            }
        }
    }

    pub fn on_file_changed(&mut self) {
        self.patch_scroll = 0;
        self.notes_scroll = 0;
        if self.comment_cursor.is_some() {
            self.comment_cursor = Some(0);
            self.cursor_sync_to_view = true;
        }
        // A range selection can't carry across files.
        self.comment_anchor = None;
        // Search matches are anchored to a single file; end the search rather
        // than leaving a stale query pointing at the previous file.
        self.clear_search();
        // The cursor is on a file again, and that file must be visible: every
        // file-order navigation path funnels through here, so no caller has to
        // know the tree can be folded.
        self.tree_cursor_dir = None;
        self.reveal_selected_file();
    }

    /// Record `path`'s current verdict on the undo stack before an explicit
    /// verdict replaces it, so `U` can put it back exactly — including whether
    /// the rejection being replaced was one the line-comment rule had set
    /// implicitly. A press that changes nothing isn't recorded: re-approving an
    /// already-approved file would otherwise leave a `U` that does nothing
    /// visible.
    pub fn push_verdict_undo(&mut self, path: &str, next: Option<&ReviewDecision>) {
        let previous = self.decisions.get(path).cloned();
        let previous_auto_rejected = self.auto_rejected.contains(path);
        // Every verdict path also drops the file from `auto_rejected`, so an
        // implicit rejection is a real change even when the verdict compares
        // equal.
        if previous.as_ref() == next && !previous_auto_rejected {
            return;
        }
        if self.verdict_undo.len() >= VERDICT_UNDO_LIMIT {
            self.verdict_undo.remove(0);
        }
        self.verdict_undo.push(VerdictUndo {
            path: path.to_string(),
            previous,
            previous_auto_rejected,
        });
    }

    /// Whether the file at `path` carries a `Blocker`-severity signal: either a
    /// blocker rejection or any kept (non-draft) blocker line comment. Feeds the
    /// `Blockers` file filter and the GitHub review-event escalation.
    pub fn file_has_blocker(&self, path: &str) -> bool {
        let reject_blocks = matches!(
            self.decisions.get(path),
            Some(ReviewDecision::Reject { severity, .. }) if severity.is_blocker()
        );
        // A resolved thread is settled: it must not keep its file pinned in the
        // blockers filter, nor escalate the GitHub review event.
        let comment_blocks = self.line_comments.get(path).is_some_and(|cs| {
            cs.iter()
                .any(|c| c.is_open_thread() && c.severity.is_blocker())
        });
        let file_comment_blocks = self
            .file_comments
            .get(path)
            .is_some_and(|c| c.is_open_thread() && c.severity.is_blocker());
        reject_blocks || comment_blocks || file_comment_blocks
    }

    /// Whether the file at `path` carries at least one open thread — a kept,
    /// unresolved line comment. Backs the `Unresolved` filter and the auto-reject
    /// rule (an open thread means the file still needs work).
    pub fn file_has_unresolved_thread(&self, path: &str) -> bool {
        self.line_comments
            .get(path)
            .is_some_and(|cs| cs.iter().any(|c| c.is_open_thread()))
            || self
                .file_comments
                .get(path)
                .is_some_and(FileComment::is_open_thread)
    }

    /// Total open threads across every file in the diff. Reported on opening a
    /// re-review and used to decide whether the `Unresolved` filter has anything
    /// to show.
    pub fn unresolved_thread_count(&self) -> usize {
        let line = self
            .line_comments
            .values()
            .flatten()
            .filter(|c| c.is_open_thread())
            .count();
        line + self
            .file_comments
            .values()
            .filter(|c| c.is_open_thread())
            .count()
    }

    /// Number of kept, unresolved suggested changes that could be applied to
    /// the worktree. Lost anchors are included so an attempted batch reports
    /// why they were skipped instead of silently hiding them.
    pub fn pending_suggestion_count(&self) -> usize {
        self.line_comments
            .values()
            .flatten()
            .filter(|comment| comment.is_open_thread() && comment.suggestion.is_some())
            .count()
    }

    /// True when no line comment was authored in *this* session — every stored
    /// comment (if any) was carried in from a previous finished round. Lets a
    /// fresh re-review still read as "pristine" for the purposes of auto-applying
    /// the `Changed` filter, even though it opens with threads restored.
    pub fn has_only_carried_comments(&self) -> bool {
        self.line_comments.values().flatten().all(|c| c.carried)
            && self.file_comments.values().all(|c| c.carried)
    }

    /// Whether `file` passes the active file-list filter. Always true outside
    /// review mode or under the `All` filter.
    fn file_passes_filter(&self, file: &crate::diff::DiffFile) -> bool {
        match self.file_filter {
            FileFilter::All => true,
            FileFilter::Undecided => !self.decisions.contains_key(&file.path),
            FileFilter::Rejected => matches!(
                self.decisions.get(&file.path),
                Some(ReviewDecision::Reject { .. })
            ),
            FileFilter::Blockers => self.file_has_blocker(&file.path),
            FileFilter::FileComments => self
                .file_comments
                .get(&file.path)
                .is_some_and(FileComment::is_open_thread),
            FileFilter::Unresolved => self.file_has_unresolved_thread(&file.path),
            FileFilter::Changed => self.changed_since_last.contains(&file.path),
        }
    }

    /// Indices into `files` of the files currently shown under the active
    /// filter, in file order. The full list outside review / with `All`.
    pub fn visible_file_indices(&self) -> Vec<usize> {
        if !self.review || self.file_filter == FileFilter::All {
            return (0..self.files.len()).collect();
        }
        self.files
            .iter()
            .enumerate()
            .filter(|(_, file)| self.file_passes_filter(file))
            .map(|(i, _)| i)
            .collect()
    }

    /// The file list as a directory tree, in the same order as
    /// `visible_file_indices` — `files` is sorted by full path
    /// (`crate::diff`), and comparing a directory as `name/` against a file as
    /// `name` reproduces exactly that ordering, so grouping never reorders the
    /// list. Directory rows are emitted when the path prefix changes; a
    /// collapsed directory emits its own row and swallows everything beneath
    /// it.
    pub fn file_tree_rows(&self) -> Vec<FileTreeRow> {
        let visible = self.visible_file_indices();
        // Visible-file count per ancestor directory, for the row's `(n)` badge.
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for &idx in &visible {
            for dir in ancestor_dirs(&self.files[idx].path) {
                *counts.entry(dir).or_default() += 1;
            }
        }

        let mut rows = Vec::new();
        // Directory segments of the previous file, so a shared prefix is only
        // emitted once.
        let mut open: Vec<&str> = Vec::new();
        for &idx in &visible {
            let path = self.files[idx].path.as_str();
            let (dir_part, name) = match path.rfind('/') {
                Some(pos) => (&path[..pos], &path[pos + 1..]),
                None => ("", path),
            };
            let comps: Vec<&str> = if dir_part.is_empty() {
                Vec::new()
            } else {
                dir_part.split('/').collect()
            };

            let mut common = 0;
            while common < open.len() && common < comps.len() && open[common] == comps[common] {
                common += 1;
            }
            open.truncate(common);

            // A directory already on the stack may be collapsed, in which case
            // its row was emitted earlier and everything below it is hidden.
            let mut hidden = (1..=open.len())
                .any(|depth| self.collapsed_dirs.contains(&comps[..depth].join("/")));

            for depth in common..comps.len() {
                open.push(comps[depth]);
                if hidden {
                    continue;
                }
                let full = comps[..=depth].join("/");
                let collapsed = self.collapsed_dirs.contains(&full);
                rows.push(FileTreeRow::Dir {
                    label: comps[depth].to_string(),
                    depth,
                    collapsed,
                    files: counts.get(&full).copied().unwrap_or(0),
                    path: full,
                });
                if collapsed {
                    hidden = true;
                }
            }

            if !hidden {
                rows.push(FileTreeRow::File {
                    index: idx,
                    depth: comps.len(),
                    name: name.to_string(),
                });
            }
        }
        rows
    }

    /// Expand every collapsed ancestor of the selected file so the selection is
    /// always on a row the reviewer can see. Called from `on_file_changed`, so
    /// every file-order navigation path (n/p, verdict advance, filters, search,
    /// summary jumps) reveals its target without having to know about the tree.
    pub fn reveal_selected_file(&mut self) {
        let Some(file) = self.files.get(self.selected_file) else {
            return;
        };
        for dir in ancestor_dirs(&file.path) {
            self.collapsed_dirs.remove(&dir);
        }
    }

    /// Toggle a directory's collapsed state. Collapsing an ancestor of the
    /// selected file is allowed — the file stays selected and the patch panel
    /// keeps showing it; only the row is folded away.
    pub fn toggle_dir_collapsed(&mut self, dir: &str) {
        if !self.collapsed_dirs.remove(dir) {
            self.collapsed_dirs.insert(dir.to_string());
        }
    }

    /// Every directory that currently has a row in the tree (regardless of
    /// collapse state), in row order.
    pub fn tree_dirs(&self) -> Vec<String> {
        self.file_tree_rows()
            .into_iter()
            .filter_map(|row| match row {
                FileTreeRow::Dir { path, .. } => Some(path),
                FileTreeRow::File { .. } => None,
            })
            .collect()
    }

    /// Row index the file-list cursor sits on: the directory row when the
    /// cursor is parked on one, else the selected file's row. Falls back to the
    /// deepest visible ancestor directory if the selected file happens to be
    /// folded away, so a row is always highlighted.
    pub fn tree_cursor_row(&self, rows: &[FileTreeRow]) -> Option<usize> {
        if let Some(dir) = &self.tree_cursor_dir
            && let Some(pos) = rows
                .iter()
                .position(|row| matches!(row, FileTreeRow::Dir { path, .. } if path == dir))
        {
            return Some(pos);
        }
        if let Some(pos) = rows.iter().position(
            |row| matches!(row, FileTreeRow::File { index, .. } if *index == self.selected_file),
        ) {
            return Some(pos);
        }
        let path = self.files.get(self.selected_file)?.path.as_str();
        ancestor_dirs(path).into_iter().rev().find_map(|dir| {
            rows.iter()
                .position(|row| matches!(row, FileTreeRow::Dir { path, .. } if *path == dir))
        })
    }

    /// Directory the fold commands act on, derived from whichever row
    /// `tree_cursor_row` highlights: a directory row folds itself, a file row
    /// folds its own directory. Reading it back off the highlighted row —
    /// rather than off the selected file — matters when the selection is
    /// hidden by the active filter, where the highlight falls back to some
    /// *shallower* ancestor than the selected file's own directory.
    pub fn tree_cursor_target_dir(&self, rows: &[FileTreeRow]) -> Option<String> {
        match rows.get(self.tree_cursor_row(rows)?)? {
            FileTreeRow::Dir { path, .. } => Some(path.clone()),
            FileTreeRow::File { index, .. } => self
                .files
                .get(*index)
                .and_then(|file| ancestor_dirs(&file.path).pop()),
        }
    }

    /// Every row of the pre-finish summary, in file order: each file's verdict
    /// row, then its open line comments (already sorted by line) and open file
    /// comment, followed by the overall feedback if any was written. Ignores
    /// the active file-list filter — the summary is deliberately everything,
    /// not just what's currently visible. Rebuilt fresh on every open/jump
    /// rather than cached, since it's cheap and always derived from state that
    /// can change underneath it (a jump-to-edit round-trip).
    pub fn summary_items(&self) -> Vec<SummaryItem> {
        let mut items = Vec::new();
        for (file_idx, file) in self.files.iter().enumerate() {
            items.push(SummaryItem::File { file_idx });
            if let Some(comments) = self.line_comments.get(&file.path) {
                for (comment_idx, comment) in comments.iter().enumerate() {
                    if comment.is_open_thread() {
                        items.push(SummaryItem::LineComment {
                            file_idx,
                            comment_idx,
                        });
                    }
                }
            }
            if self
                .file_comments
                .get(&file.path)
                .is_some_and(FileComment::is_open_thread)
            {
                items.push(SummaryItem::FileComment { file_idx });
            }
        }
        if !self.general_feedback.trim().is_empty() {
            items.push(SummaryItem::General);
        }
        items
    }
}

/// A feature whose dispatched review-fix prompt is being watched via the
/// thinking-status sync so a "fixes ready — re-review?" notification can be
/// raised once the agent goes idle again. Keyed by `feature.tmux_session` in
/// `App::awaiting_review_fixes` (thinking status is tracked per tmux session,
/// not per window, so this is the same granularity the dedicated-review-
/// session target already lives with).
#[derive(Debug, Clone)]
pub struct AwaitingReviewFix {
    /// Set once the session is observed thinking after the prompt was
    /// dispatched, so an idle transition only fires the notification after
    /// the agent has actually started (and finished) working — not on
    /// whatever idle/thinking state happened to precede the dispatch.
    pub started_thinking: bool,
}

pub struct DiffReviewState {
    pub session_id: String,
    pub workdir: PathBuf,
    #[allow(dead_code)] // populated but not read yet
    pub file_path: String,
    pub relative_path: String,
    #[allow(dead_code)] // populated but not read yet
    pub change_id: String,
    pub tool: String,
    pub old_snippet: String,
    pub new_snippet: String,
    pub diff_file: Option<crate::diff::DiffFile>,
    pub diff_error: Option<String>,
    pub patch_scroll: usize,
    pub reason: String,
    pub editing_feedback: bool,
    pub layout: DiffViewerLayout,
    pub explanation: Option<String>,
    pub explanation_child: Option<crate::headless::LeasedChild>,
    pub response_file: PathBuf,
    pub proceed_signal: PathBuf,
    pub request_id: Option<String>,
    pub reply_socket: Option<String>,
    pub return_to_view: Option<ViewState>,
    pub opened_at: Instant,
    pub hold_secs: f64,
}

impl DiffReviewState {
    pub fn hold_remaining_secs(&self) -> f64 {
        let elapsed = self.opened_at.elapsed().as_secs_f64();
        (self.hold_secs - elapsed).max(0.0)
    }

    pub fn hold_active(&self) -> bool {
        self.hold_remaining_secs() > 0.0
    }
}
