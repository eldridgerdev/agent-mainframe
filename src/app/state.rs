use ratatui_explorer::FileExplorer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use super::PromptAnalysis;
use crate::editor::TextEditor;
use crate::extension::{
    ConfiguredPlanQuestion, CustomSessionConfig, FeaturePreset, LifecycleHooks,
};
use crate::plan_interview::{PlanQuestion, PlanQuestionKind};
use crate::project::{AgentKind, SessionKind, VibeMode};
use crate::token_tracking::{SessionTokenUsage, TokenUsageSource};
use crate::worktree::WorktreeInfo;

pub const STARTUP_MASK_MAX_DURATION: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq)]
pub enum ForkFeatureStep {
    Branch,
    Agent,
}

pub struct ForkFeatureState {
    pub source_pi: usize,
    pub source_fi: usize,
    pub project_name: String,
    pub project_repo: PathBuf,
    pub source_branch: String,
    pub new_branch: String,
    pub step: ForkFeatureStep,
    pub agent: AgentKind,
    pub agent_index: usize,
    pub mode: VibeMode,
    pub review: bool,
    pub enable_chrome: bool,
    pub remote_control: bool,
    pub include_context: bool,
}

#[derive(Debug, Clone)]
pub enum Selection {
    Project(usize),
    Feature(usize, usize),
    Session(usize, usize, usize),
}

#[derive(Clone, Default)]
pub struct TextSelection {
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
    pub is_selecting: bool,
    pub has_selection: bool,
}

impl TextSelection {
    pub fn normalized(&self) -> (u16, u16, u16, u16) {
        if self.start_row < self.end_row
            || (self.start_row == self.end_row && self.start_col <= self.end_col)
        {
            (self.start_row, self.start_col, self.end_row, self.end_col)
        } else {
            (self.end_row, self.end_col, self.start_row, self.start_col)
        }
    }
}

#[derive(Clone)]
pub struct ViewState {
    pub project_name: String,
    pub feature_name: String,
    pub session: String,
    pub window: String,
    pub session_label: String,
    pub session_kind: SessionKind,
    pub vibe_mode: VibeMode,
    pub review: bool,
    pub scroll_offset: usize,
    pub scroll_content: String,
    pub scroll_mode: bool,
    pub scroll_total_lines: usize,
    pub scroll_passthrough: bool,
    pub selection: TextSelection,
    pub sidebar_visible: bool,
    pub todos_expanded: bool,
    pub startup_mask_started_at: Option<Instant>,
}

impl ViewState {
    // Constructor args map 1:1 onto the identity fields of ViewState.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        project_name: String,
        feature_name: String,
        session: String,
        window: String,
        session_label: String,
        session_kind: SessionKind,
        vibe_mode: VibeMode,
        review: bool,
    ) -> Self {
        Self {
            project_name,
            feature_name,
            session,
            window,
            session_label,
            session_kind,
            vibe_mode,
            review,
            scroll_offset: 0,
            scroll_content: String::new(),
            scroll_mode: false,
            scroll_total_lines: 0,
            scroll_passthrough: false,
            selection: TextSelection::default(),
            sidebar_visible: true,
            todos_expanded: false,
            startup_mask_started_at: None,
        }
    }

    pub fn show_startup_mask(&mut self) {
        self.startup_mask_started_at = Some(Instant::now());
    }

    pub fn startup_mask_active(&self) -> bool {
        self.startup_mask_started_at
            .is_some_and(|started_at| started_at.elapsed() < STARTUP_MASK_MAX_DURATION)
    }

    pub fn sidebar_session_kind(&self) -> Option<SessionKind> {
        if !self.sidebar_visible {
            return None;
        }

        match self.session_kind {
            SessionKind::Claude | SessionKind::Codex | SessionKind::Opencode => {
                Some(self.session_kind.clone())
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInput {
    pub session_id: String,
    pub cwd: String,
    pub message: String,
    pub notification_type: String,
    pub file_path: PathBuf,
    pub target_file_path: Option<String>,
    pub relative_path: Option<String>,
    pub change_id: Option<String>,
    pub tool: Option<String>,
    pub old_snippet: Option<String>,
    pub new_snippet: Option<String>,
    pub original_file: Option<String>,
    pub proposed_file: Option<String>,
    pub is_new_file: Option<bool>,
    pub reason: Option<String>,
    pub response_file: Option<String>,
    pub project_name: Option<String>,
    pub feature_name: Option<String>,
    pub proceed_signal: Option<String>,
    pub request_id: Option<String>,
    pub reply_socket: Option<String>,
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

pub enum RenameReturnTo {
    Dashboard,
    SessionSwitcher(super::SessionSwitcherState),
}

pub struct RenameSessionState {
    pub project_idx: usize,
    pub feature_idx: usize,
    pub session_idx: usize,
    pub input: String,
    pub return_to: RenameReturnTo,
}

#[derive(Clone)]
pub enum NewSessionTarget {
    Builtin(SessionKind),
    // Boxed: CustomSessionConfig is ~10 fields and would dominate the enum size.
    Custom(Box<CustomSessionConfig>),
}

#[derive(Clone)]
pub struct NewSessionNameState {
    pub project_idx: usize,
    pub feature_idx: usize,
    pub target: NewSessionTarget,
    pub input: String,
    pub return_to: SessionPickerState,
}

#[derive(Debug, Clone)]
pub struct RenameFeatureState {
    pub project_idx: usize,
    pub feature_idx: usize,
    pub input: String,
}

pub struct SessionConfigState {
    pub project_idx: usize,
    pub feature_idx: usize,
    pub project_name: String,
    pub feature_name: String,
    pub current_agent: AgentKind,
    pub allowed_agents: Vec<AgentKind>,
    pub selected_agent: usize,
}

pub struct ProjectAgentConfigState {
    pub project_idx: usize,
    pub project_name: String,
    pub current_agent: AgentKind,
    pub allowed_agents: Vec<AgentKind>,
    pub selected_agent: usize,
}

#[derive(Debug, Clone)]
pub struct OpencodeSessionInfo {
    pub id: String,
    pub slug: Option<String>,
    pub title: String,
    pub updated: i64,
}

#[derive(Debug, Clone)]
pub struct OpencodeSessionPickerState {
    pub sessions: Vec<OpencodeSessionInfo>,
    pub selected: usize,
    pub workdir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ClaudeSessionPickerState {
    pub sessions: Vec<super::claude_sessions::ClaudeSessionInfo>,
    pub selected: usize,
    pub workdir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct CodexSessionPickerState {
    pub sessions: Vec<super::codex_sessions::CodexSessionInfo>,
    pub selected: usize,
    pub workdir: PathBuf,
}

#[derive(Clone)]
pub struct BookmarkPickerState {
    pub selected: usize,
    pub from_view: Option<ViewState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffViewerFocus {
    FileList,
    Patch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffViewerLayout {
    Unified,
    SideBySide,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffScope {
    /// The existing branch snapshot: every commit since the resolved base plus
    /// staged, unstaged, and untracked worktree changes.
    CurrentChanges,
    /// Exactly one commit, compared with its first parent.
    Commit(crate::diff::DiffCommit),
}

pub struct DiffPickerState {
    pub from_view: ViewState,
    pub workdir: PathBuf,
    pub commits: Vec<crate::diff::DiffCommit>,
    /// Zero is "all current changes"; commit rows start at one.
    pub selected: usize,
    pub error: Option<String>,
}

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

// Not `Clone`: holds a `std::process::Child` for the in-flight walkthrough
// generation (matching `DiffReviewState`). Nothing clones this state wholesale.
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
    pub walkthrough_child: Option<Child>,
    /// Path the in-flight walkthrough is being generated for, so the result is
    /// filed correctly even if the reviewer navigates to another file.
    pub walkthrough_file: Option<String>,
    /// In-flight headless AI co-review pass (one at a time). Separate slot from
    /// the walkthrough so the two can't clobber each other.
    pub co_review_child: Option<Child>,
    /// Path the in-flight co-review is being generated for, so draft comments
    /// land on the right file even if the reviewer navigates away.
    pub co_review_file: Option<String>,
    /// Cached on-demand whole-changeset overview / risk summary (headless,
    /// reviewer-triggered — see `changeset_overview_open`). Kept until the
    /// reviewer explicitly regenerates it so reopening the modal is free.
    pub changeset_overview: Option<String>,
    /// In-flight headless process generating the changeset overview.
    pub changeset_overview_child: Option<Child>,
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
    /// the feature's existing agent pane (the default, unchanged behaviour) or a
    /// fresh dedicated review session. Toggled with `t` in the review viewer.
    pub fix_target: crate::app::pr_review::FixTarget,
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
}

impl DiffViewerState {
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

#[derive(Clone)]
pub struct SteeringPromptState {
    pub view: ViewState,
    pub workdir: PathBuf,
    pub editor: TextEditor,
    pub prompt_analysis: PromptAnalysis,
    pub scroll_offset: usize,
    pub sync_scroll_to_cursor: bool,
}

impl SteeringPromptState {
    pub fn new(view: ViewState, workdir: PathBuf, prompt: String) -> Self {
        let editor = TextEditor::with_vim(prompt);
        let prompt_analysis = crate::app::analyze_prompt(editor.text());
        Self {
            view,
            workdir,
            editor,
            prompt_analysis,
            scroll_offset: 0,
            sync_scroll_to_cursor: true,
        }
    }

    pub fn refresh_prompt_analysis(&mut self) {
        self.prompt_analysis = crate::app::analyze_prompt(self.editor.text());
    }

    pub fn request_cursor_scroll(&mut self) {
        self.sync_scroll_to_cursor = true;
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.sync_scroll_to_cursor = false;
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
        self.sync_scroll_to_cursor = false;
    }

    pub fn clear_prompt(&mut self) -> bool {
        let cleared = self.editor.clear().text_changed;
        if cleared {
            self.refresh_prompt_analysis();
        }
        self.scroll_offset = 0;
        self.sync_scroll_to_cursor = false;
        cleared
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComposeCommandSource {
    BuiltIn,
    Global,
    Project,
    Skill,
}

impl ComposeCommandSource {
    pub fn label(self) -> &'static str {
        match self {
            ComposeCommandSource::BuiltIn => "Built-in",
            ComposeCommandSource::Global => "Global",
            ComposeCommandSource::Project => "Project",
            ComposeCommandSource::Skill => "Skill",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ComposeCommandEntry {
    /// Command name without the leading slash (e.g. "compact").
    pub name: String,
    pub description: String,
    pub source: ComposeCommandSource,
    /// True when the command opens a CC-owned interactive dialog;
    /// submitting it drops the session into direct (passthrough) mode.
    pub interactive: bool,
}

/// An image pasted into the compose box, shown as a `[Image N]`
/// placeholder in the editor and delivered to Claude Code via the
/// clipboard at submit time.
#[derive(Clone)]
pub struct ComposeImage {
    pub placeholder: String,
    pub data: Vec<u8>,
    pub mime: String,
}

/// Unsent compose content saved when the box closes without sending.
#[derive(Clone, Default)]
pub struct ComposeDraft {
    pub text: String,
    pub images: Vec<ComposeImage>,
}

#[derive(Clone)]
pub struct ComposeState {
    pub view: ViewState,
    pub workdir: PathBuf,
    pub editor: TextEditor,
    pub scroll_offset: usize,
    pub sync_scroll_to_cursor: bool,
    /// Full command catalog built when the compose box opens.
    pub catalog: Vec<ComposeCommandEntry>,
    /// Catalog indices currently matching the typed /prefix.
    pub suggestions: Vec<usize>,
    pub suggestion_index: usize,
    /// Pasted images, in placeholder order.
    pub images: Vec<ComposeImage>,
    /// Background clipboard read currently feeding this compose box.
    pub clipboard_paste_id: Option<u64>,
    /// The compose buffer is being delivered to the harness in a
    /// background worker. This is used for the slower WSL image path.
    pub submit_in_progress: bool,
}

impl ComposeState {
    pub fn new(
        view: ViewState,
        workdir: PathBuf,
        text: String,
        catalog: Vec<ComposeCommandEntry>,
    ) -> Self {
        let mut state = Self {
            view,
            workdir,
            editor: TextEditor::new(text),
            scroll_offset: 0,
            sync_scroll_to_cursor: true,
            catalog,
            suggestions: Vec::new(),
            suggestion_index: 0,
            images: Vec::new(),
            clipboard_paste_id: None,
            submit_in_progress: false,
        };
        state.refresh_suggestions();
        state
    }

    /// Register a pasted image and return the placeholder to insert
    /// into the editor.
    pub fn add_image(&mut self, data: Vec<u8>, mime: String) -> String {
        let placeholder = format!("[Image {}]", self.images.len() + 1);
        self.images.push(ComposeImage {
            placeholder: placeholder.clone(),
            data,
            mime,
        });
        placeholder
    }

    pub fn request_cursor_scroll(&mut self) {
        self.sync_scroll_to_cursor = true;
    }

    pub fn paste_in_progress(&self) -> bool {
        self.clipboard_paste_id.is_some() || self.submit_in_progress
    }

    pub fn scroll_up(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(lines);
        self.sync_scroll_to_cursor = false;
    }

    pub fn scroll_down(&mut self, lines: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(lines);
        self.sync_scroll_to_cursor = false;
    }

    pub fn clear_prompt(&mut self) -> bool {
        let cleared = self.editor.clear().text_changed || !self.images.is_empty();
        self.images.clear();
        self.scroll_offset = 0;
        self.sync_scroll_to_cursor = false;
        self.refresh_suggestions();
        cleared
    }

    /// The /command token being typed, if the buffer is a single line
    /// starting with '/' and no arguments have been typed yet.
    pub fn pending_command_prefix(&self) -> Option<&str> {
        let text = self.editor.text();
        let rest = text.strip_prefix('/')?;
        if rest.contains('\n') || rest.contains(' ') {
            return None;
        }
        Some(rest)
    }

    /// True when the buffer holds a single-line /command (with or
    /// without arguments) that should be delivered as keystrokes.
    pub fn is_slash_command(&self) -> bool {
        let text = self.editor.text().trim();
        text.starts_with('/') && !text.contains('\n')
    }

    pub fn refresh_suggestions(&mut self) {
        let previously_selected = self.suggestions.get(self.suggestion_index).copied();

        match self.pending_command_prefix() {
            Some(prefix) => {
                // Fuzzy-rank the catalog so a query like "commit"
                // matches namespaced commands such as "stn:commit".
                let mut scored: Vec<(i32, usize)> = self
                    .catalog
                    .iter()
                    .enumerate()
                    .filter_map(|(idx, entry)| {
                        crate::app::compose::fuzzy_score(prefix, &entry.name)
                            .map(|score| (score, idx))
                    })
                    .collect();
                // Highest score first; ties keep catalog order, which
                // preserves the built-in/global/project/skill grouping.
                scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                self.suggestions = scored.into_iter().map(|(_, idx)| idx).collect();
            }
            None => self.suggestions.clear(),
        }

        self.suggestion_index = previously_selected
            .and_then(|catalog_idx| self.suggestions.iter().position(|idx| *idx == catalog_idx))
            .unwrap_or(0);
    }

    pub fn selected_suggestion(&self) -> Option<&ComposeCommandEntry> {
        self.suggestions
            .get(self.suggestion_index)
            .and_then(|idx| self.catalog.get(*idx))
    }

    pub fn select_next_suggestion(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        self.suggestion_index = (self.suggestion_index + 1) % self.suggestions.len();
    }

    pub fn select_prev_suggestion(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        self.suggestion_index = self
            .suggestion_index
            .checked_sub(1)
            .unwrap_or(self.suggestions.len() - 1);
    }

    /// Replace the typed /prefix with the selected suggestion's name.
    /// Returns true if a completion was applied.
    pub fn complete_selected_suggestion(&mut self) -> bool {
        let Some(entry) = self.selected_suggestion() else {
            return false;
        };
        let completed = format!("/{}", entry.name);
        if self.editor.text() == completed {
            return false;
        }
        self.editor = TextEditor::new(completed);
        self.refresh_suggestions();
        self.request_cursor_scroll();
        true
    }

    /// The command catalog entry matching the buffer exactly, if any.
    pub fn exact_command_match(&self) -> Option<&ComposeCommandEntry> {
        let text = self.editor.text().trim();
        let rest = text.strip_prefix('/')?;
        let name = rest.split_whitespace().next()?;
        self.catalog
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(name))
    }
}

#[derive(Clone)]
pub struct LatestPromptState {
    pub view: ViewState,
    pub prompts: Vec<crate::app::util::PromptEntry>,
    pub selected: usize,
}

/// A prompt-library row: a template plus where it came from. Phase 1
/// only surfaces `User` templates; the `source` field is ready for the
/// declarative `Global` / `Project` templates added in phase 3.
#[derive(Clone)]
pub struct PromptLibraryEntry {
    pub template: crate::prompt_library::PromptTemplate,
    pub source: crate::prompt_library::PromptSource,
    /// Resolved on-disk location this entry is read from / written to:
    /// the SQLite store for `User`, the relevant `.amf/config.json` for
    /// config sources. Filled in by `rebuild_prompt_library`; `None` when
    /// the scope has no resolvable location (no project context, or the
    /// empty test store path).
    pub source_path: Option<PathBuf>,
}

/// Picker over the merged, source-tagged prompt library. Mirrors the
/// `LatestPrompt` shape (a list with a `selected` index) plus fuzzy
/// filtering and an optional `from_view` to inject back into.
#[derive(Clone)]
pub struct PromptLibraryState {
    pub templates: Vec<PromptLibraryEntry>,
    /// Indices into `templates` matching `query`, best score first.
    pub filtered: Vec<usize>,
    pub query: String,
    pub search_active: bool,
    pub selected: usize,
    /// Set after the first `d` press; a second `d` confirms deletion.
    pub confirm_delete: bool,
    /// Set after `x`; `g` exports to global config, `p` to project.
    pub pending_export: bool,
    pub from_view: Option<ViewState>,
}

impl PromptLibraryState {
    /// The template currently highlighted in the filtered list.
    pub fn selected_entry(&self) -> Option<&PromptLibraryEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|idx| self.templates.get(*idx))
    }
}

/// Which field of the prompt editor currently has focus. `Tab` cycles
/// Name → Tags → Body (and `Shift+Tab` the reverse). Name and Tags are
/// single-line text fields; Body is the multi-line `TextEditor`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PromptEditorFocus {
    Name,
    Tags,
    Body,
}

impl PromptEditorFocus {
    /// The next field in the Name → Tags → Body → Name cycle.
    pub fn next(self) -> Self {
        match self {
            PromptEditorFocus::Name => PromptEditorFocus::Tags,
            PromptEditorFocus::Tags => PromptEditorFocus::Body,
            PromptEditorFocus::Body => PromptEditorFocus::Name,
        }
    }

    /// The previous field in the cycle (reverse of `next`).
    pub fn prev(self) -> Self {
        match self {
            PromptEditorFocus::Name => PromptEditorFocus::Body,
            PromptEditorFocus::Tags => PromptEditorFocus::Name,
            PromptEditorFocus::Body => PromptEditorFocus::Tags,
        }
    }
}

/// Create/edit dialog for a user template. `editing_id` is `None` for a
/// new template. `return_to` is where to land after save/cancel — the
/// picker it was opened from, or the underlying `Viewing` mode when
/// saved straight from the compose box.
pub struct PromptEditorState {
    pub editing_id: Option<String>,
    /// Where the template lives; determines which store/file is written on save.
    pub editing_source: crate::prompt_library::PromptSource,
    /// Original template for config-file sources (Project/Global/Worktree),
    /// preserving id/description/placeholders across edits.
    pub original_template: Option<crate::prompt_library::PromptTemplate>,
    pub name: String,
    /// Raw comma/space-separated tag input; parsed into the template's
    /// `tags` on save (see `prompt_library::parse_tags`).
    pub tags: String,
    pub focus: PromptEditorFocus,
    pub editor: TextEditor,
    pub return_to: Box<AppMode>,
    /// Where a save will land, for the editor's destination hint: the
    /// SQLite store for `User`, the relevant `.amf/config.json` otherwise.
    /// `None` when unresolvable (no project context, or the test store).
    pub dest_path: Option<PathBuf>,
}

/// Collects a value for each `{{slot}}` in a template before injection.
/// One field is shown at a time (`current` of `placeholders.len()`); each
/// slot's value lives in `values` (seeded with its default) so moving
/// back and forth preserves edits. `from_view` is where the rendered
/// prompt is delivered once every field is filled.
pub struct PlaceholderFillState {
    pub template: crate::prompt_library::PromptTemplate,
    /// Slots to fill, in body order. Built by `resolve_placeholders`.
    pub placeholders: Vec<crate::prompt_library::PromptPlaceholder>,
    /// One entry per placeholder; seeded with defaults, updated on nav.
    pub values: Vec<String>,
    pub current: usize,
    /// Editor for the field currently shown; reseeded from `values` on nav.
    /// Unused while the active slot is a `Select` (the option list drives it).
    pub input: TextEditor,
    /// Highlighted option index for a `Select` slot; ignored otherwise.
    pub select_index: usize,
    /// Whether the user has turned vim on for the fill fields. Persisted on
    /// the state (not the editor) so the choice survives `enter()` rebuilding
    /// `input` when moving between slots. Only applies to multi-line slots.
    pub vim_enabled: bool,
    pub from_view: Option<ViewState>,
}

impl PlaceholderFillState {
    pub fn current_placeholder(&self) -> Option<&crate::prompt_library::PromptPlaceholder> {
        self.placeholders.get(self.current)
    }

    /// Whether the active field accepts newlines (Enter inserts a line break
    /// rather than advancing to the next slot).
    pub fn current_is_multiline(&self) -> bool {
        matches!(
            self.current_placeholder().map(|p| &p.kind),
            Some(crate::prompt_library::PlaceholderKind::MultiLine { .. })
        )
    }

    /// Whether the active slot is a `Select` (choose from a fixed option list).
    pub fn is_select(&self) -> bool {
        matches!(
            self.current_placeholder().map(|p| &p.kind),
            Some(crate::prompt_library::PlaceholderKind::Select { .. })
        )
    }

    /// The options for the active slot, or an empty slice when it isn't a
    /// `Select`.
    pub fn current_options(&self) -> &[String] {
        match self.current_placeholder().map(|p| &p.kind) {
            Some(crate::prompt_library::PlaceholderKind::Select { options }) => options.as_slice(),
            _ => &[],
        }
    }

    /// Move to slot `idx`: reseed the editor from its stored value and point
    /// `select_index` at that value's position in the options (0 otherwise).
    pub fn enter(&mut self, idx: usize) {
        self.current = idx;
        let value = self.values.get(idx).cloned().unwrap_or_default();
        let is_multiline = matches!(
            self.placeholders.get(idx).map(|p| &p.kind),
            Some(crate::prompt_library::PlaceholderKind::MultiLine { .. })
        );
        self.select_index = match self.placeholders.get(idx).map(|p| &p.kind) {
            Some(crate::prompt_library::PlaceholderKind::Select { options }) => {
                options.iter().position(|o| o == &value).unwrap_or(0)
            }
            _ => 0,
        };
        // Vim applies only to multi-line slots; single-line/select slots use
        // Enter to advance, so a plain editor keeps that behaviour intact.
        self.input = if self.vim_enabled && is_multiline {
            TextEditor::with_vim(value)
        } else {
            TextEditor::new(value)
        };
    }

    /// Toggle vim on the active multi-line field, remembering the choice for
    /// later slots. No-op (and reports `false`) on non-multi-line slots, where
    /// vim would hijack Enter's "advance field" behaviour.
    pub fn toggle_input_vim(&mut self) -> bool {
        if !self.current_is_multiline() {
            return false;
        }
        self.input.toggle_vim();
        self.vim_enabled = self.input.vim_mode().is_some();
        true
    }

    /// Record the active slot's value into `values`: the chosen option for a
    /// `Select`, the editor text otherwise.
    pub fn commit_current(&mut self) {
        let value = match self.current_placeholder().map(|p| &p.kind) {
            Some(crate::prompt_library::PlaceholderKind::Select { options }) => {
                options.get(self.select_index).cloned().unwrap_or_default()
            }
            _ => self.input.text().to_string(),
        };
        if let Some(slot) = self.values.get_mut(self.current) {
            *slot = value;
        }
    }

    /// Highlight the next option (wrapping) for a `Select` slot.
    pub fn select_next(&mut self) {
        let len = self.current_options().len();
        if len > 0 {
            self.select_index = (self.select_index + 1) % len;
        }
    }

    /// Highlight the previous option (wrapping) for a `Select` slot.
    pub fn select_prev(&mut self) {
        let len = self.current_options().len();
        if len > 0 {
            self.select_index = self.select_index.checked_sub(1).unwrap_or(len - 1);
        }
    }
}

pub struct HelpState {
    pub from_view: Option<ViewState>,
    pub scroll_offset: usize,
}

/// A search-as-you-type picker over the workspace's agent skills, launched
/// from a prompt-editing surface (the prompt editor body or a text fill
/// field). Selecting an entry inserts its `/skill-name` invocation at the
/// editor cursor; `return_to` holds the editing mode to restore afterwards.
pub struct SkillPickerState {
    /// All available skills (global + project), name-sorted.
    pub skills: Vec<ComposeCommandEntry>,
    /// Indices into `skills` matching the current query, best match first.
    pub filtered: Vec<usize>,
    pub query: String,
    pub selected: usize,
    /// The editing mode to return to on select/cancel — `PromptEditor` or
    /// `PlaceholderFill`. Boxed because `AppMode` is large.
    pub return_to: Box<AppMode>,
}

impl SkillPickerState {
    /// The currently highlighted skill, if any survive the filter.
    pub fn selected_skill(&self) -> Option<&ComposeCommandEntry> {
        self.filtered
            .get(self.selected)
            .and_then(|idx| self.skills.get(*idx))
    }
}

/// Transient state while a PR's comments are being fetched off the UI thread.
#[derive(Debug, Clone)]
pub struct PrReviewLoadState {
    /// Working directory of the feature whose PR we're reviewing.
    pub workdir: PathBuf,
    /// The resolved PR being loaded.
    pub pr: crate::github::PrRef,
    /// Usage snapshots carried through a manual refresh so refreshing comments
    /// does not restart the current triage-visit tally.
    pub usage_baselines: HashMap<TokenUsageSource, SessionTokenUsage>,
}

/// Manual PR-number override prompt: shown when the branch has no detectable
/// open PR (or the user wants to review a different one). Collects a number,
/// then resolves it via `gh pr view <n>` and starts the comment fetch.
#[derive(Debug, Clone)]
pub struct PrNumberPromptState {
    /// Working directory of the feature whose PR we're reviewing.
    pub workdir: PathBuf,
    /// Digits typed so far.
    pub input: String,
    /// Last resolve failure, shown inline so the user can correct and retry.
    pub error: Option<String>,
}

/// PR picker: a selectable list of the repo's pull requests, so the user can
/// open a PR for review without knowing its number. Reached when the branch has
/// no auto-detectable PR, or on demand from PR Triage to switch PRs. The
/// manual number prompt stays one keypress away (`#`).
#[derive(Debug, Clone)]
pub struct PrPickerState {
    /// Working directory of the feature whose repo we're listing PRs for.
    pub workdir: PathBuf,
    /// The fetched PR rows (newest-updated first).
    pub entries: Vec<crate::github::PrListEntry>,
    /// Index of the highlighted row.
    pub selected: usize,
    /// When true the list includes closed/merged PRs (`gh pr list --state all`);
    /// otherwise open-only. Toggled with `a`.
    pub include_closed: bool,
    /// Last fetch/resolve failure, shown inline.
    pub error: Option<String>,
    /// When `Some`, the lookback-bootstrap depth picker (`b`) is open over the
    /// picker.
    pub bootstrap_pick: Option<BootstrapPickState>,
    /// When `Some`, the review-memory compact confirm overlay (`c`) is open
    /// over the picker.
    pub compact_confirm: Option<CompactConfirmState>,
    /// The logged-in `gh` user's login, when resolvable — used to highlight
    /// the user's own PRs in the row rendering. `None` if unresolved/failed.
    pub current_user: Option<String>,
}

/// Depth picker for the review-memory lookback bootstrap (`b` in the PR
/// picker): pick how many recent merged/closed PRs to learn from before
/// running the fetch + distill pass.
#[derive(Debug, Clone)]
pub struct BootstrapPickState {
    /// Index into [`crate::app::pr_review::BootstrapDepth::ALL`].
    pub selected: usize,
}

/// Full-screen progress view for the lookback bootstrap's background fetch +
/// distill pass, entered once a depth is confirmed.
#[derive(Debug, Clone)]
pub struct BootstrapRunState {
    /// The PR picker to return to on completion or cancel.
    pub origin: PrPickerState,
    pub depth: crate::app::pr_review::BootstrapDepth,
    pub stage: crate::app::pr_review::BootstrapStage,
}

/// Confirm overlay for the review-memory compact pass (`c` in the PR picker):
/// shows how many findings are currently in the doc before spending an agent
/// pass to merge near-duplicates and prune stale ones (Epic E "prevent
/// review-memory rot").
#[derive(Debug, Clone)]
pub struct CompactConfirmState {
    /// Bullet count in the doc as it stands, read synchronously when the
    /// overlay opens (a local file read — cheap enough not to background).
    pub existing_findings: usize,
}

/// Full-screen progress view for the review-memory compact pass's background
/// read + rewrite, entered once the confirm overlay is accepted.
#[derive(Debug, Clone)]
pub struct CompactRunState {
    /// The PR picker to return to on completion or cancel.
    pub origin: PrPickerState,
    /// Resolved path of the review-memory doc being compacted, carried
    /// through from confirm so the poll's success path doesn't need to
    /// re-resolve it (a second `repo_root` lookup) once the background
    /// thread reports back.
    pub path: PathBuf,
    pub stage: crate::app::pr_review::CompactStage,
}

/// Full-screen review of the compact pass's proposed replacement doc, entered
/// once the background run finishes. Unlike [`append_finding`]-backed dialogs
/// (`M`, the bootstrap), this proposes rewriting the *entire* doc, so nothing
/// is written until the user explicitly confirms here — editable first, same
/// as every other write in this pane.
///
/// [`append_finding`]: crate::app::review_memory::append_finding
#[derive(Debug, Clone)]
pub struct CompactReviewState {
    /// The PR picker to return to on write or discard.
    pub origin: PrPickerState,
    /// Resolved path of the review-memory doc this will write to.
    pub path: PathBuf,
    /// Bullet count in the doc before compacting, for the "N -> M" summary.
    pub original_findings: usize,
    /// Bullet count in the agent's proposed replacement, for the same summary.
    pub proposed_findings: usize,
    /// The proposed replacement text, editable before writing.
    pub editor: TextEditor,
    /// True while keystrokes go to the editor (`e` to enter); false in the
    /// confirm view (`⏎`/`w` write / `e` edit / `esc` discard).
    pub editing: bool,
    /// Scroll offset, in wrapped visual rows, for docs taller than the screen.
    pub scroll: usize,
    /// Request that the next render scroll the cursor back into view. Mirrors
    /// [`FixConfirmState::sync_to_cursor`].
    pub sync_to_cursor: bool,
    /// Last write failure, shown inline so it's recoverable without losing
    /// the edited content.
    pub error: Option<String>,
}

/// Full-screen progress view for the AI PR review's background diff-fetch +
/// review pass (`A`), entered from the AI Review pane.
#[derive(Debug, Clone)]
pub struct AiReviewRunProgress {
    pub stage: crate::app::ai_review::AiReviewStage,
    /// Wall-clock start for a live elapsed timer. This state is intentionally
    /// not persisted; an in-flight headless process belongs to this AMF
    /// process and cannot be resumed after restart.
    pub started_at: std::time::Instant,
    /// Latest sanitized activity label from the harness's structured stream.
    pub activity: Option<String>,
    /// Final token usage, when the harness reports it before completion.
    pub usage: Option<(u64, u64)>,
}

#[derive(Debug, Clone)]
pub struct AiReviewRunState {
    /// The AI Review pane to return to on completion or cancel (dialogs
    /// cleared before stashing, matching the PR Triage `P`/`f` stash
    /// convention).
    pub origin: AiReviewState,
    pub progress: AiReviewRunProgress,
}

/// State for the full-screen AI Review pane — AMF's own review of a PR's
/// diff, independent of PR Triage (see `crate::app::ai_review`'s module doc
/// for why this is a separate workflow rather than bolted onto triage).
#[derive(Debug, Clone)]
pub struct AiReviewState {
    /// Working directory of the feature whose PR this reviews.
    pub workdir: PathBuf,
    /// The PR being reviewed.
    pub pr: crate::github::PrRef,
    /// Findings from the most recent `A` run (or loaded from `ai_review_cache`
    /// on entry), in generation order.
    pub findings: Vec<crate::app::ai_review::AiReviewFinding>,
    /// Overall one-to-three sentence review summary generated in the same
    /// pass as `findings`, and loaded from the same cache row. Older cache
    /// entries may not have one.
    pub summary: Option<String>,
    /// Index into `findings` of the highlighted finding.
    pub selected: usize,
    /// Scroll offset (in lines) for the detail pane of the selected finding.
    pub detail_scroll: usize,
    /// Number of lines the detail pane rendered on the last frame, so the
    /// scroll clamp bounds against what was actually shown.
    pub detail_content_lines: usize,
    /// Record of the most recent `A` run (success/error/finding-count),
    /// shown as a header badge so a review that already ran doesn't look
    /// identical to one that never did.
    pub last_run: Option<crate::app::ai_review::AiReviewRun>,
    /// Harness chosen for this pane's `A` runs, picked once via `harness_pick`
    /// and remembered for the rest of the visit.
    pub harness: Option<AgentKind>,
    /// Single-select picker shown before the first `A` run in this pane.
    pub harness_pick: Option<AiHarnessPickState>,
    /// Harness in effect when the current harness-pick "chain" started —
    /// set the first time this pane's picker steps back from the model
    /// picker to the harness picker, and left untouched by any further
    /// back-and-forth within the same chain (cleared once a review actually
    /// starts). Lets [`App::accept_ai_review_harness_pick`] detect a switch
    /// away from the *original* harness even after the user backs out and
    /// re-confirms an already-switched-to harness, so `AppConfig::review_model`
    /// (which may only be valid for the original harness) isn't reseeded as
    /// an incompatible model for the new one. See `AiHarnessPickState::previous_harness`.
    pub harness_pick_origin: Option<AgentKind>,
    /// Model chosen for this pane's `A` runs, picked once via `model_pick`
    /// right after the harness. `None` means "use the default" — either the
    /// picker hasn't run yet (see `model_picked`) or the user explicitly
    /// chose the "Default" row.
    pub model: Option<String>,
    /// Whether the model has been picked (or auto-skipped, e.g. for Pi) yet
    /// this pane visit.
    pub model_picked: bool,
    /// Single-select picker shown once per pane, right after the harness.
    pub model_pick: Option<AiModelPickState>,
    /// When `Some`, the selected finding's body is open for editing (`e`).
    pub finding_editor: Option<TextEditor>,
    /// When `Some`, the post-to-GitHub confirm dialog is open (`W`).
    pub post_confirm: Option<AiReviewPostConfirmState>,
}

/// State for the full-screen PR Triage pane.
#[derive(Debug, Clone)]
pub struct PrReviewState {
    /// Working directory of the feature whose PR we're reviewing. Used by the
    /// manual-refresh action (`r`) to re-resolve and re-fetch the PR.
    pub workdir: PathBuf,
    /// The fetched, normalized review.
    pub review: crate::app::pr_review::PrReview,
    /// Index into `review.comments` of the highlighted comment.
    pub selected: usize,
    /// Scroll offset (in lines) for the detail pane of the selected comment.
    pub detail_scroll: usize,
    /// Number of lines the detail pane rendered on the last frame. The renderer
    /// (`ui::dialogs::pr_review`) writes this each draw so the scroll clamp can
    /// bound against what was actually shown, rather than a hand-synced estimate
    /// that drifts as the detail layout (Markdown, dividers) changes.
    pub detail_content_lines: usize,
    /// When true, comments already resolved on GitHub are hidden from the list.
    pub hide_resolved: bool,
    /// Order the comment list is shown in (cycled with `o`), independent of
    /// `hide_resolved`.
    pub sort_mode: crate::app::pr_review::PrSortMode,
    /// Which agent session "fix" prompts are injected into. Chosen once, via
    /// `harness_pick`, before the first `f`/`B` of a pane visit.
    pub fix_target: crate::app::pr_review::FixTarget,
    /// Whether `fix_target` (and, for the dedicated case, `review_harness`)
    /// has already been explicitly resolved for this pane visit — either by
    /// the user confirming `harness_pick`, or because a dedicated session
    /// already existed on entry so there was nothing to ask. Prevents
    /// re-opening the picker on every subsequent `f`/`B`.
    pub fix_target_picked: bool,
    /// Token totals already present when each fix-target session joined this
    /// visit to the PR pane. Current totals minus these snapshots are the live
    /// "this visit" tally; a target created after the pane opened has no
    /// baseline, so all of its usage belongs to the visit.
    pub usage_baselines: HashMap<TokenUsageSource, SessionTokenUsage>,
    /// Harness chosen for the dedicated triage session, picked once before the
    /// first fix is injected and reused for the rest of the PR. `None` until the
    /// user picks (or when the dedicated session already exists / isn't the
    /// target). Lets PR triage run on a different harness than the feature's
    /// working session.
    pub review_harness: Option<AgentKind>,
    /// When `Some`, the fix-target picker is open over the pane: the user is
    /// choosing whether fixes go to the feature's existing live session or a
    /// dedicated triage session (and, for the latter, which harness) before
    /// the first fix/batch is injected. Replaces the old standalone `t`
    /// toggle — the choice is made once, at the point it's needed.
    pub harness_pick: Option<HarnessPickState>,
    /// When `Some`, the fix confirm/edit dialog is open over the pane, holding
    /// the assembled (and editable) prompt awaiting the user's approval before
    /// it is injected into the agent session.
    pub fix_confirm: Option<FixConfirmState>,
    /// Whether the fix confirm/edit dialog opens with the vim keymap. Persisted
    /// on the pane (not the editor, which is rebuilt on each `f`) so the choice
    /// survives reopening the dialog for another comment — the same approach as
    /// [`PlaceholderFillState::vim_enabled`].
    pub fix_vim_enabled: bool,
    /// When `Some`, the reply-kind picker (`R`) is open: choosing between a
    /// "Done" report and a "not needed" explanation before the reply dialog
    /// itself opens.
    pub reply_kind_pick: Option<ReplyKindPickState>,
    /// When `Some`, the "Mark" picker (`m`) is open: choosing Done / Skip /
    /// Resolve-on-GitHub for the selected comment.
    pub mark_pick: Option<MarkPickState>,
    /// When `Some`, the reply dialog is open over the pane: an AI-drafted,
    /// editable reply awaiting the user's approval before it is posted to GitHub.
    pub reply: Option<ReplyState>,
    /// When `Some`, the "add to memory" dialog is open over the pane: the
    /// selected comment's finding, editable, awaiting the user's approval
    /// before it's appended to the review-memory doc.
    pub memory_add: Option<MemoryAddState>,
    /// Comment ids marked (with `space`) for a combined batch fix via `B`.
    /// Keyed by id (not index) so marks survive the hide-resolved filter
    /// shifting the visible rows. Cleared once the batch is injected.
    pub marked: std::collections::HashSet<u64>,
    /// Set while the combined-batch flow (`B`) is waiting on the harness picker:
    /// after the user picks the review harness, the continuation opens the
    /// combined-batch confirm dialog instead of the single-comment one. Cleared
    /// when the picker is confirmed or cancelled.
    pub pending_batch: bool,
    /// The branch actually checked out in `workdir`, snapshotted when the pane
    /// was entered/refreshed (`WorktreeManager::current_branch`). `f`/`B` fix
    /// injection reads files from this workdir regardless of which PR is being
    /// triaged (`G`/`g`/`#` allow picking *any* PR in the repo), so when this
    /// doesn't match `review.pr.head_ref` a fix would silently land on the
    /// wrong branch — see [`Self::branch_mismatch`]. `None` when the branch
    /// couldn't be determined (e.g. detached HEAD).
    pub checked_out_branch: Option<String>,
    /// Completed AI-review findings for this exact PR/head SHA that are still
    /// publishable. Loaded from `ai_review_cache` on entry and kept in sync as
    /// the linked AI Review is generated, skipped, or posted.
    pub pending_ai_review_findings: usize,
}

/// Identity of the PR Triage refresh started after a successful AI Review
/// post. Kept outside `AppMode` so the refresh can update a stashed triage
/// pane while the user remains in AI Review.
#[derive(Debug, Clone)]
pub struct AiReviewTriageRefresh {
    pub workdir: PathBuf,
    pub pr: crate::github::PrRef,
}

/// Confirm/edit dialog for posting the kept AI-review findings to GitHub as a
/// real review (`W`). Built once from every not-skipped, not-yet-published
/// finding; `⏎` posts as-is. Only the summary body is editable — the
/// per-finding inline comment bodies are the AI's own text, vetted by
/// skipping (`s`) rather than hand-edited here.
#[derive(Debug, Clone)]
pub struct AiReviewPostConfirmState {
    /// Inline review comments built from the anchored findings.
    pub inline: Vec<crate::github::PrReviewComment>,
    pub editor: TextEditor,
    pub editing: bool,
    /// Last post failure, shown inline so a recoverable error (e.g. GitHub
    /// rejecting the review because a finding no longer matches the current
    /// diff) doesn't require leaving the dialog to notice — `show_error`
    /// unconditionally resets `self.mode` to `Normal` outside of
    /// `Normal`/`Help`/`Viewing`, so the pane is restored with this set
    /// rather than losing the dialog entirely.
    pub error: Option<String>,
}

/// A [`PrReviewState`] stashed while the user is watching the linked fix
/// session (`P` from PR Triage), so `leader+P` can jump straight back
/// to the exact comment/scroll/dialog state without re-fetching. `session`
/// and `window` identify the tmux target the stash was jumped *to*, so the
/// restore only fires from that same session's view — a stash left behind
/// after navigating elsewhere is not mistaken for a different PR's pane.
#[derive(Debug, Clone)]
pub struct PrReviewReturn {
    pub session: String,
    pub window: String,
    pub state: PrReviewState,
}

/// Single-select fix-target picker shown before the first fix/batch of a PR
/// Triage pane visit: whether fixes go to the feature's existing live
/// session, or a dedicated triage session pinned to a specific harness.
/// Replaces the old standalone `t` toggle — the choice is made once, at the
/// point it's needed, instead of living as an always-on key. Highlights the
/// dedicated-review row for the project's preferred agent by default.
#[derive(Debug, Clone)]
pub struct HarnessPickState {
    /// The rows to choose from: the existing-live option, plus one row per
    /// allowed agent for a dedicated session.
    pub rows: Vec<crate::app::pr_review::FixTargetPickRow>,
    /// Index into `rows` of the highlighted choice.
    pub selected: usize,
}

/// Harness picker for the paid, headless `A` review pass. An unavailable CLI
/// leaves the picker open and records an actionable inline error.
#[derive(Debug, Clone)]
pub struct AiHarnessPickState {
    pub agents: Vec<AgentKind>,
    pub selected: usize,
    pub error: Option<String>,
    /// The harness-pick chain's original harness (`AiReviewState::harness_pick_origin`),
    /// carried into this picker so a confirm can tell whether the choice has
    /// actually diverged from where the chain started — not just from the
    /// harness shown on the immediately preceding screen. `None` on the
    /// initial harness step. Used to avoid seeding one harness's model choice
    /// (or the globally configured default model) into a different harness's
    /// rebuilt model picker.
    pub previous_harness: Option<AgentKind>,
}

/// One row of the model picker: either "use the default", a known-good
/// preset `--model` value, or "type your own".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelPickRow {
    /// No explicit model — the harness's own default (or `AppConfig::review_model`
    /// as an underlying override) applies.
    Default,
    /// A verified alias/name for the chosen harness (e.g. Claude's `"sonnet"`).
    Preset(&'static str),
    /// Free-text entry for anything not in the preset list.
    Custom,
}

/// Single-select model picker for the `A` AI review, shown once per pane
/// right after the harness is chosen. Presets are a best-effort, *verified*
/// set of model aliases for the chosen harness — currently only Claude's
/// (`sonnet`/`opus`/`haiku`/`fable`, confirmed against `claude --help`).
/// Other harnesses offer just `Default` and `Custom`, since their valid
/// model strings aren't reliably enumerable here; guessing wrong presets
/// would be worse than not offering any.
#[derive(Debug, Clone)]
pub struct AiModelPickState {
    pub rows: Vec<ModelPickRow>,
    /// Index into `rows` of the highlighted choice.
    pub selected: usize,
    /// Free-text buffer for the `Custom` row, live only while `editing_custom`.
    pub custom_input: String,
    /// True while keystrokes go to `custom_input` (opened by `⏎`/`e` on the
    /// `Custom` row); false in the plain list-navigation view.
    pub editing_custom: bool,
}

/// Single-select picker shown by `R` before the reply dialog itself: choose
/// between a "Done in `<sha>`" report and a "not needed" explanation. Once
/// confirmed, routes into the same [`ReplyState`] flow either kind already
/// used — this is purely a UI step in front of it, replacing the old
/// separate `R`/`n` top-level keys.
#[derive(Debug, Clone)]
pub struct ReplyKindPickState {
    /// Index into `ReplyKind::ALL` of the highlighted choice.
    pub selected: usize,
}

/// Single-select picker shown by `m` ("Mark"): choose between marking the
/// selected comment `Done` (local), `Skip` (local), or toggling its GitHub
/// review thread's resolved state. Replaces the old separate `m`/`s`/`x`
/// top-level keys with one entry point; applying a row is immediate (no
/// further confirm step, matching the original single-key behavior) since
/// none of the three actions need editable text.
#[derive(Debug, Clone)]
pub struct MarkPickState {
    /// Index into `MarkAction::ALL` of the highlighted choice.
    pub selected: usize,
}

/// Reply dialog for one comment. Replies are contextual, not free-form: either
/// a "Done in `<sha>`." report of a completed fix or a "not needed" explanation.
/// The seeded body is editable; nothing is posted until the user confirms.
#[derive(Debug, Clone)]
pub struct ReplyState {
    /// GitHub id of the comment being replied to (resolves the post target).
    pub comment_id: u64,
    /// Which contextual reply this is (drives the seed, title, and the triage
    /// outcome applied on post).
    pub kind: crate::app::pr_review::ReplyKind,
    /// The reply body, editable before posting.
    pub editor: TextEditor,
    /// Whether the initial body came back from an agent fix session. Agent
    /// drafts receive AI-authorship attribution; deterministic templates and
    /// user-written not-needed replies receive channel-only AMF attribution.
    /// Only ever `true` for [`super::pr_review::ReplyKind::Done`] — see
    /// [`super::pr_review::App::open_reply`].
    pub agent_drafted: bool,
    /// The exact body the editor was seeded with when the dialog opened.
    /// Compared against the current editor text at post time: if the user has
    /// changed it, the draft is no longer purely the agent's own words, so
    /// `agent_drafted` attribution no longer applies (see
    /// [`super::pr_review::reply_effective_agent_drafted`]).
    pub original_seed: String,
    /// True while keystrokes go to the editor (`e` to enter); false in the
    /// confirm view (`⏎` post / `e` edit / `esc` cancel).
    pub editing: bool,
}

/// "Add to memory" dialog (`M`): appends the selected comment's distilled
/// finding to the review-findings memory doc
/// (`review_memory::append_finding`). Mirrors [`ReplyState`]'s edit/confirm
/// split, plus a category cycled with `Tab` in the confirm view.
#[derive(Debug, Clone)]
pub struct MemoryAddState {
    /// GitHub id of the comment the finding is drawn from.
    pub comment_id: u64,
    /// Index into `crate::app::pr_review::MEMORY_CATEGORIES`, cycled with `Tab`.
    pub category: usize,
    /// The finding text, editable before it's appended.
    pub editor: TextEditor,
    /// True while keystrokes go to the editor (`e` to enter); false in the
    /// confirm view (`⏎` append / `e` edit / `Tab` cycle category / `esc`
    /// cancel).
    pub editing: bool,
}

/// Confirm/edit dialog for a fix prompt: shows the exact text that will be
/// injected (token principle #3 — no file contents), with a `~N tokens`
/// preview, before it reaches the agent. The prompt is editable so the user can
/// tweak it before sending.
#[derive(Debug, Clone)]
pub struct FixConfirmState {
    /// The assembled fix prompt, editable before injection.
    pub editor: TextEditor,
    /// True while keystrokes go to the editor (`e` to enter); false in the
    /// default confirm view (`⏎` inject / `e` edit / `esc` cancel).
    pub editing: bool,
    /// Scroll offset, in wrapped visual rows, for prompts taller than the
    /// dialog. Clamped to the rendered content each frame.
    pub scroll: usize,
    /// Request that the next render scroll the cursor back into view. Set on
    /// edits / cursor moves and cleared once applied; an explicit scroll key
    /// clears it so the user can scroll away from the cursor.
    pub sync_to_cursor: bool,
    /// When `Some`, this dialog holds a **combined** batch prompt built from
    /// several marked comments (the `B` flow) rather than a single comment's
    /// fix. The vector is the ids of every comment included in the batch;
    /// injecting marks all of them `Fixing` and clears the marked set. `None`
    /// for an ordinary single-comment fix (only the selected comment is marked).
    pub batch: Option<Vec<u64>>,
    /// Per-comment correlation ids embedded in the prompt's `amf reply-draft`
    /// handoff commands. They become authoritative only when the user confirms
    /// injection, at which point AMF invalidates any older stored draft.
    pub reply_draft_requests: Vec<crate::app::pr_review::ReplyDraftRequest>,
}

impl PrReviewState {
    pub fn selected_comment(&self) -> Option<&crate::app::pr_review::PrComment> {
        self.review.comments.get(self.selected)
    }

    /// The checked-out branch when it's known and doesn't match the PR being
    /// triaged — `None` when they match, or either side is unknown (an empty
    /// `head_ref` means the PR was resolved before this field existed; a
    /// `None` `checked_out_branch` means detached HEAD or the branch lookup
    /// failed). Surfaced as a pane-header warning and inside the fix confirm
    /// dialog, since fix injection reads files from `workdir` regardless of
    /// which PR is loaded.
    pub fn branch_mismatch(&self) -> Option<&str> {
        branch_mismatch(&self.review.pr.head_ref, self.checked_out_branch.as_deref())
    }

    /// Indices into `review.comments` that pass the current filter, ordered by
    /// `sort_mode`. With `hide_resolved` on, GitHub-resolved comments are
    /// dropped.
    pub fn visible_indices(&self) -> Vec<usize> {
        let indices = self
            .review
            .comments
            .iter()
            .enumerate()
            .filter(|(_, c)| !self.hide_resolved || !c.is_resolved)
            .map(|(i, _)| i)
            .collect();
        self.sort_indices(indices)
    }

    /// Every comment index (ignoring `hide_resolved`) in `sort_mode` order.
    /// Used to find a hidden selection's nearest visible neighbor when a
    /// filter change hides it — the filter can't change relative order, only
    /// remove from it, so this is the same order `visible_indices` would use.
    pub(crate) fn all_sorted_indices(&self) -> Vec<usize> {
        self.sort_indices((0..self.review.comments.len()).collect())
    }

    /// If `selected` is currently hidden by `hide_resolved`, snap it to the
    /// nearest remaining visible comment in sort order (forward first, then
    /// backward, then the first visible comment). No-op when `selected` is
    /// already visible, or nothing is visible at all. Shared by the `x`
    /// toggle and by a PR Triage refresh, either of which can newly hide the
    /// selected comment (resolved on GitHub, in the toggle case; refreshed
    /// into a resolved state, in the refresh case).
    pub fn snap_selection_to_visible(&mut self) {
        let visible = self.visible_indices();
        if visible.is_empty() || visible.contains(&self.selected) {
            return;
        }
        let order = self.all_sorted_indices();
        let pos = order.iter().position(|&i| i == self.selected);
        let snapped = pos
            .and_then(|p| order[p..].iter().find(|i| visible.contains(i)))
            .or_else(|| pos.and_then(|p| order[..p].iter().rev().find(|i| visible.contains(i))))
            .copied()
            .unwrap_or(visible[0]);
        self.selected = snapped;
        self.detail_scroll = 0;
    }

    /// Apply `sort_mode` to a set of comment indices. Stable, so ties keep
    /// their relative (fetch) order.
    fn sort_indices(&self, mut indices: Vec<usize>) -> Vec<usize> {
        use crate::app::pr_review::PrSortMode;
        match self.sort_mode {
            PrSortMode::FetchOrder => {}
            PrSortMode::ByFile => indices.sort_by(|&a, &b| {
                let path = |i: usize| self.review.comments[i].path.as_deref();
                match (path(a), path(b)) {
                    (Some(pa), Some(pb)) => pa.cmp(pb),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            }),
            PrSortMode::ByAuthor => indices.sort_by(|&a, &b| {
                self.review.comments[a]
                    .author
                    .cmp(&self.review.comments[b].author)
            }),
            PrSortMode::HumansFirst => indices.sort_by_key(|&i| self.review.comments[i].is_bot),
            PrSortMode::Conversations => indices.sort_by_key(|&i| {
                matches!(
                    self.review.comments[i].kind,
                    crate::app::pr_review::CommentKind::Conversation
                )
            }),
        }
        indices
    }

    /// Under [`PrSortMode::Conversations`], the position within
    /// [`Self::visible_indices`] where the conversation-comment section
    /// begins — `None` when not in that mode, or when the visible list has no
    /// conversation comments (nothing to separate) or is *entirely*
    /// conversation comments (no code-anchored section to divide from).
    /// `draw_comment_list` uses this to insert a section divider rather than
    /// silently reordering the list.
    pub fn conversation_section_start(&self) -> Option<usize> {
        if self.sort_mode != crate::app::pr_review::PrSortMode::Conversations {
            return None;
        }
        let visible = self.visible_indices();
        let start = visible.iter().position(|&i| {
            matches!(
                self.review.comments[i].kind,
                crate::app::pr_review::CommentKind::Conversation
            )
        })?;
        (start > 0).then_some(start)
    }

    /// Number of comments hidden by the resolved filter (0 when showing all).
    pub fn hidden_resolved_count(&self) -> usize {
        if !self.hide_resolved {
            return 0;
        }
        self.review
            .comments
            .iter()
            .filter(|c| c.is_resolved)
            .count()
    }
}

/// Free function behind [`PrReviewState::branch_mismatch`] — pulled out so it's
/// testable without constructing a full `PrReviewState`. `None` when the
/// branches match, or when either side is unknown (empty `head_ref` from a
/// pre-existing cache row, or no `checked_out_branch` — detached HEAD / lookup
/// failure).
fn branch_mismatch<'a>(pr_head_ref: &str, checked_out_branch: Option<&'a str>) -> Option<&'a str> {
    let checked_out = checked_out_branch?;
    if pr_head_ref.is_empty() || pr_head_ref == checked_out {
        return None;
    }
    Some(checked_out)
}

/// What an active TODOs inline edit targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TodoEditTarget {
    /// Adding a brand-new item (its title).
    New,
    /// Editing the selected item's title.
    Title,
    /// Editing the selected item's notes/detail body (multi-line).
    Notes,
    /// Editing the list's free-form scratchpad note (persisted in the
    /// legacy `carry_over` column).
    Scratchpad,
}

/// An in-progress inline edit within the TODOs overlay.
pub struct TodoEditor {
    pub target: TodoEditTarget,
    pub editor: TextEditor,
}

/// State for the native TODOs overlay (`AppMode::Todos`). Holds the loaded
/// per-project list and its items plus the cursor and any in-progress edit.
pub struct TodoViewState {
    /// Project that owns the list (and whose `S` picker created the session).
    pub project_id: String,
    /// Project / host-feature indices the TODOs session lives under, used to
    /// resolve the session for selection on close and to host the list.
    pub pi: usize,
    pub fi: usize,
    /// Display labels for the header.
    pub project_name: String,
    pub feature_name: String,
    /// The loaded list (carry-over note, id). `None` when no DB is available
    /// (e.g. tests) — the overlay then shows an empty list.
    pub list: Option<crate::db::todos::TodoList>,
    /// Items in display order (open first, then by sort_order).
    pub todos: Vec<crate::db::todos::Todo>,
    /// Cursor into `todos`.
    pub selected: usize,
    /// Vertical scroll offset into the list area.
    pub scroll_offset: usize,
    /// Active inline edit, if any (add/edit title/notes/carry-over).
    pub editor: Option<TodoEditor>,
    /// Set when a delete is awaiting y/n confirmation.
    pub pending_delete: bool,
}

/// Single-line quick-capture of a TODO from inside a session view. The typed
/// title is appended to the current project's list, auto-creating the list (and
/// a TODOs session under the current feature) when the project has none yet.
/// `view` is the session view to return to on commit/cancel.
pub struct TodoQuickCaptureState {
    pub view: ViewState,
    /// Name of the project the TODO will be added to (shown in the dialog).
    pub project_name: String,
    /// The title being typed.
    pub input: String,
}

/// Prompt shown when the feature that hosts a project's TODO list is deleted
/// while the project survives (see `docs/backlog/feature-todos-plan.md`, Epic 1).
/// The user chooses which surviving feature re-homes the list, or deletes it.
pub struct TodosHostReassignState {
    /// Project whose list is being re-homed.
    pub project_name: String,
    /// Name of the just-deleted host feature (shown in the prompt).
    pub deleted_feature_name: String,
    /// `todo_lists.id` of the orphaned list.
    pub list_id: String,
    /// Surviving features the list can be re-homed onto: `(name, feature_id)`.
    pub candidates: Vec<(String, String)>,
    /// Selected option index: `0..candidates.len()` re-homes onto that feature;
    /// `== candidates.len()` deletes the list.
    pub selected: usize,
    /// Number of TODOs in the list (shown so the user knows what's at stake).
    pub todo_count: usize,
}

pub enum AppMode {
    Normal,
    Todos(TodoViewState),
    TodoQuickCapture(TodoQuickCaptureState),
    /// Re-home or delete a project's TODO list after its host feature is deleted.
    TodosHostReassign(TodosHostReassignState),
    CreatingProject(CreateProjectState),
    CreatingFeature(CreateFeatureState),
    #[allow(dead_code)] // Entered by the next Epic 1 feature-launch integration.
    PlanInterview(PlanInterviewState),
    DeletingProject(String),
    DeletingFeature(String, String),
    DeletingFeatureInProgress(DeletingFeatureState),
    Viewing(ViewState),
    Help(HelpState),
    NotificationPicker(usize, Option<ViewState>),
    SessionSwitcher(super::SessionSwitcherState),
    RenamingSession(RenameSessionState),
    RenamingFeature(RenameFeatureState),
    SessionConfig(SessionConfigState),
    ProjectAgentConfig(ProjectAgentConfigState),
    BrowsingPath(Box<BrowsePathState>),
    CommandPicker(super::CommandPickerState),
    Searching(SearchState),
    NamingNewSession(NewSessionNameState),
    OpencodeSessionPicker(OpencodeSessionPickerState),
    ConfirmingOpencodeSession {
        session_id: String,
        workdir: PathBuf,
    },
    ClaudeSessionPicker(ClaudeSessionPickerState),
    ConfirmingClaudeSession {
        session_id: String,
        workdir: PathBuf,
    },
    CodexSessionPicker(CodexSessionPickerState),
    ConfirmingCodexSession {
        session_id: String,
        workdir: PathBuf,
    },
    BookmarkPicker(BookmarkPickerState),
    DiffPicker(DiffPickerState),
    DiffViewerLoading(DiffViewerState),
    DiffViewer(DiffViewerState),
    /// Prompting for a PR number when the branch has no auto-detectable PR.
    PrNumberPrompt(PrNumberPromptState),
    /// Choosing a PR from a list (or falling through to the number prompt).
    PrPicker(PrPickerState),
    /// Fetching a PR's comments off the UI thread; shows a loading frame.
    PrReviewLoading(PrReviewLoadState),
    /// Triaging a PR's comments in the full-screen PR Triage pane.
    PrReview(PrReviewState),
    /// Running the review-memory lookback bootstrap's fetch + distill pass off
    /// the UI thread; shows a loading frame with the current stage.
    ReviewMemoryBootstrapRunning(BootstrapRunState),
    /// Running the review-memory compact pass off the UI thread ("prevent
    /// review-memory rot"); shows a loading frame with the current stage.
    ReviewMemoryCompactRunning(CompactRunState),
    /// Reviewing the compact pass's proposed replacement doc before it's
    /// written — full-screen, editable, nothing written until confirmed.
    ReviewMemoryCompactReview(CompactReviewState),
    /// Reviewing/triaging findings from AMF's own AI review of a PR's diff —
    /// its own workflow, independent of `PrReview` (see `crate::app::ai_review`).
    AiReview(AiReviewState),
    /// Running the AI PR review's diff-fetch + review pass off the UI thread
    /// (`A`); shows a loading frame with the current stage.
    AiReviewRunning(AiReviewRunState),
    SteeringPrompt(SteeringPromptState),
    Compose(ComposeState),
    SessionPicker(SessionPickerState),
    DiffReviewPrompt(DiffReviewState),
    RunningHook(RunningHookState),
    HookPrompt(HookPromptState),
    LatestPrompt(LatestPromptState),
    PromptLibrary(PromptLibraryState),
    PromptEditor(PromptEditorState),
    PlaceholderFill(PlaceholderFillState),
    SkillPicker(SkillPickerState),
    ForkingFeature(ForkFeatureState),
    ThemePicker(ThemePickerState),
    SyntaxLanguagePicker(SyntaxLanguagePickerState),
    DebugLog(DebugLogState),
    MarkdownLoading(MarkdownLoadingState),
    MarkdownViewer(MarkdownViewerState),
    MarkdownFilePicker(MarkdownFilePickerState),
    CreatingBatchFeatures(CreateBatchFeaturesState),
    HarnessSetup(HarnessSetupState),
    ConfigWizard(ConfigWizardState),
    /// Choosing which harness runs a fresh dedicated session for a finished
    /// review's fixes (only shown when the dedicated fix target is selected and
    /// no review session exists yet). The feedback file is already written; this
    /// only governs where the "address the feedback" prompt is dispatched.
    ReviewHarnessPick(ReviewHarnessPickState),
}

/// Pending dispatch of a finished review's feedback to a freshly-spun-up
/// dedicated agent session, paused on the harness choice.
pub struct ReviewHarnessPickState {
    /// Project / feature indices the dedicated session is created under.
    pub pi: usize,
    pub fi: usize,
    /// The finish summary shown after the prompt is dispatched.
    pub summary: String,
    /// The feature view to return to once dispatch completes or is cancelled.
    pub from_view: ViewState,
    /// Harnesses offered to the reviewer (the project's enabled harnesses).
    pub harnesses: Vec<crate::project::AgentKind>,
    pub selected: usize,
}

#[derive(Debug, Clone)]
pub struct HarnessOption {
    pub kind: AgentKind,
    pub status: HarnessCheckStatus,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum HarnessCheckStatus {
    Unchecked,
    Checking,
    Installed,
    NotFound(String),
}

#[derive(Debug, Clone)]
pub struct HarnessSetupState {
    pub selected: usize,
    pub harnesses: Vec<HarnessOption>,
    pub is_startup: bool,
}

impl HarnessSetupState {
    pub fn new(is_startup: bool, existing: &[AgentKind]) -> Self {
        let harnesses = AgentKind::ALL
            .iter()
            .map(|kind| {
                let already_enabled = existing.contains(kind);
                HarnessOption {
                    kind: kind.clone(),
                    status: if already_enabled {
                        HarnessCheckStatus::Installed
                    } else {
                        HarnessCheckStatus::Unchecked
                    },
                    enabled: already_enabled,
                }
            })
            .collect();
        Self {
            selected: 0,
            harnesses,
            is_startup,
        }
    }

    pub fn enabled_harnesses(&self) -> Vec<AgentKind> {
        self.harnesses
            .iter()
            .filter(|h| h.enabled)
            .map(|h| h.kind.clone())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigCategory {
    CustomSessions,
    FeaturePresets,
    PlanQuestions,
    LifecycleHooks,
    Keybindings,
    AllowedAgents,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigScope {
    Global,
    Project(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConfigWizardStep {
    CategoryPicker,
    ScopePicker,
    ItemList,
    EditItem,
    ConfirmSave,
}

pub struct ConfigWizardState {
    pub step: ConfigWizardStep,
    pub category: ConfigCategory,
    pub scope: ConfigScope,
    pub selected: usize,
    pub field_focus: usize,
    pub input_mode: bool,
    pub sessions: Vec<CustomSessionConfig>,
    pub presets: Vec<FeaturePreset>,
    pub plan_questions: Vec<ConfiguredPlanQuestion>,
    pub skip_builtin_questions: Option<bool>,
    pub hooks: LifecycleHooks,
    pub keybindings: HashMap<String, char>,
    pub allowed_agents: Option<Vec<AgentKind>>,
    pub editing_index: Option<usize>,
    pub field_values: Vec<String>,
    pub field_editor: Option<ConfigWizardFieldEditor>,
    pub field_toggles: Vec<bool>,
    pub agent_toggles: Vec<bool>,
    pub agent_toggles_dirty: bool,
    pub keybinding_actions: Vec<String>,
    pub capturing_key: bool,
    pub original_json: String,
    pub modified_json: String,
    pub confirm_diff: Option<crate::diff::DiffFile>,
    pub preview_scroll: usize,
    pub project_repo: Option<PathBuf>,
    pub project_name: Option<String>,
    pub error: Option<String>,
}

pub struct ConfigWizardFieldEditor {
    pub field_index: usize,
    pub label: String,
    pub editor: TextEditor,
    pub scroll_offset: usize,
    pub sync_scroll_to_cursor: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // summary-prefetch payload, populated but not read back yet
pub struct PendingSummary {
    pub tmux_session: String,
    pub workdir: PathBuf,
    pub agent: crate::project::AgentKind,
}

#[derive(Debug, Clone, Default)]
pub struct SummaryState {
    #[allow(dead_code)] // populated but not read back yet
    pub pending: Vec<PendingSummary>,
    #[allow(dead_code)] // populated but not read back yet
    pub last_status: std::collections::HashMap<String, crate::project::ProjectStatus>,
    pub generating: std::collections::HashSet<String>,
}

impl SummaryState {
    pub fn new() -> Self {
        Self {
            pending: Vec::new(),
            last_status: std::collections::HashMap::new(),
            generating: std::collections::HashSet::new(),
        }
    }
}

pub struct ThemePickerState {
    pub selected: usize,
    pub themes: Vec<crate::theme::ThemeName>,
    pub original_theme: crate::theme::ThemeName,
}

pub struct SyntaxLanguageRow {
    pub language: crate::highlight::HighlightLanguage,
    pub status: crate::highlight::HighlightInstallState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyntaxOperationAction {
    Install,
    Uninstall,
}

pub enum SyntaxOperationEvent {
    Output(String),
    Finished(Result<String, String>),
}

pub struct SyntaxOperationState {
    pub language: crate::highlight::HighlightLanguage,
    pub action: SyntaxOperationAction,
    pub last_output: Option<String>,
    pub started_at: std::time::Instant,
    pub output_rx: std::sync::mpsc::Receiver<SyntaxOperationEvent>,
}

pub struct SyntaxLanguagePickerState {
    pub languages: Vec<SyntaxLanguageRow>,
    pub selected: usize,
    pub notice: Option<String>,
    pub operation: Option<SyntaxOperationState>,
    pub return_to: Option<Box<AppMode>>,
    pub auto_return_on_success: bool,
    pub return_language: Option<crate::highlight::HighlightLanguage>,
}

pub struct DebugLogState {
    pub scroll_offset: usize,
    pub from_view: Option<ViewState>,
    pub hide_perf_logs: bool,
}

pub struct MarkdownViewerState {
    pub title: String,
    pub source_path: PathBuf,
    pub content: String,
    pub scroll_offset: usize,
    pub rendered_width: u16,
    pub rendered_lines: Vec<ratatui::text::Line<'static>>,
    pub return_to_picker: Option<MarkdownFilePickerState>,
    pub from_view: Option<ViewState>,
}

pub enum MarkdownLoadingOperation {
    DiscoverFromView {
        view: ViewState,
    },
    DiscoverFromViewer {
        viewer: MarkdownViewerState,
    },
    ReadPath {
        path: PathBuf,
        workdir: PathBuf,
        repo_root: Option<PathBuf>,
        view: ViewState,
        return_to_picker: Option<MarkdownFilePickerState>,
    },
}

pub struct MarkdownLoadingState {
    pub title: String,
    pub from_view: Option<ViewState>,
    pub operation: MarkdownLoadingOperation,
}

pub struct MarkdownFilePickerState {
    pub files: Vec<PathBuf>,
    pub selected: usize,
    pub plan_only: bool,
    pub search_active: bool,
    pub query: String,
    pub workdir: PathBuf,
    pub repo_root: Option<PathBuf>,
    pub from_view: Option<ViewState>,
}

#[derive(Clone)]
pub struct SessionPickerState {
    pub builtin_sessions: Vec<BuiltinSessionOption>,
    pub custom_sessions: Vec<CustomSessionConfig>,
    pub selected: usize,
    pub pi: usize,
    pub fi: usize,
    pub from_view: Option<ViewState>,
}

#[derive(Clone)]
pub struct BuiltinSessionOption {
    pub kind: crate::project::SessionKind,
    pub label: String,
    pub disabled: Option<String>,
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
    pub explanation_child: Option<Child>,
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

pub enum HookNext {
    WorktreeCreated {
        project_name: String,
        branch: String,
        mode: VibeMode,
        review: bool,
        plan_mode: bool,
        agent: AgentKind,
        create_terminal: bool,
        session_name: String,
        enable_chrome: bool,
        remote_control: bool,
        steering_enabled: bool,
    },
    StartFeature {
        pi: usize,
        fi: usize,
    },
    StopFeature {
        pi: usize,
        fi: usize,
    },
}

pub struct HookPromptState {
    pub script: String,
    pub workdir: PathBuf,
    pub title: String,
    pub options: Vec<String>,
    pub selected: usize,
    pub next: HookNext,
}

pub struct RunningHookState {
    pub script: String,
    pub workdir: PathBuf,
    pub project_name: String,
    pub branch: String,
    pub mode: VibeMode,
    pub review: bool,
    pub plan_mode: bool,
    pub agent: AgentKind,
    pub create_terminal: bool,
    pub session_name: String,
    pub enable_chrome: bool,
    pub remote_control: bool,
    pub steering_enabled: bool,
    pub child: Option<Child>,
    pub output: String,
    pub success: Option<bool>,
    pub output_rx: Option<std::sync::mpsc::Receiver<String>>,
}

impl RunningHookState {
    pub fn key(&self) -> String {
        format!("{}/{}", self.workdir.display(), self.script)
    }
}

pub struct DeletingFeatureState {
    pub project_name: String,
    pub feature_name: String,
    pub tmux_session: String,
    pub is_worktree: bool,
    pub repo: PathBuf,
    pub workdir: PathBuf,
    pub stage: DeleteStage,
    pub child: Option<Child>,
    pub output: String,
    pub output_rx: Option<std::sync::mpsc::Receiver<String>>,
    pub error: Option<String>,
}

impl DeletingFeatureState {
    pub fn key(&self) -> String {
        format!("{}/{}", self.project_name, self.feature_name)
    }
}

pub struct BackgroundDeletion {
    pub project_name: String,
    pub feature_name: String,
    pub tmux_session: String,
    pub is_worktree: bool,
    pub repo: PathBuf,
    pub workdir: PathBuf,
    pub stage: DeleteStage,
    pub child: Option<Child>,
    pub output: String,
    pub output_rx: Option<std::sync::mpsc::Receiver<String>>,
    pub error: Option<String>,
}

impl BackgroundDeletion {
    pub fn from_deleting_state(state: DeletingFeatureState) -> Self {
        Self {
            project_name: state.project_name,
            feature_name: state.feature_name,
            tmux_session: state.tmux_session,
            is_worktree: state.is_worktree,
            repo: state.repo,
            workdir: state.workdir,
            stage: state.stage,
            child: state.child,
            output: state.output,
            output_rx: state.output_rx,
            error: state.error,
        }
    }
}

pub struct BackgroundHook {
    #[allow(dead_code)] // retained for the background-hook key, not read directly
    pub script: String,
    pub workdir: PathBuf,
    pub project_name: String,
    pub branch: String,
    pub mode: VibeMode,
    pub review: bool,
    pub plan_mode: bool,
    pub agent: AgentKind,
    pub create_terminal: bool,
    pub session_name: String,
    pub enable_chrome: bool,
    pub remote_control: bool,
    pub steering_enabled: bool,
    pub child: Option<Child>,
    pub output: String,
    pub success: Option<bool>,
    pub output_rx: Option<std::sync::mpsc::Receiver<String>>,
}

impl BackgroundHook {
    pub fn from_running_state(state: RunningHookState) -> Self {
        Self {
            script: state.script,
            workdir: state.workdir,
            project_name: state.project_name,
            branch: state.branch,
            mode: state.mode,
            review: state.review,
            plan_mode: state.plan_mode,
            agent: state.agent,
            create_terminal: state.create_terminal,
            session_name: state.session_name,
            enable_chrome: state.enable_chrome,
            remote_control: state.remote_control,
            steering_enabled: state.steering_enabled,
            child: state.child,
            output: state.output,
            success: state.success,
            output_rx: state.output_rx,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum DeleteStage {
    KillingTmux,
    RemovingWorktree,
    Completed,
}

pub struct BrowsePathState {
    pub explorer: FileExplorer,
    pub create_state: CreateProjectState,
    pub new_folder_name: String,
    pub creating_folder: bool,
}

#[derive(Clone)]
pub struct CreateProjectState {
    pub step: CreateProjectStep,
    pub name: String,
    pub path: String,
    pub agent: AgentKind,
    pub agent_index: usize,
}

#[derive(Clone, PartialEq)]
pub enum CreateProjectStep {
    Name,
    Path,
    Agent,
}

impl CreateProjectState {
    pub fn auto_detect() -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();
        let repo_path = crate::worktree::WorktreeManager::repo_root(&cwd)
            .unwrap_or(cwd)
            .to_string_lossy()
            .into_owned();
        Self {
            step: CreateProjectStep::Name,
            name: String::new(),
            path: repo_path,
            agent: AgentKind::default(),
            agent_index: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum CreateFeatureStep {
    Source,
    ExistingWorktree,
    SelectPreset,
    Branch,
    Worktree,
    Mode,
    SessionName,
    #[allow(dead_code)] // not constructed yet
    TaskPrompt,
    ConfirmSuperVibe,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CreateBatchFeaturesStep {
    WorkspacePath,
    ProjectName,
    FeatureCount,
    FeatureBaseName,
    FeatureSettings,
}

pub struct CreateFeatureState {
    pub project_name: String,
    pub project_repo: PathBuf,
    pub branch: String,
    pub branch_error: Option<String>,
    pub allowed_agents: Vec<AgentKind>,
    pub feature_presets: Vec<FeaturePreset>,
    pub step: CreateFeatureStep,
    pub agent: AgentKind,
    pub agent_index: usize,
    pub mode: VibeMode,
    pub mode_index: usize,
    pub mode_focus: usize,
    pub review: bool,
    pub plan_mode: bool,
    pub create_terminal: bool,
    pub session_name: String,
    pub source_index: usize,
    pub worktrees: Vec<WorktreeInfo>,
    pub worktree_index: usize,
    pub worktree_search_active: bool,
    pub worktree_query: String,
    pub use_worktree: bool,
    pub enable_chrome: bool,
    pub remote_control: bool,
    /// Whether Remote Control can be enabled for this feature. False when
    /// the resolved auth is incompatible (e.g. a z.ai / third-party
    /// provider session). When false the wizard shows the toggle disabled
    /// with a reason rather than letting the user enable something that
    /// would be silently dropped at launch.
    pub remote_control_available: bool,
    /// When `remote_control_available` is false, a short reason shown in
    /// the wizard (e.g. "Unavailable with z.ai provider" or a version
    /// requirement). `None` when Remote Control is available.
    pub remote_control_block_reason: Option<String>,
    pub steering_enabled: bool,
    pub preset_index: usize,
    pub task_prompt: String,
    pub prompt_analysis: PromptAnalysis,
    #[allow(dead_code)] // populated but not read yet
    pub prepared_launch: Option<PreparedFeatureLaunch>,
}

impl CreateFeatureState {
    pub fn new(
        project_name: String,
        project_repo: PathBuf,
        worktrees: Vec<WorktreeInfo>,
        is_first_feature: bool,
    ) -> Self {
        let cwd = std::env::current_dir().unwrap_or_default();
        let branch = crate::worktree::WorktreeManager::current_branch(&cwd)
            .ok()
            .flatten()
            .unwrap_or_default();

        let step = if worktrees.is_empty() {
            CreateFeatureStep::Branch
        } else {
            CreateFeatureStep::Source
        };
        Self {
            project_name,
            project_repo,
            branch,
            branch_error: None,
            allowed_agents: AgentKind::ALL.to_vec(),
            feature_presets: Vec::new(),
            step,
            agent: AgentKind::default(),
            agent_index: 0,
            mode: VibeMode::default(),
            mode_index: 0,
            mode_focus: 0,
            review: false,
            plan_mode: false,
            create_terminal: false,
            session_name: "Claude 1".to_string(),
            source_index: 0,
            worktrees,
            worktree_index: 0,
            worktree_search_active: false,
            worktree_query: String::new(),
            use_worktree: !is_first_feature,
            enable_chrome: false,
            remote_control: false,
            // Assume available; the caller refines this from the resolved
            // auth (see feature_ops.rs) when opening the wizard.
            remote_control_available: true,
            remote_control_block_reason: None,
            steering_enabled: false,
            preset_index: 0,
            task_prompt: String::new(),
            prompt_analysis: crate::app::analyze_prompt(""),
            prepared_launch: None,
        }
    }

    pub fn focused_mode_description(&self) -> Option<&'static str> {
        match self.mode_focus {
            0 | 1 => None,
            2 => Some(
                "High token usage: writes developer notes with every code change for a detailed code review.",
            ),
            3 => Some("Start in planning mode so the agent discusses the approach before editing."),
            4 if self.agent == AgentKind::Claude => {
                Some("Enable browser automation for features that need Chrome.")
            }
            4 => Some("Use the prompt coach to sharpen the feature request before launch."),
            5 if self.agent == AgentKind::Claude && self.remote_control_available => {
                Some("Enable claude.ai and mobile sync for this Claude session.")
            }
            5 if self.agent == AgentKind::Claude => {
                Some("Remote Control is unavailable for the selected Claude auth provider.")
            }
            _ => Some("Use the prompt coach to sharpen the feature request before launch."),
        }
    }

    pub fn refresh_prompt_analysis(&mut self) {
        self.prompt_analysis = crate::app::analyze_prompt(&self.task_prompt);
    }

    pub fn visible_worktree_indices(&self) -> Vec<usize> {
        let mut matches: Vec<(usize, usize)> = self
            .worktrees
            .iter()
            .enumerate()
            .filter_map(|(idx, worktree)| {
                let score =
                    crate::app::util::worktree_picker_score(worktree, &self.worktree_query)?;
                Some((idx, score))
            })
            .collect();

        matches.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        matches.into_iter().map(|(idx, _)| idx).collect()
    }

    pub fn clamp_worktree_selection(&mut self) {
        let visible = self.visible_worktree_indices();
        if visible.is_empty() {
            self.worktree_index = 0;
        } else if !visible.contains(&self.worktree_index) {
            self.worktree_index = visible[0];
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreparedFeatureLaunch {
    pub project_name: String,
    pub branch: String,
    pub workdir: PathBuf,
    pub is_worktree: bool,
    pub mode: VibeMode,
    pub review: bool,
    pub plan_mode: bool,
    pub agent: AgentKind,
    pub create_terminal: bool,
    pub session_name: String,
    pub enable_chrome: bool,
    pub remote_control: bool,
    pub steering_enabled: bool,
    pub hook_succeeded: Option<bool>,
    #[allow(dead_code)] // populated but not read yet
    pub startup_prompt: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanInterviewPhase {
    Brief,
    StaticQuestions,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanInterviewAdvanceError {
    BriefRequired,
    AnswerRequired,
}

/// In-memory state for the static-question interview delivered in Epic 1.
///
/// Draft persistence and AI-generated phases are intentionally layered onto
/// this state in later epics. `pending_launch` is optional so the same state
/// can also support on-demand interviews for existing features.
pub struct PlanInterviewState {
    pub feature_name: String,
    pub phase: PlanInterviewPhase,
    pub questions: Vec<PlanQuestion>,
    pub question_index: usize,
    pub brief: String,
    pub answers: Vec<Option<String>>,
    pub editor: TextEditor,
    pub selected_option: usize,
    pub pending_launch: Option<PreparedFeatureLaunch>,
    pub abort_confirmation: bool,
}

impl PlanInterviewState {
    pub fn for_feature_creation(
        pending_launch: PreparedFeatureLaunch,
        questions: Vec<PlanQuestion>,
    ) -> Self {
        let feature_name = pending_launch.branch.clone();
        Self::new(feature_name, questions, Some(pending_launch))
    }

    pub fn new(
        feature_name: String,
        questions: Vec<PlanQuestion>,
        pending_launch: Option<PreparedFeatureLaunch>,
    ) -> Self {
        let answer_count = questions.len();
        Self {
            feature_name,
            phase: PlanInterviewPhase::Brief,
            questions,
            question_index: 0,
            brief: String::new(),
            answers: vec![None; answer_count],
            editor: TextEditor::new(String::new()),
            selected_option: 0,
            pending_launch,
            abort_confirmation: false,
        }
    }

    pub fn current_question(&self) -> Option<&PlanQuestion> {
        if self.phase == PlanInterviewPhase::StaticQuestions {
            self.questions.get(self.question_index)
        } else {
            None
        }
    }

    pub fn select_previous_option(&mut self) {
        let option_count = self
            .current_question()
            .and_then(|question| match &question.kind {
                PlanQuestionKind::Select(options) => Some(options.len()),
                PlanQuestionKind::FreeText => None,
            })
            .unwrap_or(0);
        if option_count > 0 {
            self.selected_option = self
                .selected_option
                .checked_sub(1)
                .unwrap_or(option_count - 1);
        }
    }

    pub fn select_next_option(&mut self) {
        let option_count = self
            .current_question()
            .and_then(|question| match &question.kind {
                PlanQuestionKind::Select(options) => Some(options.len()),
                PlanQuestionKind::FreeText => None,
            })
            .unwrap_or(0);
        if option_count > 0 {
            self.selected_option = (self.selected_option + 1) % option_count;
        }
    }

    /// Save the current input and move to the next interview step.
    pub fn advance(&mut self) -> Result<(), PlanInterviewAdvanceError> {
        match self.phase {
            PlanInterviewPhase::Brief => {
                if self.editor.text().trim().is_empty() {
                    return Err(PlanInterviewAdvanceError::BriefRequired);
                }
                self.brief = self.editor.text().to_string();
                if self.questions.is_empty() {
                    self.phase = PlanInterviewPhase::Done;
                } else {
                    self.phase = PlanInterviewPhase::StaticQuestions;
                    self.question_index = 0;
                    self.load_current_answer();
                }
            }
            PlanInterviewPhase::StaticQuestions => {
                self.save_current_answer(false)?;
                self.move_after_current_question();
            }
            PlanInterviewPhase::Done => {}
        }
        Ok(())
    }

    /// Skip an optional question and move forward without recording an answer.
    pub fn skip(&mut self) -> Result<(), PlanInterviewAdvanceError> {
        let Some(question) = self.current_question() else {
            return Ok(());
        };
        if !question.optional {
            return Err(PlanInterviewAdvanceError::AnswerRequired);
        }
        self.answers[self.question_index] = None;
        self.move_after_current_question();
        Ok(())
    }

    /// Return to the previous step, restoring its draft answer into the editor.
    pub fn back(&mut self) -> bool {
        match self.phase {
            PlanInterviewPhase::Brief => false,
            PlanInterviewPhase::StaticQuestions if self.question_index == 0 => {
                self.save_current_draft();
                self.phase = PlanInterviewPhase::Brief;
                self.editor = TextEditor::new(self.brief.clone());
                self.selected_option = 0;
                true
            }
            PlanInterviewPhase::StaticQuestions => {
                self.save_current_draft();
                self.question_index -= 1;
                self.load_current_answer();
                true
            }
            PlanInterviewPhase::Done if !self.questions.is_empty() => {
                self.phase = PlanInterviewPhase::StaticQuestions;
                self.question_index = self.questions.len() - 1;
                self.load_current_answer();
                true
            }
            PlanInterviewPhase::Done => {
                self.phase = PlanInterviewPhase::Brief;
                self.editor = TextEditor::new(self.brief.clone());
                true
            }
        }
    }

    /// End questioning with the answers collected so far.
    pub fn finish_early(&mut self) -> Result<(), PlanInterviewAdvanceError> {
        match self.phase {
            PlanInterviewPhase::Brief => {
                if self.editor.text().trim().is_empty() {
                    return Err(PlanInterviewAdvanceError::BriefRequired);
                }
                self.brief = self.editor.text().to_string();
            }
            PlanInterviewPhase::StaticQuestions => self.save_current_draft(),
            PlanInterviewPhase::Done => {}
        }
        self.phase = PlanInterviewPhase::Done;
        Ok(())
    }

    fn save_current_answer(
        &mut self,
        allow_empty_optional: bool,
    ) -> Result<(), PlanInterviewAdvanceError> {
        let Some(question) = self.questions.get(self.question_index) else {
            return Ok(());
        };
        let answer = match &question.kind {
            PlanQuestionKind::FreeText => {
                let text = self.editor.text();
                if text.trim().is_empty() {
                    None
                } else {
                    Some(text.to_string())
                }
            }
            PlanQuestionKind::Select(options) => options.get(self.selected_option).cloned(),
        };
        if answer.is_none() && !question.optional && !allow_empty_optional {
            return Err(PlanInterviewAdvanceError::AnswerRequired);
        }
        self.answers[self.question_index] = answer;
        Ok(())
    }

    fn save_current_draft(&mut self) {
        let _ = self.save_current_answer(true);
    }

    fn move_after_current_question(&mut self) {
        if self.question_index + 1 >= self.questions.len() {
            self.phase = PlanInterviewPhase::Done;
        } else {
            self.question_index += 1;
            self.load_current_answer();
        }
    }

    fn load_current_answer(&mut self) {
        let existing = self
            .answers
            .get(self.question_index)
            .and_then(|answer| answer.as_deref());
        match self.questions.get(self.question_index).map(|q| &q.kind) {
            Some(PlanQuestionKind::FreeText) => {
                self.editor = TextEditor::new(existing.unwrap_or_default().to_string());
                self.selected_option = 0;
            }
            Some(PlanQuestionKind::Select(options)) => {
                self.editor = TextEditor::new(String::new());
                self.selected_option = existing
                    .and_then(|answer| options.iter().position(|option| option == answer))
                    .unwrap_or(0);
            }
            None => {}
        }
    }
}

#[derive(Clone)]
pub struct CreateBatchFeaturesState {
    pub workspace_path: String,
    pub project_name: String,
    pub feature_count: usize,
    pub feature_prefix: String,
    pub agent: AgentKind,
    pub agent_index: usize,
    pub mode: VibeMode,
    pub mode_index: usize,
    pub mode_focus: usize,
    pub review: bool,
    pub enable_chrome: bool,
    pub step: CreateBatchFeaturesStep,
}

impl CreateBatchFeaturesState {
    pub fn with_workspace(workspace_path: Option<String>) -> Self {
        let repo_path = if let Some(ws) = workspace_path {
            ws
        } else {
            let cwd = std::env::current_dir().unwrap_or_default();
            crate::worktree::WorktreeManager::repo_root(&cwd)
                .unwrap_or(cwd)
                .to_string_lossy()
                .into_owned()
        };
        let workspace_name = std::path::Path::new(&repo_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace")
            .to_string();

        Self {
            workspace_path: repo_path,
            project_name: workspace_name,
            feature_count: 3,
            feature_prefix: "feature".to_string(),
            agent: AgentKind::default(),
            agent_index: 0,
            mode: VibeMode::default(),
            mode_index: 0,
            mode_focus: 0,
            review: false,
            enable_chrome: false,
            step: CreateBatchFeaturesStep::WorkspacePath,
        }
    }
}

pub const DASHBOARD_SESSION_FILTER_ENABLED: bool = false;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum SessionFilter {
    #[default]
    All,
    Claude,
    Opencode,
    Codex,
    Terminal,
    Nvim,
    Vscode,
}

impl SessionFilter {
    pub const ALL: [SessionFilter; 7] = [
        SessionFilter::All,
        SessionFilter::Claude,
        SessionFilter::Opencode,
        SessionFilter::Codex,
        SessionFilter::Terminal,
        SessionFilter::Nvim,
        SessionFilter::Vscode,
    ];

    pub fn display_name(&self) -> &str {
        match self {
            SessionFilter::All => "all",
            SessionFilter::Claude => "claude",
            SessionFilter::Opencode => "opencode",
            SessionFilter::Codex => "codex",
            SessionFilter::Terminal => "terminal",
            SessionFilter::Nvim => "nvim",
            SessionFilter::Vscode => "vscode",
        }
    }

    pub fn next(&self) -> Self {
        let variants = Self::ALL.as_slice();
        let idx = variants.iter().position(|v| v == self).unwrap_or(0);
        variants[(idx + 1) % variants.len()].clone()
    }
}

pub struct SearchState {
    pub query: String,
    pub matches: Vec<SearchMatch>,
    pub selected_match: usize,
}

#[derive(Debug, Clone)]
pub struct SearchMatch {
    pub item: VisibleItem,
    pub label: String,
    pub context: String,
}

#[derive(Debug, Clone)]
pub enum VisibleItem {
    Project(usize),
    Feature(usize, usize),
    Session(usize, usize, usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── SessionFilter::next ───────────────────────────────────

    #[test]
    fn session_filter_next_cycles_through_all_variants() {
        let all = SessionFilter::ALL.as_slice();
        for (i, variant) in all.iter().enumerate() {
            let next = variant.next();
            let expected = &all[(i + 1) % all.len()];
            assert_eq!(
                &next, expected,
                "after {i} expected {:?} got {:?}",
                expected, next
            );
        }
    }

    #[test]
    fn session_filter_last_wraps_to_first() {
        let last = SessionFilter::ALL.last().unwrap();
        let next = last.next();
        assert_eq!(next, SessionFilter::ALL[0]);
    }

    #[test]
    fn session_filter_all_has_seven_variants() {
        assert_eq!(SessionFilter::ALL.len(), 7);
    }

    // ── branch_mismatch ───────────────────────────────────

    #[test]
    fn branch_mismatch_none_when_branches_match() {
        assert_eq!(branch_mismatch("main", Some("main")), None);
    }

    #[test]
    fn branch_mismatch_some_when_branches_differ() {
        assert_eq!(
            branch_mismatch("main", Some("other-branch")),
            Some("other-branch")
        );
    }

    #[test]
    fn branch_mismatch_none_when_pr_head_ref_unknown() {
        // Pre-existing cache row from before `head_ref` existed.
        assert_eq!(branch_mismatch("", Some("other-branch")), None);
    }

    #[test]
    fn branch_mismatch_none_when_checked_out_branch_unknown() {
        // Detached HEAD or the `git branch --show-current` lookup failed.
        assert_eq!(branch_mismatch("main", None), None);
    }

    #[test]
    fn plan_interview_requires_a_brief_before_questions() {
        let mut state = PlanInterviewState::new(
            "feature".into(),
            crate::plan_interview::builtin_questions(),
            None,
        );

        assert_eq!(
            state.advance(),
            Err(PlanInterviewAdvanceError::BriefRequired)
        );
        assert_eq!(state.phase, PlanInterviewPhase::Brief);

        state.editor = TextEditor::new("Build the feature\nwith care".into());
        state.advance().unwrap();

        assert_eq!(state.phase, PlanInterviewPhase::StaticQuestions);
        assert_eq!(state.brief, "Build the feature\nwith care");
        assert_eq!(state.current_question().unwrap().id, "scope");
    }

    #[test]
    fn plan_interview_retains_answers_when_navigating_back() {
        let mut state = PlanInterviewState::new(
            "feature".into(),
            crate::plan_interview::builtin_questions(),
            None,
        );
        state.editor = TextEditor::new("A useful feature".into());
        state.advance().unwrap();
        state.editor = TextEditor::new("In: interviews. Out: AI.".into());
        state.advance().unwrap();

        assert_eq!(state.question_index, 1);
        assert!(state.back());
        assert_eq!(state.question_index, 0);
        assert_eq!(state.editor.text(), "In: interviews. Out: AI.");

        assert!(state.back());
        assert_eq!(state.phase, PlanInterviewPhase::Brief);
        assert_eq!(state.editor.text(), "A useful feature");
    }

    #[test]
    fn plan_interview_skip_and_finish_early_preserve_progress() {
        let mut state = PlanInterviewState::new(
            "feature".into(),
            crate::plan_interview::builtin_questions(),
            None,
        );
        state.editor = TextEditor::new("A useful feature".into());
        state.advance().unwrap();
        state.skip().unwrap();
        state.editor = TextEditor::new("Developers use it from the dashboard".into());
        state.finish_early().unwrap();

        assert_eq!(state.phase, PlanInterviewPhase::Done);
        assert_eq!(state.answers[0], None);
        assert_eq!(
            state.answers[1].as_deref(),
            Some("Developers use it from the dashboard")
        );
    }

    #[test]
    fn plan_interview_select_options_wrap_and_restore_the_answer() {
        let question = PlanQuestion {
            id: "surface".into(),
            text: "Where should this appear?".into(),
            kind: PlanQuestionKind::Select(vec!["Dashboard".into(), "Session".into()]),
            source: crate::plan_interview::QuestionSource::Template,
            optional: false,
        };
        let mut state = PlanInterviewState::new("feature".into(), vec![question], None);
        state.editor = TextEditor::new("A useful feature".into());
        state.advance().unwrap();

        state.select_previous_option();
        assert_eq!(state.selected_option, 1);
        state.advance().unwrap();
        assert_eq!(state.answers[0].as_deref(), Some("Session"));

        assert!(state.back());
        assert_eq!(state.selected_option, 1);
        state.select_next_option();
        assert_eq!(state.selected_option, 0);
    }
}
