use ratatui_explorer::FileExplorer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use super::PromptAnalysis;
use crate::editor::TextEditor;
use crate::extension::{CustomSessionConfig, FeaturePreset, LifecycleHooks};
use crate::project::{AgentKind, SessionKind, VibeMode};
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
    /// Cycle All → Undecided → Rejected → Blockers → Unresolved → Changed → All.
    /// Steps with nothing to show are skipped by the caller (see
    /// `diff_review_cycle_file_filter`): `Changed` without a prior review
    /// snapshot, `Unresolved` without an open thread.
    pub fn next(self) -> Self {
        match self {
            FileFilter::All => FileFilter::Undecided,
            FileFilter::Undecided => FileFilter::Rejected,
            FileFilter::Rejected => FileFilter::Blockers,
            FileFilter::Blockers => FileFilter::Unresolved,
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

// Not `Clone`: holds a `std::process::Child` for the in-flight walkthrough
// generation (matching `DiffReviewState`). Nothing clones this state wholesale.
pub struct DiffViewerState {
    pub from_view: ViewState,
    pub workdir: PathBuf,
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
    /// True while typing a *suggested replacement* for the cursored line/span
    /// (also reuses `feedback_editor`; mutually exclusive with
    /// `editing_line_comment`). The editor content is the replacement code.
    pub editing_suggestion: bool,
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
}

impl DiffViewerState {
    pub fn new(from_view: ViewState, workdir: PathBuf) -> Self {
        Self {
            from_view,
            workdir,
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
            comment_cursor: None,
            comment_anchor: None,
            editing_line_comment: false,
            editing_suggestion: false,
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
        reject_blocks || comment_blocks
    }

    /// Whether the file at `path` carries at least one open thread — a kept,
    /// unresolved line comment. Backs the `Unresolved` filter and the auto-reject
    /// rule (an open thread means the file still needs work).
    pub fn file_has_unresolved_thread(&self, path: &str) -> bool {
        self.line_comments
            .get(path)
            .is_some_and(|cs| cs.iter().any(|c| c.is_open_thread()))
    }

    /// Total open threads across every file in the diff. Reported on opening a
    /// re-review and used to decide whether the `Unresolved` filter has anything
    /// to show.
    pub fn unresolved_thread_count(&self) -> usize {
        self.line_comments
            .values()
            .flatten()
            .filter(|c| c.is_open_thread())
            .count()
    }

    /// True when no line comment was authored in *this* session — every stored
    /// comment (if any) was carried in from a previous finished round. Lets a
    /// fresh re-review still read as "pristine" for the purposes of auto-applying
    /// the `Changed` filter, even though it opens with threads restored.
    pub fn has_only_carried_comments(&self) -> bool {
        self.line_comments.values().flatten().all(|c| c.carried)
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
        self.clipboard_paste_id.is_some()
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
/// no auto-detectable PR, or on demand from the review pane to switch PRs. The
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
}

/// State for the full-screen PR comment-review pane.
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
    /// Which agent session "fix" prompts are injected into (toggle with `t`).
    pub fix_target: crate::app::pr_review::FixTarget,
    /// Harness chosen for the dedicated review session, picked once before the
    /// first fix is injected and reused for the rest of the PR. `None` until the
    /// user picks (or when the dedicated session already exists / isn't the
    /// target). Lets PR triage run on a different harness than the feature's
    /// working session.
    pub review_harness: Option<AgentKind>,
    /// When `Some`, the harness picker is open over the pane: the user is
    /// choosing which agent harness the dedicated review session will run before
    /// the first fix is injected.
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
    /// When `Some`, the reply dialog is open over the pane: an AI-drafted,
    /// editable reply awaiting the user's approval before it is posted to GitHub.
    pub reply: Option<ReplyState>,
    /// Comment ids marked (with `space`) for a batch fix. `F` queues every
    /// marked comment's fix prompt into the dedicated review session in one
    /// pass. Keyed by id (not index) so marks survive the hide-resolved filter
    /// shifting the visible rows. Cleared once the batch is queued.
    pub marked: std::collections::HashSet<u64>,
    /// Set while the combined-batch flow (`B`) is waiting on the harness picker:
    /// after the user picks the review harness, the continuation opens the
    /// combined-batch confirm dialog instead of the single-comment one. Cleared
    /// when the picker is confirmed or cancelled.
    pub pending_batch: bool,
}

/// A [`PrReviewState`] stashed while the user is watching the linked fix
/// session (`P` from the review pane), so `leader+P` can jump straight back
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

/// Single-select harness picker shown before the dedicated PR-review session is
/// spun up, so the user can run triage fixes on a different harness than the
/// feature's working session. Highlights the project's preferred agent by
/// default.
#[derive(Debug, Clone)]
pub struct HarnessPickState {
    /// The harnesses to choose from (the repo's allowed agents).
    pub agents: Vec<AgentKind>,
    /// Index into `agents` of the highlighted choice.
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
    /// True while keystrokes go to the editor (`e` to enter); false in the
    /// confirm view (`⏎` post / `e` edit / `esc` cancel).
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
}

impl PrReviewState {
    pub fn selected_comment(&self) -> Option<&crate::app::pr_review::PrComment> {
        self.review.comments.get(self.selected)
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
        }
        indices
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
    DiffViewerLoading(DiffViewerState),
    DiffViewer(DiffViewerState),
    /// Prompting for a PR number when the branch has no auto-detectable PR.
    PrNumberPrompt(PrNumberPromptState),
    /// Choosing a PR from a list (or falling through to the number prompt).
    PrPicker(PrPickerState),
    /// Fetching a PR's comments off the UI thread; shows a loading frame.
    PrReviewLoading(PrReviewLoadState),
    /// Triaging a PR's comments in the full-screen review pane.
    PrReview(PrReviewState),
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
                "Write developer notes with every code change for a detailed code review (may use more tokens).",
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
}
