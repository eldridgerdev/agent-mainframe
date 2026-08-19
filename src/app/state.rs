use ratatui_explorer::FileExplorer;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use super::PromptAnalysis;
use crate::db::plan_interviews::{PlanInterviewRecord, PlanInterviewStage};
use crate::editor::TextEditor;
use crate::extension::{
    ConfiguredPlanQuestion, CustomSessionConfig, FeaturePreset, LifecycleHooks,
};
use crate::plan_interview::{PlanQuestion, PlanQuestionKind, QuestionSource};
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

/// A request to suspend the TUI and hand the terminal to `$VISUAL`/`$EDITOR`.
/// Raised by the review viewer (`E`) and drained by the main loop, which owns
/// the terminal's raw-mode/alternate-screen state — the app layer can resolve
/// *what* to open but must not tear the screen down underneath itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingEditorOpen {
    /// Absolute path to the file, already validated as a regular file inside
    /// the worktree.
    pub path: PathBuf,
    /// Directory the editor is spawned in.
    pub workdir: PathBuf,
    /// 1-based line to place the cursor on, when the editor understands one.
    pub line: Option<usize>,
    /// Worktree-relative path, for the message shown after the editor exits.
    pub display: String,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoppedSessionChoice {
    Resume,
    Clear,
    /// Hand off to the harness's saved-transcript picker (the `S` path), so
    /// older sessions than the one AMF has recorded stay reachable.
    PickSession,
    Cancel,
}

#[derive(Debug, Clone)]
pub struct StoppedSessionDialogState {
    pub project_id: String,
    pub feature_id: String,
    pub session_id: String,
    pub selected: usize,
    /// Choices offered for this session, in display order. Only harnesses with
    /// a transcript picker get [`StoppedSessionChoice::PickSession`]; every
    /// entry present is selectable, so there is no disabled state to skip.
    pub choices: Vec<StoppedSessionChoice>,
    /// Harness name used in the dialog copy ("Claude", "Codex", ...).
    pub harness_label: String,
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
    /// Which doc the distilled findings land in, toggled with `g`. Defaults to
    /// `Project`: a bootstrap learns from *this* repo's PR history, so its
    /// findings belong to this repo unless the user says otherwise.
    pub scope: crate::app::review_memory::MemoryScope,
}

/// Full-screen progress view for the lookback bootstrap's background fetch +
/// distill pass, entered once a depth is confirmed.
#[derive(Debug, Clone)]
pub struct BootstrapRunState {
    /// The PR picker to return to on completion or cancel.
    pub origin: PrPickerState,
    pub depth: crate::app::pr_review::BootstrapDepth,
    /// Which doc the run is appending to, carried through from the picker so
    /// the running screen and completion toast can name it.
    pub scope: crate::app::review_memory::MemoryScope,
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
    /// Re-read for the newly selected doc on every `scope` toggle, so the
    /// number always describes what `⏎` would actually compact.
    pub existing_findings: usize,
    /// Which doc gets compacted, toggled with `g`. Defaults to `Project` when
    /// that doc has findings, otherwise `Global` — so `c` still reaches the
    /// only non-empty doc without the user having to know to press `g`.
    pub scope: crate::app::review_memory::MemoryScope,
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
    /// Which doc the run is rewriting, carried through from confirm so the
    /// running screen can name it (the path alone doesn't read as
    /// project-vs-global at a glance).
    pub scope: crate::app::review_memory::MemoryScope,
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
    /// Which doc is being rewritten, so the success toast names it.
    pub scope: crate::app::review_memory::MemoryScope,
    /// Bullet count in the doc before compacting, for the "N -> M" summary.
    pub original_findings: usize,
    /// Bullet count in the agent's proposed replacement, for the same summary.
    pub proposed_findings: usize,
    /// The proposed replacement text, editable before writing.
    pub editor: TextEditor,
    /// The doc exactly as the compact pass read it. The write re-reads the file
    /// and compares against this before overwriting, so findings another AMF
    /// session appended while the agent ran (or while this dialog sat open) are
    /// re-applied rather than clobbered — the cross-project doc in particular is
    /// shared by every AMF session on the machine.
    pub original_content: String,
    /// Set once a write has been refused because the doc on disk diverged in
    /// ways an append can't explain. The next confirm overwrites deliberately,
    /// so the user is warned but never stuck.
    pub overwrite_confirmed: bool,
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
    /// has already been explicitly resolved for this pane visit by the user
    /// confirming `harness_pick`. Prevents re-opening the picker on every
    /// subsequent `f`/`B` while still allowing each new visit to name another
    /// dedicated session.
    pub fix_target_picked: bool,
    /// Token totals already present when each fix-target session joined this
    /// visit to the PR pane. Current totals minus these snapshots are the live
    /// "this visit" tally; a target created after the pane opened has no
    /// baseline, so all of its usage belongs to the visit.
    pub usage_baselines: HashMap<TokenUsageSource, SessionTokenUsage>,
    /// Harness chosen for the dedicated triage session, picked once before the
    /// first fix is injected and reused for the rest of the pane visit. `None`
    /// until the user picks (or when a dedicated session isn't the target).
    /// Lets PR triage run on a different harness than the feature's working
    /// session.
    pub review_harness: Option<AgentKind>,
    /// Label (and lookup identity) of the dedicated triage session selected for
    /// this pane visit. Defaults to `PR Triage` for backwards compatibility,
    /// but can be named before the first `f`/`B` hand-off so several triage
    /// agents can run alongside one another in the same feature.
    pub dedicated_session_label: String,
    /// When `Some`, the fix-target picker is open over the pane: the user is
    /// choosing whether fixes go to the feature's existing live session or a
    /// dedicated triage session (and, for the latter, which harness) before
    /// the first fix/batch is injected. Replaces the old standalone `t`
    /// toggle — the choice is made once, at the point it's needed.
    pub harness_pick: Option<HarnessPickState>,
    /// When `Some`, the compact triage-feature setup overlay is open: the user
    /// picked `New feature…` in the fix-target picker and is choosing the
    /// companion feature's preset / harness / vibe mode before it is created.
    pub new_feature_setup: Option<TriageFeatureSetupState>,
    /// When `Some`, the integration overlay is open: the review of what the
    /// companion triage feature has committed and how to land it on the PR
    /// branch (push, or cherry-pick into the source worktree).
    pub integrate: Option<TriageIntegrateState>,
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
    /// `Some` after a dedicated harness row is chosen, while the picker is on
    /// its second step accepting an optional session name. An empty name means
    /// the backwards-compatible `PR Triage` label.
    pub session_name: Option<String>,
}

/// One editable row of the compact triage-feature setup overlay
/// ([`TriageFeatureSetupState`]). Deliberately much smaller than the full
/// feature-creation wizard: only the settings that change how the *triage*
/// agent behaves, plus the branch it lands on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageSetupRow {
    /// Apply a configured feature preset (or "Manual", which changes nothing).
    Preset,
    /// Which agent harness the triage feature runs.
    Harness,
    /// Vibe mode — the setting the whole feature exists for: triaging review
    /// comments in, say, Vibeless while the source feature runs SuperVibe.
    Mode,
    /// Review mode (developer notes on every change).
    Review,
    /// Chrome/browser automation.
    Chrome,
    /// The companion branch name. Pre-filled and editable.
    Branch,
}

impl TriageSetupRow {
    pub const ALL: [TriageSetupRow; 6] = [
        TriageSetupRow::Preset,
        TriageSetupRow::Harness,
        TriageSetupRow::Mode,
        TriageSetupRow::Review,
        TriageSetupRow::Chrome,
        TriageSetupRow::Branch,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TriageSetupRow::Preset => "Preset",
            TriageSetupRow::Harness => "Harness",
            TriageSetupRow::Mode => "Vibe mode",
            TriageSetupRow::Review => "Review mode",
            TriageSetupRow::Chrome => "Chrome",
            TriageSetupRow::Branch => "Branch",
        }
    }
}

/// The compact feature-creation flow shown when the user picks `New feature…`
/// as the fix target: a single settings list (no multi-step wizard) that
/// creates an isolated, worktree-backed companion feature for this PR's
/// triage work.
///
/// Plan mode is deliberately absent — it defers the launch into a planning
/// interview, which makes no sense for a feature whose whole job is to apply
/// review comments that already say what to do.
#[derive(Debug, Clone)]
pub struct TriageFeatureSetupState {
    /// Presets available for this repo. Index 0 of the *choice* is "Manual"
    /// (no preset); `presets[i - 1]` for any higher index.
    pub presets: Vec<crate::extension::FeaturePreset>,
    pub preset_index: usize,
    /// Harnesses allowed for this repo.
    pub agents: Vec<AgentKind>,
    pub agent_index: usize,
    pub mode: VibeMode,
    pub review: bool,
    pub enable_chrome: bool,
    /// Companion branch name — deliberately *not* the PR's branch, which git
    /// can't check out in a second worktree.
    pub branch: String,
    /// Focused row.
    pub row: usize,
    /// Inline validation/creation error (e.g. a duplicate feature name), shown
    /// in the overlay so the user can correct it without losing the pane.
    pub error: Option<String>,
    /// True when the combined-batch flow (`B`) opened this, so the
    /// continuation after creation reopens the batch dialog rather than the
    /// single-comment one — mirroring `PrReviewState::pending_batch`.
    pub pending_batch: bool,
}

impl TriageFeatureSetupState {
    /// The chosen preset, or `None` for "Manual".
    pub fn selected_preset(&self) -> Option<&crate::extension::FeaturePreset> {
        self.preset_index
            .checked_sub(1)
            .and_then(|i| self.presets.get(i))
    }

    /// Display text for the preset row.
    pub fn preset_label(&self) -> String {
        match self.selected_preset() {
            Some(preset) => preset.name.clone(),
            None => "Manual".to_string(),
        }
    }

    /// The focused row, or `Branch` if `row` somehow ran past the list.
    pub fn focused_row(&self) -> TriageSetupRow {
        TriageSetupRow::ALL
            .get(self.row)
            .copied()
            .unwrap_or(TriageSetupRow::Branch)
    }

    pub fn agent(&self) -> AgentKind {
        self.agents
            .get(self.agent_index)
            .cloned()
            .unwrap_or_default()
    }
}

/// How the companion triage feature's commits get back onto the PR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriageIntegration {
    /// `git push <remote> <triage-branch>:<pr-branch>` — a normal
    /// fast-forward push. Never forced: a diverged PR branch is reported, not
    /// overwritten.
    Push,
    /// Cherry-pick the triage commits into the source worktree. Offered only
    /// when that worktree is clean, so an in-progress change is never
    /// clobbered.
    CherryPick,
}

impl TriageIntegration {
    pub const ALL: [TriageIntegration; 2] =
        [TriageIntegration::Push, TriageIntegration::CherryPick];

    pub fn label(self) -> &'static str {
        match self {
            TriageIntegration::Push => "Push to the PR branch",
            TriageIntegration::CherryPick => "Cherry-pick into the source worktree",
        }
    }
}

/// The integration overlay (`I`): what the companion triage feature has
/// committed since it branched, and the two explicit, non-destructive ways to
/// land it on the PR. Everything here is computed before the overlay opens, so
/// the user sees exactly what will happen before confirming.
#[derive(Debug, Clone)]
pub struct TriageIntegrateState {
    /// Companion branch holding the triage commits.
    pub triage_branch: String,
    /// The PR's own head branch — where a push lands. Not necessarily the
    /// branch the source worktree has checked out.
    pub pr_branch: String,
    /// One-line summaries of the commits on the triage branch since it
    /// branched (newest first), for the "what will land" preview.
    pub commits: Vec<String>,
    /// Set when the source worktree has uncommitted changes: the cherry-pick
    /// option is disabled and this explains why. Pushing is unaffected — it
    /// never touches the source worktree.
    pub source_dirty: Option<String>,
    /// Set when the companion worktree itself has uncommitted changes — those
    /// wouldn't be included, so say so rather than silently landing less than
    /// the user expects.
    pub triage_dirty: bool,
    pub selected: usize,
    /// Inline result/error from the last attempt, kept in the overlay so a
    /// rejected push can be read and retried in place.
    pub error: Option<String>,
    /// Set once an integration succeeded, so the overlay reports the outcome
    /// instead of inviting the same action again.
    pub done: Option<String>,
}

impl TriageIntegrateState {
    pub fn focused(&self) -> TriageIntegration {
        TriageIntegration::ALL
            .get(self.selected)
            .copied()
            .unwrap_or(TriageIntegration::Push)
    }
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
    /// Best-effort details about the agent session that produced the draft.
    /// Captured when the reply opens so the confirmation UI previews the exact
    /// disclosure that will be posted with an unchanged AI-authored reply.
    pub generation_metadata: Option<crate::app::pr_review::ReplyGenerationMetadata>,
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
    /// Which doc the finding lands in, toggled with `g`. Defaults to
    /// `Project` — a finding from this PR is about this repo until the user
    /// says it's a habit worth carrying everywhere.
    pub scope: crate::app::review_memory::MemoryScope,
    /// The finding text, editable before it's appended.
    pub editor: TextEditor,
    /// True while keystrokes go to the editor (`e` to enter); false in the
    /// confirm view (`⏎` append / `e` edit / `Tab` cycle category / `g` toggle
    /// scope / `esc` cancel).
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
    /// dropped. AMF follow-up replies whose root is present are always
    /// collated under that root's detail view instead of duplicated here.
    pub fn visible_indices(&self) -> Vec<usize> {
        let indices = self
            .review
            .comments
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                !self.review.is_collated_amf_reply(c) && (!self.hide_resolved || !c.is_resolved)
            })
            .map(|(i, _)| i)
            .collect();
        self.sort_indices(indices)
    }

    /// Every comment index — including ones `visible_indices` would drop for
    /// `hide_resolved` or collation — in `sort_mode` order. Used to find a
    /// hidden selection's nearest visible neighbor when a filter or refresh
    /// hides it. Must stay unfiltered so `self.selected` itself can always be
    /// located by `position()`, even when `selected` is the very comment that
    /// just became hidden (e.g. an orphaned AMF reply that a refresh just
    /// collated under its now-present root); the neighbor search then walks
    /// this order and tests `visible_indices().contains()` to find the
    /// nearest comment that's actually shown.
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

/// Which files Learning Mode lists.
// Learning Mode's overlay lands in the plan's Epics 2-5
// (`docs/backlog/learning-mode-plan.md`), so this state is written before
// anything reads it. The `dead_code` allows through the end of
// `LearningViewState` come off in Epic 6.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowseScope {
    /// Every file in the project's working tree (git-tracked plus untracked
    /// files git doesn't ignore).
    RepoTree,
    /// Only the files changed on the feature's branch.
    BranchChanges,
}

#[allow(dead_code)]
impl BrowseScope {
    /// Short header label.
    pub fn label(self) -> &'static str {
        match self {
            BrowseScope::RepoTree => "Repo tree",
            BrowseScope::BranchChanges => "Branch changes",
        }
    }

    /// Spelled-out description — the header says what the scope *is* rather
    /// than relying on the user knowing AMF's vocabulary.
    pub fn description(self) -> &'static str {
        match self {
            BrowseScope::RepoTree => "all files in this project",
            BrowseScope::BranchChanges => "files changed on this branch",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            BrowseScope::RepoTree => BrowseScope::BranchChanges,
            BrowseScope::BranchChanges => BrowseScope::RepoTree,
        }
    }
}

/// What a Learning Mode question is asked *about*. Persisted as an
/// `anchor_kind` string plus an optional line range (see [`LearningAnchor::kind_str`]
/// and [`LearningAnchor::from_parts`]).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningAnchor {
    /// The repository as a whole — used by the orientation ("give me a tour")
    /// question, which has no file to point at.
    Project,
    /// The whole of the currently loaded file.
    File,
    /// A hunk of the current file's diff, by index into `DiffFile::hunks`.
    /// Only reachable in [`BrowseScope::BranchChanges`] — repo-tree browsing
    /// has no diff, so a hunk has nothing to mean there.
    Hunk { index: usize },
    /// An inclusive 1-based line range in the current file.
    Lines { start: usize, end: usize },
}

#[allow(dead_code)]
impl LearningAnchor {
    /// Stable string stored in `learning_qa.anchor_kind`.
    pub fn kind_str(self) -> &'static str {
        match self {
            LearningAnchor::Project => "project",
            LearningAnchor::File => "file",
            LearningAnchor::Hunk { .. } => "hunk",
            LearningAnchor::Lines { .. } => "lines",
        }
    }

    /// The persisted line range: `(line_start, line_end)`, both `None` for
    /// anchors that cover no specific lines.
    pub fn line_range(self) -> (Option<usize>, Option<usize>) {
        match self {
            LearningAnchor::Project | LearningAnchor::File => (None, None),
            LearningAnchor::Hunk { index } => (Some(index), None),
            LearningAnchor::Lines { start, end } => (Some(start), Some(end)),
        }
    }

    /// Rebuild an anchor from its persisted parts. Unknown kinds and
    /// range-less `lines` rows fall back to [`LearningAnchor::File`] rather
    /// than failing the load — a slightly coarse anchor beats a lost note.
    pub fn from_parts(kind: &str, start: Option<usize>, end: Option<usize>) -> Self {
        match kind {
            "project" => LearningAnchor::Project,
            "hunk" => match start {
                Some(index) => LearningAnchor::Hunk { index },
                None => LearningAnchor::File,
            },
            "lines" => match (start, end) {
                (Some(start), Some(end)) => LearningAnchor::Lines { start, end },
                (Some(start), None) => LearningAnchor::Lines { start, end: start },
                _ => LearningAnchor::File,
            },
            _ => LearningAnchor::File,
        }
    }

    /// The 1-based inclusive line range this anchor actually names, for prose
    /// that quotes it back. Unlike [`line_range`](Self::line_range) — which is
    /// the persistence shape and reuses `line_start` to hold a hunk index —
    /// this is `None` for every anchor that does not cover specific lines.
    pub fn line_range_for_display(self) -> Option<(usize, usize)> {
        match self {
            LearningAnchor::Lines { start, end } => Some((start, end)),
            _ => None,
        }
    }

    /// Plain-words description echoed above the question input, e.g.
    /// `lines 40-58 of src/app/learning.rs`.
    pub fn describe(self, path: Option<&str>) -> String {
        let path = path.unwrap_or("this file");
        match self {
            LearningAnchor::Project => "this whole project".to_string(),
            LearningAnchor::File => format!("all of {path}"),
            LearningAnchor::Hunk { index } => format!("change #{} in {path}", index + 1),
            LearningAnchor::Lines { start, end } if start == end => {
                format!("line {start} of {path}")
            }
            LearningAnchor::Lines { start, end } => format!("lines {start}-{end} of {path}"),
        }
    }
}

/// What became of a stored Q&A anchor when it was checked against the file as
/// it stands now.
///
/// Computed when the history loads and **never persisted**. The row's
/// `selection_text` is the evidence, so the verdict can always be re-derived,
/// and the stored `line_start`/`line_end` stay what they have always been: the
/// historical fact of where the question was asked. Overwriting them would
/// trade a recoverable answer for an unrecoverable one.
///
/// Absence of a verdict means "still where it was stored, as far as we can
/// tell" — which is also what a row with nothing to check against reports, so
/// the common case costs nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningAnchorDrift {
    /// The code moved and was found again, at this 1-based inclusive range.
    Reanchored { start: usize, end: usize },
    /// The code the question was asked about can no longer be pointed at.
    Lost(LearningAnchorLoss),
}

/// Why an anchor was given up on. Each reads differently to the user: a
/// deleted file is not the same event as code that was rewritten, and neither
/// is the same as code that now appears in several places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningAnchorLoss {
    /// The file itself is no longer in the working directory.
    FileGone,
    /// The file is there, but the selected text is not in it any more.
    NotFound,
    /// The selected text now appears more than once, so there is no honest
    /// way to say which copy the question was about.
    Ambiguous,
}

#[allow(dead_code)]
impl LearningAnchorDrift {
    /// Compact marker for the Q&A history row.
    pub fn marker(self) -> &'static str {
        match self {
            LearningAnchorDrift::Reanchored { .. } => "⚠ moved",
            LearningAnchorDrift::Lost(_) => "⚠ anchor lost",
        }
    }

    /// Whether this is a loss rather than a relocation — the two are coloured
    /// differently, because one of them still points at the right code.
    pub fn is_lost(self) -> bool {
        matches!(self, LearningAnchorDrift::Lost(_))
    }

    /// Full sentence for the answer pane, given the range the row was stored
    /// with. Says what happened *and* that the question and answer are intact,
    /// since a newcomer's reading of "anchor lost" is otherwise "this entry is
    /// broken".
    pub fn describe(self, stored: Option<(usize, usize)>) -> String {
        let was = match stored {
            Some((start, end)) if start == end => format!("line {start}"),
            Some((start, end)) => format!("lines {start}-{end}"),
            None => "this file".to_string(),
        };
        match self {
            LearningAnchorDrift::Reanchored { start, end } if start == end => {
                format!("The code has moved since this was asked: it was {was}, it is now line {start}.")
            }
            LearningAnchorDrift::Reanchored { start, end } => {
                format!(
                    "The code has moved since this was asked: it was {was}, it is now lines {start}-{end}."
                )
            }
            LearningAnchorDrift::Lost(LearningAnchorLoss::FileGone) => {
                "This file is no longer in the project, so there is nothing left to point at. The question and answer below are unchanged.".to_string()
            }
            LearningAnchorDrift::Lost(LearningAnchorLoss::NotFound) => {
                format!(
                    "The code this was asked about is no longer in the file, so {was} now shows something else. The question and answer below are unchanged."
                )
            }
            LearningAnchorDrift::Lost(LearningAnchorLoss::Ambiguous) => {
                "This code now appears in more than one place in the file, so there is no way to say which copy the question was about. The question and answer below are unchanged.".to_string()
            }
        }
    }
}

/// What the user is asking for. Chosen at ask time and re-labelable
/// afterwards; it shapes the prompt framing and which follow-up action the UI
/// offers first, nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningQaIntent {
    /// Teach me what this does. No change is proposed; the answer lives on as
    /// an anchored note.
    Explain,
    /// Propose a concrete change.
    Action,
}

#[allow(dead_code)]
impl LearningQaIntent {
    /// Stable string stored in `learning_qa.intent`.
    pub fn as_str(self) -> &'static str {
        match self {
            LearningQaIntent::Explain => "explain",
            LearningQaIntent::Action => "action",
        }
    }

    pub fn from_str(raw: &str) -> Self {
        match raw {
            "action" => LearningQaIntent::Action,
            _ => LearningQaIntent::Explain,
        }
    }

    /// The label the ask keys carry in the UI.
    pub fn label(self) -> &'static str {
        match self {
            LearningQaIntent::Explain => "Explain this to me",
            LearningQaIntent::Action => "Ask for a change",
        }
    }

    /// Compact marker + word shown on a Q&A row.
    pub fn marker(self) -> &'static str {
        match self {
            LearningQaIntent::Explain => "? explain",
            LearningQaIntent::Action => "! change",
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            LearningQaIntent::Explain => LearningQaIntent::Action,
            LearningQaIntent::Action => LearningQaIntent::Explain,
        }
    }
}

/// How much the answer should assume. A per-session setting, not a
/// per-question one; it changes prompt wording only — never tools, model, or
/// which files are visible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningLevel {
    /// Default: assume no prior knowledge of this codebase, define jargon,
    /// end with a "Where to look next" pointer.
    Newcomer,
    /// Denser answers for a user who has outgrown the newcomer framing.
    Familiar,
}

#[allow(dead_code)]
impl LearningLevel {
    /// Stable string stored in `learning_sessions.level` / `learning_qa.level`.
    pub fn as_str(self) -> &'static str {
        match self {
            LearningLevel::Newcomer => "newcomer",
            LearningLevel::Familiar => "familiar",
        }
    }

    pub fn from_str(raw: &str) -> Self {
        match raw {
            "familiar" => LearningLevel::Familiar,
            _ => LearningLevel::Newcomer,
        }
    }

    pub fn toggled(self) -> Self {
        match self {
            LearningLevel::Newcomer => LearningLevel::Familiar,
            LearningLevel::Familiar => LearningLevel::Newcomer,
        }
    }
}

/// Whether an answer came from the fast no-tools pass or the slower pass that
/// lets the agent read the rest of the repo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningRunMode {
    /// `HeadlessRunner::run(.., restricted = true)` — answered from the
    /// prompt's own context only.
    NoTools,
    /// `HeadlessRunner::run_read_only` — the agent may read the repository.
    DeepDive,
}

#[allow(dead_code)]
impl LearningRunMode {
    /// Stable string stored in `learning_qa.run_mode`.
    pub fn as_str(self) -> &'static str {
        match self {
            LearningRunMode::NoTools => "no_tools",
            LearningRunMode::DeepDive => "deep_dive",
        }
    }

    pub fn from_str(raw: &str) -> Self {
        match raw {
            "deep_dive" => LearningRunMode::DeepDive,
            _ => LearningRunMode::NoTools,
        }
    }

    /// The mode `harness` can actually deliver.
    ///
    /// Codex has no no-tools headless invocation: `codex exec` is always an
    /// ephemeral read-only sandbox that can read the whole repository, and
    /// `HeadlessRunner::run` ignores `restricted` for it. Asking for
    /// [`NoTools`](Self::NoTools) there would run a repo-reading agent while
    /// the row claimed "this file only", so the request is downgraded to
    /// [`DeepDive`](Self::DeepDive) before it is recorded or run — the label,
    /// the stored row, and the command then all say the same thing.
    pub fn effective_for(self, harness: &AgentKind) -> Self {
        match (self, harness) {
            (LearningRunMode::NoTools, AgentKind::Codex) => LearningRunMode::DeepDive,
            _ => self,
        }
    }

    /// What the mode does, in the user's terms rather than AMF's.
    pub fn description(self) -> &'static str {
        match self {
            LearningRunMode::NoTools => "this file only",
            LearningRunMode::DeepDive => "read the repo",
        }
    }
}

/// Lifecycle of one queued question. Rendered as a full word, never a glyph
/// alone — a stalled screen must not be ambiguous between "thinking" and
/// "broken".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningQaStatus {
    /// Enqueued, no thread started yet.
    Pending,
    /// A headless run is in flight.
    Running,
    /// An answer arrived.
    Answered,
    /// The run failed; `LearningQa::error` carries what to do about it.
    Failed,
}

#[allow(dead_code)]
impl LearningQaStatus {
    /// Stable string stored in `learning_qa.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            LearningQaStatus::Pending => "pending",
            LearningQaStatus::Running => "running",
            LearningQaStatus::Answered => "answered",
            LearningQaStatus::Failed => "failed",
        }
    }

    pub fn from_str(raw: &str) -> Self {
        match raw {
            "running" => LearningQaStatus::Running,
            "answered" => LearningQaStatus::Answered,
            "failed" => LearningQaStatus::Failed,
            _ => LearningQaStatus::Pending,
        }
    }

    /// The word shown on the row.
    pub fn word(self) -> &'static str {
        match self {
            LearningQaStatus::Pending => "queued",
            LearningQaStatus::Running => "thinking…",
            LearningQaStatus::Answered => "answered",
            LearningQaStatus::Failed => "failed",
        }
    }

    /// True while the user is still waiting on this row (drives the header's
    /// in-flight counter).
    pub fn is_in_flight(self) -> bool {
        matches!(self, LearningQaStatus::Pending | LearningQaStatus::Running)
    }
}

/// One question and its answer, anchored to a place in the project. The
/// in-memory list is the overlay's source of truth; the DB persists it when
/// one is available (mirroring the TODOs overlay).
#[derive(Debug, Clone, PartialEq)]
pub struct LearningQa {
    pub id: String,
    /// `learning_sessions.id` this row belongs to.
    pub session_id: String,
    /// Set on a follow-up: the row whose question and answer are carried into
    /// this one's prompt. Follow-ups render indented under their parent.
    ///
    /// Also set on a deep dive, which hangs under the answer it re-derives —
    /// see [`deep_dive_of`](Self::deep_dive_of) for why that one is *not* a
    /// conversational parent.
    pub parent_qa_id: Option<String>,
    /// Set only on a deep dive: the row this one re-ran.
    ///
    /// A deep dive is threaded under its origin so the two read as a pair, but
    /// it *replaces* that answer rather than continuing from it. Without this
    /// field the two relationships are indistinguishable — a follow-up on a
    /// deep dive would walk `parent_qa_id` straight back into the shallow
    /// answer the deep dive was run to check, feeding possibly-fabricated
    /// claims into the prompt that was meant to be free of them. It cannot be
    /// inferred from `run_mode` either: every Codex row is a `DeepDive` (see
    /// [`LearningRunMode::effective_for`]), including ordinary follow-ups
    /// whose ancestry must be kept.
    pub deep_dive_of: Option<String>,
    /// Repo-relative path, `None` for the project-level anchor.
    pub file_path: Option<String>,
    pub anchor: LearningAnchor,
    /// The text the anchor covered when the question was asked. Kept verbatim
    /// so the answer stays readable even after the file moves on.
    pub selection_text: String,
    /// Whether [`selection_text`](Self::selection_text) is a unified-diff
    /// excerpt. Stored rather than re-derived: a line anchor from the repo tree
    /// and one from a diff are indistinguishable once the browse scope is gone,
    /// and a follow-up needs to label its parent's capture correctly however
    /// far the file list has moved on since.
    pub selection_is_diff: bool,
    pub question: String,
    pub intent: LearningQaIntent,
    /// The level this row was answered at, so a reloaded answer explains why
    /// it reads the way it does.
    pub level: LearningLevel,
    pub answer: Option<String>,
    pub harness: AgentKind,
    pub run_mode: LearningRunMode,
    pub status: LearningQaStatus,
    /// Failure text for a `Failed` row, phrased as what to do next.
    pub error: Option<String>,
    /// `todos.id`, set only once the user explicitly made this answer
    /// actionable. Renders as `→ TODO` and makes re-invocation jump to the
    /// item instead of duplicating it.
    pub todo_id: Option<String>,
    /// `FeatureSession.id` of a live session escalated from this row.
    pub spawned_session_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl LearningQa {
    /// The row this one stands in for, when it is a deep dive threaded under
    /// the answer it re-derived.
    ///
    /// Ancestor traversal uses this to step *over* that row: the deep dive
    /// occupies its position in the conversation, so the answer it was run to
    /// check is not a turn that ever happened. Only honoured when it is also
    /// this row's thread parent, which is the only shape
    /// [`App::learning_deep_dive`](crate::app::App::learning_deep_dive)
    /// writes — a mismatch means the ancestry never runs through it anyway.
    pub fn superseded_id(&self) -> Option<&str> {
        match (self.deep_dive_of.as_deref(), self.parent_qa_id.as_deref()) {
            (Some(origin), Some(parent)) if origin == parent => Some(origin),
            _ => None,
        }
    }
}

/// A Learning Mode session: one per project, carrying the settings that
/// outlive a single question.
#[derive(Debug, Clone, PartialEq)]
pub struct LearningSession {
    pub id: String,
    pub project_id: String,
    /// Feature the session was opened under (its workdir is what gets read).
    pub feature_id: String,
    pub title: String,
    pub harness: AgentKind,
    pub level: LearningLevel,
    /// False until the first-open help overlay has been shown once.
    pub onboarding_seen: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Which group a file-list row belongs to.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningListGroup {
    /// The pinned orientation group shown at the top of repo-tree scope until
    /// the project has some Q&A history.
    StartHere,
    /// The ordinary file list.
    Files,
}

/// One row in Learning Mode's file list.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearningListEntry {
    /// Collapsible header for the `Start here` group.
    StartHereHeader,
    /// The repo-level orientation question — anchors to the project rather
    /// than to any file.
    ProjectTour,
    /// A directory in the repo tree. Navigation only: a directory is not a
    /// question anchor, so resting on one leaves the loaded file and the
    /// anchor exactly where they were. Only ever built in repo-tree scope.
    Dir {
        /// Repo-relative path with no trailing slash, e.g. `src/app`. This is
        /// the key `LearningViewState::expanded_dirs` stores.
        path: String,
        /// Nesting depth; 0 for a top-level directory.
        depth: usize,
        expanded: bool,
        /// Files anywhere beneath this directory, so a collapsed row can still
        /// say how much it is hiding.
        file_count: usize,
        /// Children not listed because this one directory exceeded
        /// `MAX_DIR_CHILDREN`. Non-zero rows say so rather than looking
        /// complete — the whole-listing cap this replaced had the same duty.
        truncated: usize,
    },
    /// A file. `diff_index` indexes `LearningViewState::diff_files` in
    /// branch-changes scope and is `None` in repo-tree scope. `depth` is the
    /// tree indent; it is 0 for the flat branch-changes list and for the
    /// `Start here` group, neither of which is a tree.
    File {
        path: String,
        group: LearningListGroup,
        diff_index: Option<usize>,
        depth: usize,
    },
}

#[allow(dead_code)]
impl LearningListEntry {
    /// The repo-relative path this row loads, if it loads one. Deliberately
    /// `None` for a directory: this is what the content pane and the anchor
    /// follow, and a directory must move neither.
    pub fn path(&self) -> Option<&str> {
        match self {
            LearningListEntry::File { path, .. } => Some(path.as_str()),
            _ => None,
        }
    }

    /// The directory this row is, if it is one.
    pub fn dir_path(&self) -> Option<&str> {
        match self {
            LearningListEntry::Dir { path, .. } => Some(path.as_str()),
            _ => None,
        }
    }

    /// A stable identity for the row, used to put the cursor back on the same
    /// thing after the list is rebuilt. Unlike `path()` this covers
    /// directories, because collapsing one must leave the cursor on it.
    pub fn row_key(&self) -> Option<(bool, &str)> {
        match self {
            LearningListEntry::Dir { path, .. } => Some((true, path.as_str())),
            LearningListEntry::File { path, .. } => Some((false, path.as_str())),
            _ => None,
        }
    }

    /// How far the row is indented in the tree.
    pub fn depth(&self) -> usize {
        match self {
            LearningListEntry::Dir { depth, .. } | LearningListEntry::File { depth, .. } => *depth,
            _ => 0,
        }
    }

    /// Whether the cursor can rest here (group headers are skipped by
    /// navigation only when collapsed — they stay selectable so the group can
    /// be expanded again).
    pub fn is_file(&self) -> bool {
        matches!(self, LearningListEntry::File { .. })
    }
}

/// Which pane has focus in the Learning Mode overlay.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LearningFocus {
    FileList,
    Content,
    Qa,
}

/// An open starter-question picker: indices into
/// `crate::app::learning::STARTER_QUESTIONS`, filtered to the ones that make
/// sense for the current anchor. Picking one fills the prompt so it can still
/// be edited before it's asked.
#[allow(dead_code)]
pub struct LearningStarterPicker {
    pub indices: Vec<usize>,
    pub selected: usize,
}

/// Which harness answers questions from here on. Pre-selected when the
/// overlay opens, so this only exists while the user is actively changing it.
#[allow(dead_code)]
pub struct LearningHarnessPicker {
    pub harnesses: Vec<AgentKind>,
    pub selected: usize,
}

/// An open question prompt. The same editor serves both intents and
/// follow-ups; the title bar shows the resolved anchor and chosen intent.
#[allow(dead_code)]
pub struct LearningQuestionEditor {
    pub editor: TextEditor,
    pub intent: LearningQaIntent,
    /// Set when this prompt is a follow-up to an existing row.
    pub parent_qa_id: Option<String>,
    /// Anchor captured when the prompt opened, so browsing can't move it.
    pub anchor: LearningAnchor,
    pub file_path: Option<String>,
    /// The anchored text captured alongside `anchor`.
    pub selection_text: String,
    /// Whether `selection_text` is a unified-diff excerpt, captured with it so
    /// the prompt labels it the same way however the browse scope changes
    /// before the question is submitted.
    pub selection_is_diff: bool,
    pub scroll: usize,
    pub sync_to_cursor: bool,
}

/// An open "add this answer to the project's TODO list" confirmation.
///
/// Lives inside [`LearningViewState`] like the pickers rather than as its own
/// `AppMode`, so cancelling returns to exactly the browsing state underneath.
/// Nothing is written until it is confirmed: the seeded title is a guess, and
/// this mode's audience is the least likely to notice a wrong one going in
/// behind their back.
#[allow(dead_code)]
pub struct LearningActionEditor {
    /// The Q&A row the item is being made from.
    pub qa_id: String,
    /// Editable title, seeded from the answer. An explanation has no one-line
    /// summary in it, so the seed there is a truncation the user is expected to
    /// fix — which is most of why this dialog exists at all.
    pub title: TextEditor,
    /// The note's body: where the question was anchored, what was asked, and an
    /// excerpt of the answer. Shown but not edited here, so what gets written
    /// is never a surprise.
    pub body: String,
    /// Refusal raised by a key pressed *in* the dialog (an emptied title). Kept
    /// here rather than on the overlay because the dialog covers the overlay's
    /// banner line.
    pub error: Option<String>,
    pub scroll: usize,
    pub sync_to_cursor: bool,
}

/// State for the Learning Mode overlay (`AppMode::Learning`) — a read-only
/// file browser over a project, with an agent answering questions about
/// whatever the cursor is on. Nothing in this mode writes to the repository.
#[allow(dead_code)]
pub struct LearningViewState {
    /// Project being studied.
    pub project_id: String,
    /// Project / feature indices the overlay was opened from, used to resolve
    /// the feature for escalation and to restore dashboard selection on close.
    pub pi: usize,
    pub fi: usize,
    /// Display labels for the header.
    pub project_name: String,
    pub feature_name: String,
    /// The feature's working directory — everything is read from here.
    pub workdir: PathBuf,
    /// False for non-git projects, where branch-changes scope has no meaning
    /// and the file list falls back to a capped plain walk.
    pub is_git: bool,
    pub scope: BrowseScope,
    /// File-list rows in display order (`Start here` group first, when shown).
    pub entries: Vec<LearningListEntry>,
    pub selected_entry: usize,
    pub list_scroll: usize,
    pub start_here_collapsed: bool,
    /// Which repo-tree directories are expanded, by repo-relative path. The
    /// tree is rebuilt from this on every reload, so it — not `entries` — is
    /// what expansion state actually lives in. Seeded on open with the
    /// ancestors of the `Start here` candidates, so `src/` is open at the file
    /// a newcomer is most likely to want.
    pub expanded_dirs: std::collections::BTreeSet<String>,
    /// Whether that seeding has happened. It runs once per overlay, so a later
    /// reload can't re-open a directory the user deliberately closed.
    pub expanded_seeded: bool,
    /// The repo's flat path list, kept so expanding or collapsing a directory
    /// rebuilds `entries` from memory instead of shelling out to `git ls-files`
    /// again. `entries` is derived from this plus `expanded_dirs`; this is the
    /// input, and it only changes when the listing is genuinely re-read.
    pub repo_files: Vec<String>,
    /// The surviving `Start here` candidates, cached for the same reason.
    pub start_here: Vec<String>,
    /// Diff snapshot backing `BrowseScope::BranchChanges`.
    pub diff_files: Vec<crate::diff::DiffFile>,
    /// Lines of the loaded file, and the path they came from.
    pub content: Vec<String>,
    pub content_path: Option<String>,
    pub content_scroll: usize,
    /// Why the selected file could not be shown (binary, too large, unreadable).
    pub content_error: Option<String>,
    /// Cursor into the content pane: a 0-based index into `content` in
    /// repo-tree scope, or into the file's `addressable_lines()` in
    /// branch-changes scope.
    pub cursor_line: usize,
    /// Start of an in-progress multi-line selection; `None` selects only the
    /// cursor line.
    pub selection_anchor: Option<usize>,
    /// The anchor a question would currently be asked against.
    pub anchor: LearningAnchor,
    pub focus: LearningFocus,
    /// Open question prompt, if any.
    pub question: Option<LearningQuestionEditor>,
    /// Q&A history for this project, oldest first, follow-ups after parents.
    pub qa: Vec<LearningQa>,
    /// Anchors that no longer point where they were stored, by `LearningQa::id`.
    ///
    /// Deliberately a side table rather than a field on the row: a verdict is a
    /// judgment about the working directory as it is right now, not something
    /// the row carries, and keeping the two apart is what stops it being
    /// written back over the range the question was actually asked at. A row
    /// with no entry here is anchored as stored.
    pub anchor_drift: std::collections::HashMap<String, LearningAnchorDrift>,
    pub selected_qa: usize,
    pub qa_scroll: usize,
    /// Answer pane state — offset plus the render cache
    /// `draw_markdown_document` needs.
    pub answer_open: bool,
    pub answer_scroll: usize,
    pub answer_rendered_width: u16,
    pub answer_rendered_lines: Vec<ratatui::text::Line<'static>>,
    /// Harness answering questions. Pre-selected, so the picker is optional.
    pub harness: AgentKind,
    /// Open harness picker, if any. Lives inside the overlay rather than as
    /// its own `AppMode` so opening it can't lose the browsing state behind it.
    pub harness_picker: Option<LearningHarnessPicker>,
    /// Open starter-question picker, if any.
    pub starter_picker: Option<LearningStarterPicker>,
    /// Open "add this to the TODO list" confirmation, if any.
    pub action_editor: Option<LearningActionEditor>,
    pub level: LearningLevel,
    /// `learning_sessions.id` backing this overlay.
    pub session_id: String,
    /// True while the `?` help overlay is open (also shown automatically on
    /// first open, per `onboarding_seen`).
    pub help_open: bool,
    pub help_scroll: usize,
    /// Transient error banner (file load, DB, run dispatch).
    pub error: Option<String>,
    /// Transient confirmation banner — what a key just *did*, as opposed to
    /// why it refused. Shares the error's line but not its colour: telling
    /// someone their entry was re-filed in the failure red is its own small
    /// lie, and this mode's audience is the least equipped to discount it.
    pub notice: Option<String>,
    /// The Q&A row `notice` was raised on, when it describes one. The wording
    /// is only true of that row as it stood at the keypress ("the answer on
    /// its way was asked for as an explanation"), so the banner is dropped
    /// when the cursor leaves the row or the row's run lands.
    pub notice_qa_id: Option<String>,
}

#[allow(dead_code)]
impl LearningViewState {
    /// How many answers are still generating — shown in the header so a slow
    /// run reads as progress rather than a hang.
    pub fn in_flight_count(&self) -> usize {
        self.qa.iter().filter(|q| q.status.is_in_flight()).count()
    }

    /// The path a question would anchor to, `None` for the project anchor.
    pub fn anchor_path(&self) -> Option<&str> {
        match self.anchor {
            LearningAnchor::Project => None,
            _ => self.content_path.as_deref(),
        }
    }

    /// The currently selected file-list entry.
    pub fn selected_entry(&self) -> Option<&LearningListEntry> {
        self.entries.get(self.selected_entry)
    }

    /// The `DiffFile` behind the selected entry, in branch-changes scope.
    pub fn selected_diff_file(&self) -> Option<&crate::diff::DiffFile> {
        match self.entries.get(self.selected_entry) {
            Some(LearningListEntry::File {
                diff_index: Some(i),
                ..
            }) => self.diff_files.get(*i),
            _ => None,
        }
    }

    /// Whether hunk selection is available — it needs a diff, so repo-tree
    /// scope has none.
    pub fn hunk_selection_available(&self) -> bool {
        self.scope == BrowseScope::BranchChanges && self.selected_diff_file().is_some()
    }

    /// The Q&A row under the history cursor.
    pub fn selected_qa(&self) -> Option<&LearningQa> {
        self.qa.get(self.selected_qa)
    }

    /// Drop the confirmation banner and whatever row it was raised on.
    pub fn clear_notice(&mut self) {
        self.notice = None;
        self.notice_qa_id = None;
    }

    /// Move the history cursor to `index`, dropping a banner raised on the row
    /// being left.
    ///
    /// A notice describes the row it was raised on ("re-filed as a change
    /// request"), so it must not follow the cursor onto a different entry and
    /// appear to describe that one instead. Every cursor move goes through
    /// here — including the programmatic ones (a follow-up selecting its new
    /// row, a deep dive jumping to the one that already exists), which is
    /// where a notice would otherwise survive untouched.
    pub fn select_qa(&mut self, index: usize) {
        if index != self.selected_qa {
            self.clear_notice();
        }
        self.selected_qa = index;
    }
}

pub enum AppMode {
    Normal,
    Todos(TodoViewState),
    /// Read-only Learning Mode overlay: browse the project and ask an agent
    /// about what you're looking at (`crate::app::learning`).
    #[allow(dead_code)] // Entered by the plan's Epic 4 dashboard key.
    Learning(Box<LearningViewState>),
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
    StoppedSessionDialog(StoppedSessionDialogState),
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
    /// Soft warning shown before starting an agent when the machine is already
    /// at the concurrency cap and/or low on memory. Confirming starts anyway.
    ConfirmResourceStart(Box<ResourceConfirmState>),
    /// Features that are idle and unattended, with per-row reclaim actions.
    Dormant(DormantViewState),
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
    pub icon_picker: Option<ConfigWizardIconPicker>,
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

#[derive(Debug, Clone, Copy)]
pub struct ConfigWizardIconPicker {
    pub selected: usize,
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

pub enum ThemePickerEntry {
    Theme(crate::theme::ThemeName),
    Group {
        label: &'static str,
        themes: Vec<crate::theme::ThemeName>,
    },
}

impl ThemePickerEntry {
    /// The theme this entry previews when highlighted: itself for a single
    /// theme, or the first member for a group (a representative peek).
    pub fn preview_theme(&self) -> Option<crate::theme::ThemeName> {
        match self {
            ThemePickerEntry::Theme(name) => Some(*name),
            ThemePickerEntry::Group { themes, .. } => themes.first().copied(),
        }
    }

    /// The full set of top-level entries, with subtype-heavy families
    /// (Catppuccin, Gruvbox Material) collapsed into groups so they don't
    /// clog the single-level list.
    pub fn build() -> Vec<Self> {
        use crate::theme::ThemeName::*;

        vec![
            ThemePickerEntry::Theme(Default),
            ThemePickerEntry::Theme(Amf),
            ThemePickerEntry::Theme(Dracula),
            ThemePickerEntry::Theme(Nord),
            ThemePickerEntry::Theme(GruvboxDark),
            ThemePickerEntry::Theme(GruvboxLight),
            ThemePickerEntry::Group {
                label: "Catppuccin",
                themes: vec![
                    CatppuccinLatte,
                    CatppuccinFrappe,
                    CatppuccinMacchiato,
                    CatppuccinMocha,
                ],
            },
            ThemePickerEntry::Group {
                label: "Gruvbox Material",
                themes: vec![
                    GruvboxMaterialDarkHard,
                    GruvboxMaterialDarkMedium,
                    GruvboxMaterialDarkSoft,
                    GruvboxMaterialLightHard,
                    GruvboxMaterialLightMedium,
                    GruvboxMaterialLightSoft,
                    GruvboxMaterialMixDarkHard,
                    GruvboxMaterialMixDarkMedium,
                    GruvboxMaterialMixDarkSoft,
                    GruvboxMaterialMixLightHard,
                    GruvboxMaterialMixLightMedium,
                    GruvboxMaterialMixLightSoft,
                    GruvboxMaterialOriginalDarkHard,
                    GruvboxMaterialOriginalDarkMedium,
                    GruvboxMaterialOriginalDarkSoft,
                    GruvboxMaterialOriginalLightHard,
                    GruvboxMaterialOriginalLightMedium,
                    GruvboxMaterialOriginalLightSoft,
                ],
            },
        ]
    }
}

/// Second-screen state for a group entry drilled into from the top-level
/// list; `None` means the picker is showing the top-level list.
pub struct ThemePickerGroupState {
    pub label: &'static str,
    pub themes: Vec<crate::theme::ThemeName>,
    pub selected: usize,
}

pub struct ThemePickerState {
    pub selected: usize,
    pub entries: Vec<ThemePickerEntry>,
    pub original_theme: crate::theme::ThemeName,
    /// The theme currently rendered on screen (live preview). Drilling into
    /// a group lands on this theme when it's a member, so where `Enter` puts
    /// the cursor always matches what the user is already looking at.
    pub previewed: crate::theme::ThemeName,
    pub group: Option<ThemePickerGroupState>,
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

/// The dormant-features overlay: features that are idle *and* unattended, with
/// what each is still holding.
pub struct DormantViewState {
    pub features: Vec<crate::app::dormant::DormantFeature>,
    pub selected: usize,
    /// Result of the last action, shown in the overlay's footer.
    pub message: Option<String>,
}

impl DormantViewState {
    pub fn selected_feature(&self) -> Option<&crate::app::dormant::DormantFeature> {
        self.features.get(self.selected)
    }

    /// Keep the cursor on a real row after the list shrinks.
    pub fn clamp_selection(&mut self) {
        if self.selected >= self.features.len() {
            self.selected = self.features.len().saturating_sub(1);
        }
    }
}

/// A harness start paused on the resource-gate confirmation, replayed verbatim
/// if the user confirms and dropped if they cancel.
#[derive(Debug, Clone, PartialEq)]
pub enum PendingStart {
    /// Starting a stopped feature (`c` on the dashboard).
    Feature { pi: usize, fi: usize },
    /// Adding a session to a feature (session picker) that will spawn a
    /// harness — either the session itself, or the stopped feature's saved
    /// agents coming up underneath it.
    BuiltinSession {
        pi: usize,
        fi: usize,
        kind: SessionKind,
        label: Option<String>,
    },
    /// Opening a stopped feature or session from the dashboard (`Enter`).
    /// Replayed against the current selection, which the dialog leaves alone.
    EnterView { auto_compose: bool },
    /// Jumping to a stopped feature from inside a session view (leader n/p).
    SwitchViewToFeature { pi: usize, fi: usize },
}

/// The pre-start warning: what tripped, what it was about to do, and where to
/// go back to afterwards.
pub struct ResourceConfirmState {
    pub over_limit: Option<crate::app::resource_gate::OverLimit>,
    pub low_memory: Option<crate::app::resource_gate::LowMemory>,
    /// Editor windows open right now, collected only when the memory half
    /// tripped: they are not agents and are not counted as such, but they are
    /// usually the larger half of where the memory went.
    pub open_editors: Vec<String>,
    pub pending: PendingStart,
    /// Session view to restore after confirming or cancelling, when the start
    /// was initiated from inside an embedded session rather than the dashboard.
    pub from_view: Option<ViewState>,
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
    /// Optional composer seed to show immediately after the agent starts.
    pub startup_prompt: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanInterviewPhase {
    /// A saved draft for this interview was found on entry and the user must
    /// choose to resume or discard it before any questions are shown.
    ResumePrompt,
    Brief,
    StaticQuestions,
    /// The static question flow is complete and the user must explicitly
    /// choose whether to spend agent tokens on adaptive follow-ups.
    AiConsent,
    /// A background AI-adaptive round is in flight (`App::poll_plan_interview_ai_bg`).
    /// Question navigation is frozen; `current_question()` returns `None`.
    AiLoading,
    /// The completed interview is being synthesized into structured markdown
    /// by a background headless call.
    SynthesisLoading,
    /// The proposed plan is rendered as markdown and awaits an explicit
    /// accept, edit, regenerate, review, or abort action.
    Review,
    /// The proposed plan is open as raw markdown in the shared text editor.
    Editing,
    /// The user is composing a free-form instruction for the planning agent.
    DirectedFeedback,
    /// A repository-aware, read-only revision from that instruction is in flight.
    DirectedFeedbackLoading,
    /// The user is identifying one or more plan questions that need a focused,
    /// context-isolated repository investigation.
    Investigation,
    /// Fresh read-only investigator contexts are gathering findings, after
    /// which a separate no-tools planning context merges them into the draft.
    InvestigationLoading,
    /// A background headless call is reviewing the draft plan.
    CritiqueLoading,
    /// An agent's advisory review of the draft plan is on screen. The plan
    /// itself is untouched unless the user asks for a revision from here.
    Critique,
    /// An on-demand plan was accepted for a feature whose agent session is
    /// already running, and the user is choosing whether to hand the kickoff
    /// prompt to that live session. The plan is already written by this point,
    /// so both answers are safe — only the handoff is in question.
    KickoffHandoff,
    /// Transient question-flow completion used while app-level code decides
    /// whether to run another adaptive round, synthesize, or use the fallback.
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanInterviewAdvanceError {
    BriefRequired,
    AnswerRequired,
}

/// The live agent session an accepted on-demand plan can be handed off to.
///
/// Identified by session **id** rather than by index: the accept saves the
/// store before the prompt is answered, and resolving the id again at send time
/// means a store that moved underneath cannot seed the wrong session's composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanKickoffTarget {
    pub session_id: String,
    /// The session's display label, so the prompt can name what it will type into.
    pub session_label: String,
    /// Where the plan was just written, shown alongside the offer and reused as
    /// the confirmation message when the handoff is declined.
    pub plan_path: PathBuf,
}

/// How the step on screen compares with the same step's answer in the feature's
/// last accepted interview. Only meaningful on a re-run, which pre-fills those
/// answers so keeping one is the default and changing it is deliberate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriorAnswerState {
    /// Still the previously accepted answer, verbatim.
    Kept,
    /// Edited this run; the previous answer is still restorable.
    Changed,
    /// Emptied this run, which records as a skip unless it is restored.
    Cleared,
}

/// In-memory state for one plan-mode discovery interview.
///
/// `pending_launch` is optional so the same state can also support on-demand
/// interviews for existing features.
pub struct PlanInterviewState {
    pub feature_name: String,
    /// The key this interview's draft and transcript are filed under in the
    /// `plan_interviews` table: the feature's id for an on-demand interview,
    /// or [`crate::plan_interview::pending_interview_key`] while the feature it
    /// plans does not exist yet.
    pub interview_key: String,
    pub phase: PlanInterviewPhase,
    pub questions: Vec<PlanQuestion>,
    pub question_index: usize,
    pub brief: String,
    pub answers: Vec<Option<String>>,
    pub editor: TextEditor,
    pub selected_option: usize,
    /// Where the accepted plan is written (`<workdir>/AMF_PLAN.md`). Held
    /// separately from `pending_launch` because an on-demand interview has an
    /// existing feature's workdir and no launch at all.
    pub workdir: PathBuf,
    pub pending_launch: Option<PreparedFeatureLaunch>,
    pub abort_confirmation: bool,
    /// The feature's configured agent, preferred as the AI-adaptive
    /// interviewer/synthesis engine before
    /// `HeadlessRunner::select_for_interview` falls back to another installed
    /// harness.
    pub preferred_harness: AgentKind,
    /// Resolved lazily on the first AI round attempt. `None` = not yet
    /// resolved; `Some(None)` = resolution was attempted and no
    /// headless-capable harness is available, so remaining AI work falls back
    /// to the raw Q&A plan; `Some(Some(harness))` = the engine powering AI
    /// work for the rest of this interview.
    pub ai_harness: Option<Option<AgentKind>>,
    /// Number of AI rounds that have finished (successfully or not),
    /// checked against [`crate::plan_interview::MAX_AI_ROUNDS`].
    pub ai_rounds_completed: usize,
    /// True only after the user explicitly accepts the token-use prompt.
    /// App-level round dispatch also checks this so no headless call can
    /// start from an accidental `Done` transition.
    pub ai_followups_opted_in: bool,
    /// Set when the user finishes early or declines the token-use prompt so
    /// the `Done` transition skips any remaining AI rounds.
    pub skip_ai_rounds: bool,
    /// Set by the explicit "draft plan now" action. This permits the final
    /// headless pass without opting into adaptive rounds while preserving the
    /// consent screen's guarantee that ordinary completion spends no tokens.
    pub synthesis_requested: bool,
    /// When set, an AI round is in flight; used to render elapsed time on
    /// the `AiLoading` frame. `None` outside `AiLoading`.
    pub ai_round_started_at: Option<std::time::Instant>,
    /// Cheap token estimate for the in-flight round's prompt, shown on the
    /// `AiLoading` frame (`app::pr_review::estimate_tokens` is a chars/4
    /// heuristic, not a harness-reported count — no headless call currently
    /// surfaces real usage).
    pub ai_round_token_estimate: usize,
    /// True once plan synthesis has been started or deliberately bypassed
    /// because no headless engine is available. Prevents a failed plan-file
    /// write from spending tokens again when the user retries completion.
    pub synthesis_attempted: bool,
    /// The plan currently displayed at the review gate. App-level completion
    /// fills this with either valid synthesized markdown or the raw-Q&A
    /// fallback, and edits replace it before acceptance.
    pub synthesized_plan: Option<String>,
    /// Start time and prompt-size estimate for the synthesis loading frame.
    pub synthesis_started_at: Option<std::time::Instant>,
    pub synthesis_token_estimate: usize,
    /// Cached markdown-viewer layout for the review gate.
    pub review_scroll_offset: usize,
    pub review_rendered_width: u16,
    pub review_rendered_lines: Vec<ratatui::text::Line<'static>>,
    pub edit_scroll_offset: usize,
    pub edit_sync_to_cursor: bool,
    /// Start time and prompt-size estimate for a directed revision. The
    /// instruction itself remains in `editor` while loading so a failed call
    /// can return it intact for retrying or adjustment.
    pub directed_feedback_started_at: Option<std::time::Instant>,
    pub directed_feedback_token_estimate: usize,
    /// Start time and aggregate prompt-size estimate for the isolated
    /// investigation plus its separate no-tools merge pass.
    pub investigation_started_at: Option<std::time::Instant>,
    pub investigation_token_estimate: usize,
    /// An agent's advisory review of the plan currently at the review gate.
    /// Cleared whenever the plan changes, since the findings describe the
    /// draft they were written against.
    pub critique: Option<String>,
    /// Start time and prompt-size estimate for the agent-review loading frame.
    pub critique_started_at: Option<std::time::Instant>,
    pub critique_token_estimate: usize,
    /// Cached markdown-viewer layout for the advisory review.
    pub critique_scroll_offset: usize,
    pub critique_rendered_width: u16,
    pub critique_rendered_lines: Vec<ratatui::text::Line<'static>>,
    /// Advisory review staged as input for the next synthesis pass by the
    /// review's "revise" action. Consumed once that pass actually starts, so a
    /// revision that cannot run leaves the feedback recoverable.
    pub revision_critique: Option<String>,
    /// Bumped whenever `synthesized_plan` changes. A review is written against
    /// one revision, so a result that lands after the plan moved on can be
    /// recognized as stale without keeping a second copy of the plan.
    pub plan_revision: u64,
    /// The `plan_revision` the in-flight or displayed review describes.
    pub critique_plan_revision: Option<u64>,
    /// A saved draft found on entry, held while the user decides whether to
    /// resume or discard it. Taken by [`Self::resume_from_draft`]; dropped by
    /// [`Self::discard_draft`].
    pub resume_draft: Option<PlanInterviewRecord>,
    /// The brief from the feature's last accepted interview, pre-filled as this
    /// run's starting point. `None` unless this is a re-run of a feature that
    /// has an accepted transcript.
    pub prior_brief: Option<String>,
    /// Answers from that transcript, keyed by question id — the stable slug
    /// that survives a config change to the question bank. Kept after
    /// pre-filling so each question can say whether its answer is still the
    /// previous one ([`Self::prior_answer_state`]) and
    /// [`Self::restore_prior_answer`] can put it back.
    pub prior_answers: HashMap<String, String>,
    /// The live session an accepted on-demand plan is being offered to. Only
    /// set in [`PlanInterviewPhase::KickoffHandoff`], which is only reached
    /// after the plan file is already on disk.
    pub kickoff_handoff: Option<PlanKickoffTarget>,
}

impl PlanInterviewState {
    pub fn for_feature_creation(
        pending_launch: PreparedFeatureLaunch,
        questions: Vec<PlanQuestion>,
    ) -> Self {
        let feature_name = pending_launch.branch.clone();
        let interview_key = crate::plan_interview::pending_interview_key(
            &pending_launch.project_name,
            &feature_name,
        );
        Self::new(feature_name, interview_key, questions, Some(pending_launch))
    }

    /// An on-demand interview for a feature that already exists: no launch to
    /// defer, and the plan is written into the workdir the feature is already
    /// checked out in. Keyed by the feature's id, which is where an accepted
    /// transcript is filed, so a re-run finds the previous one.
    pub fn for_feature(
        feature_name: String,
        feature_id: String,
        questions: Vec<PlanQuestion>,
        workdir: PathBuf,
        agent: AgentKind,
    ) -> Self {
        let mut state = Self::new(feature_name, feature_id, questions, None);
        state.workdir = workdir;
        state.preferred_harness = agent;
        state
    }

    pub fn new(
        feature_name: String,
        interview_key: String,
        questions: Vec<PlanQuestion>,
        pending_launch: Option<PreparedFeatureLaunch>,
    ) -> Self {
        let answer_count = questions.len();
        let preferred_harness = pending_launch
            .as_ref()
            .map(|prepared| prepared.agent.clone())
            .unwrap_or_default();
        let workdir = pending_launch
            .as_ref()
            .map(|prepared| prepared.workdir.clone())
            .unwrap_or_default();
        Self {
            feature_name,
            interview_key,
            phase: PlanInterviewPhase::Brief,
            questions,
            question_index: 0,
            brief: String::new(),
            answers: vec![None; answer_count],
            editor: TextEditor::new(String::new()),
            selected_option: 0,
            workdir,
            pending_launch,
            abort_confirmation: false,
            preferred_harness,
            ai_harness: None,
            ai_rounds_completed: 0,
            ai_followups_opted_in: false,
            skip_ai_rounds: false,
            synthesis_requested: false,
            ai_round_started_at: None,
            ai_round_token_estimate: 0,
            synthesis_attempted: false,
            synthesized_plan: None,
            synthesis_started_at: None,
            synthesis_token_estimate: 0,
            review_scroll_offset: 0,
            review_rendered_width: 0,
            review_rendered_lines: Vec::new(),
            edit_scroll_offset: 0,
            edit_sync_to_cursor: false,
            directed_feedback_started_at: None,
            directed_feedback_token_estimate: 0,
            investigation_started_at: None,
            investigation_token_estimate: 0,
            critique: None,
            critique_started_at: None,
            critique_token_estimate: 0,
            critique_scroll_offset: 0,
            critique_rendered_width: 0,
            critique_rendered_lines: Vec::new(),
            revision_critique: None,
            plan_revision: 0,
            critique_plan_revision: None,
            resume_draft: None,
            prior_brief: None,
            prior_answers: HashMap::new(),
            kickoff_handoff: None,
        }
    }

    /// Ask whether the accepted plan should be handed to the feature's already
    /// running agent session.
    ///
    /// Only reached from an accepted on-demand interview: a feature-creation
    /// interview seeds the session it just launched without asking, and an
    /// on-demand accept with no live session has nothing to hand off to.
    pub fn offer_kickoff_handoff(&mut self, target: PlanKickoffTarget) {
        self.kickoff_handoff = Some(target);
        self.phase = PlanInterviewPhase::KickoffHandoff;
    }

    /// The directory headless interview calls run in and gather repo context
    /// from. Falls back to the process's cwd only when the interview was built
    /// without a workdir, which outside tests means neither a launch nor a
    /// feature was available to take one from.
    pub fn context_workdir(&self) -> PathBuf {
        if self.workdir.as_os_str().is_empty() {
            std::env::current_dir().unwrap_or_default()
        } else {
            self.workdir.clone()
        }
    }

    /// Hold a saved draft and ask the user whether to resume it, before any
    /// question is shown. Called on interview entry only, so the answers the
    /// draft would restore cannot overwrite answers given in this session.
    pub fn offer_resume(&mut self, draft: PlanInterviewRecord) {
        self.resume_draft = Some(draft);
        self.phase = PlanInterviewPhase::ResumePrompt;
    }

    /// Restore the held draft's brief, answers, and spent AI rounds, then land
    /// on the first question still unanswered.
    ///
    /// Answers are matched by question id, not position: the built-in bank and
    /// the project's `plan_questions` config may both have changed since the
    /// draft was saved, so anything the current bank no longer asks is dropped
    /// rather than mapped onto the wrong question. Stored AI-generated questions
    /// are appended instead, since those rounds were paid for and the current
    /// bank cannot contain them.
    pub fn resume_from_draft(&mut self) -> bool {
        let Some(draft) = self.resume_draft.take() else {
            return false;
        };

        self.brief = draft.brief.clone();
        self.adopt_recorded_answers(&draft);

        self.ai_rounds_completed = draft.ai_rounds_completed;
        // Rounds only run after an explicit opt-in, so a draft that spent one
        // carries that consent forward rather than re-asking for it.
        self.ai_followups_opted_in = draft.ai_rounds_completed > 0;

        // A draft abandoned at the review gate already has a paid-for plan.
        // Resume there rather than walking the questions again and synthesizing
        // a second time.
        if let Some(plan) = draft.plan {
            self.synthesized_plan = Some(plan);
            self.synthesis_attempted = true;
            self.phase = PlanInterviewPhase::Review;
            return true;
        }

        match self
            .answers
            .iter()
            .position(|answer| answer.as_deref().unwrap_or_default().trim().is_empty())
        {
            Some(index) => {
                self.phase = PlanInterviewPhase::StaticQuestions;
                self.question_index = index;
                self.load_current_answer();
            }
            // Every question already has an answer, so there is nothing to
            // resume *into*; go straight to the choice that follows them.
            None if self.ai_followups_opted_in => self.phase = PlanInterviewPhase::Done,
            None => self.phase = PlanInterviewPhase::AiConsent,
        }
        true
    }

    /// Fill this interview's answers from a stored record, matching by question
    /// **id** rather than position: the built-in bank and the project's
    /// `plan_questions` config may both have changed since the record was
    /// written, so anything the current bank no longer asks is dropped rather
    /// than mapped onto the wrong question. The record's AI-generated questions
    /// are appended instead of dropped — those rounds were paid for and the
    /// current bank cannot contain them.
    ///
    /// Matching by id is not enough on its own for a select question: config can
    /// rewrite the same id's options, leaving a stored answer that names a choice
    /// the question no longer offers. Such an answer is dropped rather than
    /// pre-filled, because it is unselectable in the UI and would otherwise reach
    /// the AI rounds and synthesis attached to the current question text.
    fn adopt_recorded_answers(&mut self, record: &PlanInterviewRecord) {
        self.answers = self
            .questions
            .iter()
            .map(|question| {
                record
                    .answer_for(&question.id)
                    .filter(|answer| question.accepts_answer(answer))
                    .map(str::to_string)
            })
            .collect();

        let known: HashSet<&str> = self.questions.iter().map(|q| q.id.as_str()).collect();
        let carried: Vec<(PlanQuestion, Option<String>)> = record
            .questions
            .iter()
            .enumerate()
            .filter(|(_, question)| {
                matches!(question.source, QuestionSource::Ai { .. })
                    && !known.contains(question.id.as_str())
            })
            .map(|(index, question)| {
                (
                    question.clone(),
                    record.answers.get(index).cloned().flatten(),
                )
            })
            .collect();
        for (question, answer) in carried {
            self.questions.push(question);
            self.answers.push(answer);
        }
    }

    /// Adopt the feature's last accepted interview as this run's starting point,
    /// so a re-run asks the same questions with the previous answers already in
    /// place: `Enter` keeps one, typing changes it, and
    /// [`Self::restore_prior_answer`] puts a changed one back.
    ///
    /// Returns whether anything was pre-filled, so the caller can say so rather
    /// than announcing a re-run that restored nothing.
    ///
    /// Spent AI rounds are deliberately *not* carried over: this is a new
    /// interview, so it gets its own consent step and its own round budget. The
    /// previous run's AI questions are still asked again (with their answers),
    /// since what the user told the interviewer about this feature is exactly
    /// the context the re-run should start from.
    pub fn apply_previous_transcript(&mut self, record: &PlanInterviewRecord) -> bool {
        self.adopt_recorded_answers(record);
        self.prior_brief = (!record.brief.trim().is_empty()).then(|| record.brief.clone());
        // Read back off the adopted answers rather than the record, so carried AI
        // questions are covered and answers the current bank rejected are not
        // remembered as the baseline: the keep/change note would call an untouched
        // question "changed", and `Ctrl+R` would offer to restore a value that
        // cannot be selected.
        self.prior_answers = self
            .questions
            .iter()
            .zip(self.answers.iter())
            .filter_map(|(question, answer)| {
                answer.clone().map(|answer| (question.id.clone(), answer))
            })
            .collect();

        self.brief = self.prior_brief.clone().unwrap_or_default();
        self.editor = TextEditor::new(self.brief.clone());
        self.prior_brief.is_some() || !self.prior_answers.is_empty()
    }

    /// Whether a previously accepted interview was pre-filled into this run.
    pub fn has_prior_answers(&self) -> bool {
        self.prior_brief.is_some() || !self.prior_answers.is_empty()
    }

    /// Drop the held draft and start the interview over. The caller deletes the
    /// stored row.
    ///
    /// Resets the collected brief and answers rather than only the held record:
    /// "discard and start over" has to mean the interview begins from its
    /// baseline, whatever the state held when the draft was offered. On a re-run
    /// that baseline is the previously accepted transcript, not a blank
    /// interview — discarding a stale draft must not also throw away the
    /// accepted answers it was revising.
    pub fn discard_draft(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::ResumePrompt {
            return false;
        }
        self.resume_draft = None;
        self.brief = self.prior_brief.clone().unwrap_or_default();
        let baseline = self
            .questions
            .iter()
            .map(|question| self.prior_answers.get(&question.id).cloned())
            .collect();
        self.answers = baseline;
        self.question_index = 0;
        self.selected_option = 0;
        self.phase = PlanInterviewPhase::Brief;
        self.editor = TextEditor::new(self.brief.clone());
        true
    }

    /// How the step on screen compares with the previously accepted answer for
    /// it, or `None` when there is no previous answer to compare against.
    pub fn prior_answer_state(&self) -> Option<PriorAnswerState> {
        let (prior, current) = match self.phase {
            PlanInterviewPhase::Brief => (self.prior_brief.as_deref()?, self.editor.text()),
            PlanInterviewPhase::StaticQuestions => {
                let question = self.questions.get(self.question_index)?;
                let prior = self.prior_answers.get(&question.id)?.as_str();
                match &question.kind {
                    PlanQuestionKind::FreeText => (prior, self.editor.text()),
                    PlanQuestionKind::Select(options) => (
                        prior,
                        options
                            .get(self.selected_option)
                            .map(String::as_str)
                            .unwrap_or_default(),
                    ),
                }
            }
            _ => return None,
        };

        Some(if current.trim() == prior.trim() {
            PriorAnswerState::Kept
        } else if current.trim().is_empty() {
            PriorAnswerState::Cleared
        } else {
            PriorAnswerState::Changed
        })
    }

    /// Put the previously accepted answer for the current step back, undoing an
    /// edit made this run. Returns false when there is nothing stored for this
    /// step, so the caller can say so rather than appearing to do nothing.
    pub fn restore_prior_answer(&mut self) -> bool {
        match self.phase {
            PlanInterviewPhase::Brief => {
                let Some(brief) = self.prior_brief.clone() else {
                    return false;
                };
                self.editor = TextEditor::new(brief);
                true
            }
            PlanInterviewPhase::StaticQuestions => {
                let Some((id, kind)) = self
                    .questions
                    .get(self.question_index)
                    .map(|question| (question.id.clone(), question.kind.clone()))
                else {
                    return false;
                };
                let Some(prior) = self.prior_answers.get(&id).cloned() else {
                    return false;
                };
                match &kind {
                    PlanQuestionKind::FreeText => {
                        self.editor = TextEditor::new(prior);
                        true
                    }
                    // Adoption keeps only answers the question still offers, so
                    // this normally finds one. The lookup stays defensive: an
                    // answer with nothing to select is reported as "nothing
                    // restored" rather than moving the highlight to option 0.
                    PlanQuestionKind::Select(options) => {
                        match options.iter().position(|option| *option == prior) {
                            Some(index) => {
                                self.selected_option = index;
                                true
                            }
                            None => false,
                        }
                    }
                }
            }
            _ => false,
        }
    }

    /// Snapshot the interview as a draft record for persistence.
    ///
    /// Deliberately a plain snapshot of what has been collected: the caller
    /// decides when a save is worth making, and re-saving the same state is
    /// harmless because the row is keyed by `(feature_id, stage)`.
    pub fn to_draft_record(&self) -> PlanInterviewRecord {
        PlanInterviewRecord {
            feature_id: self.interview_key.clone(),
            stage: PlanInterviewStage::Draft,
            feature_name: self.feature_name.clone(),
            brief: self.brief.clone(),
            questions: self.questions.clone(),
            answers: self.answers.clone(),
            // A draft holds the plan only once one has been generated, so
            // resuming after synthesis does not silently re-spend those tokens.
            plan: self.synthesized_plan.clone(),
            ai_rounds_completed: self.ai_rounds_completed,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    /// Move into the `AiLoading` phase while a background round runs.
    pub fn begin_ai_round(&mut self, token_estimate: usize) {
        self.phase = PlanInterviewPhase::AiLoading;
        self.ai_round_started_at = Some(std::time::Instant::now());
        self.ai_round_token_estimate = token_estimate;
    }

    /// Move into the synthesis loading phase while the final plan is
    /// generated off the UI thread.
    pub fn begin_synthesis(&mut self, token_estimate: usize) {
        self.synthesis_attempted = true;
        self.phase = PlanInterviewPhase::SynthesisLoading;
        self.synthesis_started_at = Some(std::time::Instant::now());
        self.synthesis_token_estimate = token_estimate;
    }

    /// Store the synthesized or fallback plan and stop at the review gate.
    ///
    /// A pass that returns the plan already on screen — the "keep the current
    /// plan" path taken when no headless engine is available — is not a plan
    /// change, so any review of that plan stays valid.
    pub fn apply_synthesis(&mut self, plan: String) {
        let changed = self.synthesized_plan.as_deref() != Some(plan.as_str());
        self.synthesis_attempted = true;
        self.synthesized_plan = Some(plan);
        self.synthesis_started_at = None;
        if changed {
            self.mark_plan_changed();
        }
        self.phase = PlanInterviewPhase::Review;
    }

    /// The scroll offset of whatever pane the current phase puts on screen, if
    /// that pane scrolls at all.
    ///
    /// Mouse-wheel events route through here so the wheel moves the plan (or
    /// the advisory review, or an instruction editor) rather than the dashboard
    /// list behind the dialog. `None` is a phase whose body always fits — the
    /// caller still swallows the event so the hidden selection cannot drift.
    /// Every one of these offsets is clamped by the renderer against the
    /// laid-out content, so this only ever has to move it.
    pub fn scroll_offset_mut(&mut self) -> Option<&mut usize> {
        if self.abort_confirmation {
            return None;
        }
        match self.phase {
            PlanInterviewPhase::Review => Some(&mut self.review_scroll_offset),
            PlanInterviewPhase::Critique => Some(&mut self.critique_scroll_offset),
            PlanInterviewPhase::Editing
            | PlanInterviewPhase::DirectedFeedback
            | PlanInterviewPhase::Investigation => Some(&mut self.edit_scroll_offset),
            _ => None,
        }
    }

    /// Open a blank multi-line instruction editor from the review gate.
    pub fn begin_directed_feedback(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Review || self.synthesized_plan.is_none() {
            return false;
        }
        self.editor = TextEditor::new(String::new());
        self.edit_scroll_offset = 0;
        self.edit_sync_to_cursor = true;
        self.phase = PlanInterviewPhase::DirectedFeedback;
        true
    }

    /// Freeze the directed-feedback editor while a read-only agent pass runs.
    pub fn begin_directed_feedback_loading(&mut self, token_estimate: usize) -> bool {
        if self.phase != PlanInterviewPhase::DirectedFeedback
            || self.synthesized_plan.is_none()
            || self.editor.text().trim().is_empty()
        {
            return false;
        }
        self.phase = PlanInterviewPhase::DirectedFeedbackLoading;
        self.directed_feedback_started_at = Some(std::time::Instant::now());
        self.directed_feedback_token_estimate = token_estimate;
        true
    }

    /// Return to the instruction editor after a failed revision, preserving
    /// what the user wrote so retrying does not require retyping it.
    pub fn fail_directed_feedback(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::DirectedFeedbackLoading {
            return false;
        }
        self.directed_feedback_started_at = None;
        self.phase = PlanInterviewPhase::DirectedFeedback;
        true
    }

    /// Leave directed feedback without changing the draft plan.
    pub fn cancel_directed_feedback(&mut self) -> bool {
        if !matches!(
            self.phase,
            PlanInterviewPhase::DirectedFeedback | PlanInterviewPhase::DirectedFeedbackLoading
        ) {
            return false;
        }
        self.directed_feedback_started_at = None;
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// Open a blank editor for research questions or plan sections that need
    /// repository evidence before the plan is accepted.
    pub fn begin_investigation(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Review || self.synthesized_plan.is_none() {
            return false;
        }
        self.editor = TextEditor::new(String::new());
        self.edit_scroll_offset = 0;
        self.edit_sync_to_cursor = true;
        self.phase = PlanInterviewPhase::Investigation;
        true
    }

    /// Freeze the research-focus editor while isolated investigators and the
    /// final no-tools merge pass run in the background.
    pub fn begin_investigation_loading(&mut self, token_estimate: usize) -> bool {
        if self.phase != PlanInterviewPhase::Investigation
            || self.synthesized_plan.is_none()
            || self.editor.text().trim().is_empty()
        {
            return false;
        }
        self.phase = PlanInterviewPhase::InvestigationLoading;
        self.investigation_started_at = Some(std::time::Instant::now());
        self.investigation_token_estimate = token_estimate;
        true
    }

    /// Return to the research-focus editor after any investigator or merge
    /// failure, preserving the request so the user can retry or narrow it.
    pub fn fail_investigation(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::InvestigationLoading {
            return false;
        }
        self.investigation_started_at = None;
        self.phase = PlanInterviewPhase::Investigation;
        true
    }

    /// Leave the optional investigation without changing the draft plan.
    pub fn cancel_investigation(&mut self) -> bool {
        if !matches!(
            self.phase,
            PlanInterviewPhase::Investigation | PlanInterviewPhase::InvestigationLoading
        ) {
            return false;
        }
        self.investigation_started_at = None;
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// Apply the context-isolated merge result and return to the review gate.
    pub fn apply_investigation_revision(&mut self, plan: String) {
        self.investigation_started_at = None;
        self.apply_synthesis(plan);
    }

    /// Move into the agent-review loading phase. Returns false outside the
    /// review gate so a stray keypress cannot start a paid call from a phase
    /// that has no plan to review.
    pub fn begin_critique(&mut self, token_estimate: usize) -> bool {
        if self.phase != PlanInterviewPhase::Review || self.synthesized_plan.is_none() {
            return false;
        }
        self.phase = PlanInterviewPhase::CritiqueLoading;
        self.critique_started_at = Some(std::time::Instant::now());
        self.critique_token_estimate = token_estimate;
        self.critique_plan_revision = Some(self.plan_revision);
        true
    }

    /// Show a finished advisory review. The plan is deliberately untouched.
    pub fn apply_critique(&mut self, critique: String) {
        self.critique = Some(critique);
        self.critique_started_at = None;
        self.critique_scroll_offset = 0;
        self.critique_rendered_width = 0;
        self.critique_rendered_lines.clear();
        self.critique_plan_revision = Some(self.plan_revision);
        self.phase = PlanInterviewPhase::Critique;
    }

    /// Keep a review that finished after the user dismissed it, without
    /// pulling them back into it. Returns false when there is nothing to keep
    /// or the plan moved on while the review was in flight, since the findings
    /// then describe a draft that is gone.
    pub fn stash_critique(&mut self, critique: String) -> bool {
        if self.phase != PlanInterviewPhase::Review
            || self.critique.is_some()
            || self.critique_plan_revision != Some(self.plan_revision)
        {
            return false;
        }
        self.critique = Some(critique);
        self.critique_started_at = None;
        self.critique_scroll_offset = 0;
        self.critique_rendered_width = 0;
        self.critique_rendered_lines.clear();
        true
    }

    /// Re-open the review already held for the current plan. This is what
    /// makes a dismissed review recoverable instead of leaving the user to pay
    /// for an identical second call.
    pub fn reopen_critique(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Review || self.critique.is_none() {
            return false;
        }
        self.phase = PlanInterviewPhase::Critique;
        true
    }

    /// Return to the plan from the advisory review, or from a review still in
    /// flight — a result that arrives after this is stashed rather than shown.
    pub fn close_critique(&mut self) -> bool {
        if !matches!(
            self.phase,
            PlanInterviewPhase::Critique | PlanInterviewPhase::CritiqueLoading
        ) {
            return false;
        }
        self.critique_started_at = None;
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// Stage the advisory review as input for the next synthesis pass. The
    /// caller starts that pass; until it lands the plan is unchanged.
    pub fn revise_from_critique(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Critique {
            return false;
        }
        let Some(critique) = self.critique.clone() else {
            return false;
        };
        self.revision_critique = Some(critique);
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// The advisory review staged for the next synthesis pass, if any. Read
    /// without consuming so a pass that turns out to be impossible leaves the
    /// feedback where the user can still reach it.
    pub fn staged_revision_critique(&self) -> Option<&str> {
        self.revision_critique.as_deref()
    }

    /// Take the staged revision feedback, leaving none behind so a later
    /// regenerate is a clean pass rather than a repeat of the same revision.
    /// Called only once the revision pass has actually started.
    pub fn take_revision_critique(&mut self) -> Option<String> {
        self.revision_critique.take()
    }

    /// Record that the plan on screen is a different plan: reset its rendered
    /// layout and drop an advisory review that no longer describes it.
    fn mark_plan_changed(&mut self) {
        self.plan_revision = self.plan_revision.wrapping_add(1);
        self.review_scroll_offset = 0;
        self.review_rendered_width = 0;
        self.review_rendered_lines.clear();
        self.clear_critique();
    }

    /// Drop an advisory review that no longer describes the current plan.
    fn clear_critique(&mut self) {
        self.critique = None;
        self.critique_started_at = None;
        self.critique_scroll_offset = 0;
        self.critique_rendered_width = 0;
        self.critique_rendered_lines.clear();
        self.critique_plan_revision = None;
    }

    /// Open the reviewed plan as raw markdown without changing the staged
    /// plan until the user explicitly saves the edit.
    pub fn begin_plan_edit(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Review {
            return false;
        }
        let Some(plan) = self.synthesized_plan.clone() else {
            return false;
        };
        self.editor = TextEditor::new(plan);
        self.edit_scroll_offset = 0;
        self.edit_sync_to_cursor = true;
        self.phase = PlanInterviewPhase::Editing;
        true
    }

    /// Save the raw markdown edit and return to the rendered preview.
    /// Empty plans are rejected so acceptance can never write a blank file.
    pub fn save_plan_edit(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Editing || self.editor.text().trim().is_empty() {
            return false;
        }
        let mut plan = self.editor.text().to_string();
        if !plan.ends_with('\n') {
            plan.push('\n');
        }
        let changed = self.synthesized_plan.as_deref() != Some(plan.as_str());
        self.synthesized_plan = Some(plan);
        if changed {
            self.mark_plan_changed();
        }
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// Discard the editor buffer and return to the last rendered plan.
    pub fn cancel_plan_edit(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::Editing {
            return false;
        }
        self.phase = PlanInterviewPhase::Review;
        true
    }

    /// Explicitly accept the optional token-spending AI follow-up stage.
    /// Returns false outside the consent screen so callers cannot opt in
    /// accidentally from an ordinary answer editor.
    pub fn opt_in_ai_followups(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::AiConsent {
            return false;
        }
        self.ai_followups_opted_in = true;
        self.skip_ai_rounds = false;
        self.phase = PlanInterviewPhase::Done;
        true
    }

    /// Apply a finished AI round's parsed follow-up questions.
    ///
    /// With no usable follow-ups, moves straight back to `Done` so the
    /// caller can decide whether to try another round or complete. With
    /// follow-ups, appends them to the question list and resumes the
    /// question flow at the first new one.
    pub fn apply_ai_round(&mut self, round: usize, new_questions: Vec<PlanQuestion>) {
        self.ai_round_started_at = None;
        self.ai_rounds_completed = round;
        if new_questions.is_empty() {
            self.phase = PlanInterviewPhase::Done;
            return;
        }
        let first_new_index = self.questions.len();
        self.answers.extend(new_questions.iter().map(|_| None));
        self.questions.extend(new_questions);
        self.phase = PlanInterviewPhase::StaticQuestions;
        self.question_index = first_new_index;
        self.load_current_answer();
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
                    self.phase = PlanInterviewPhase::AiConsent;
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
            PlanInterviewPhase::AiConsent => {
                // Enter is deliberately the no-token default. Opting in uses
                // the dedicated `a` action and `opt_in_ai_followups`.
                self.skip_ai_rounds = true;
                self.phase = PlanInterviewPhase::Done;
            }
            // The resume choice has its own dedicated keys; Enter must not fall
            // through to a question flow whose answers are not loaded yet. The
            // handoff prompt is past acceptance entirely.
            PlanInterviewPhase::ResumePrompt
            | PlanInterviewPhase::AiLoading
            | PlanInterviewPhase::SynthesisLoading
            | PlanInterviewPhase::Review
            | PlanInterviewPhase::Editing
            | PlanInterviewPhase::DirectedFeedback
            | PlanInterviewPhase::DirectedFeedbackLoading
            | PlanInterviewPhase::Investigation
            | PlanInterviewPhase::InvestigationLoading
            | PlanInterviewPhase::CritiqueLoading
            | PlanInterviewPhase::Critique
            | PlanInterviewPhase::KickoffHandoff
            | PlanInterviewPhase::Done => {}
        }
        Ok(())
    }

    /// Skip an optional question, or decline the optional AI follow-up stage.
    pub fn skip(&mut self) -> Result<(), PlanInterviewAdvanceError> {
        if self.phase == PlanInterviewPhase::AiConsent {
            self.skip_ai_rounds = true;
            self.phase = PlanInterviewPhase::Done;
            return Ok(());
        }
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
            PlanInterviewPhase::AiConsent if !self.questions.is_empty() => {
                self.phase = PlanInterviewPhase::StaticQuestions;
                self.question_index = self.questions.len() - 1;
                self.load_current_answer();
                true
            }
            PlanInterviewPhase::AiConsent => {
                self.phase = PlanInterviewPhase::Brief;
                self.editor = TextEditor::new(self.brief.clone());
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
            // Loading is a transient App-driven state; there is nothing to
            // navigate back to until it resolves. The review-gate phases have
            // their own dedicated navigation, the resume choice is the first
            // screen of the interview, and the handoff prompt comes after an
            // accept that already wrote the plan.
            PlanInterviewPhase::ResumePrompt
            | PlanInterviewPhase::AiLoading
            | PlanInterviewPhase::SynthesisLoading
            | PlanInterviewPhase::Review
            | PlanInterviewPhase::Editing
            | PlanInterviewPhase::DirectedFeedback
            | PlanInterviewPhase::DirectedFeedbackLoading
            | PlanInterviewPhase::Investigation
            | PlanInterviewPhase::InvestigationLoading
            | PlanInterviewPhase::CritiqueLoading
            | PlanInterviewPhase::Critique
            | PlanInterviewPhase::KickoffHandoff => false,
        }
    }

    /// End questioning with the answers collected so far, skip any remaining
    /// adaptive rounds, and explicitly request plan synthesis.
    pub fn finish_early(&mut self) -> Result<(), PlanInterviewAdvanceError> {
        match self.phase {
            PlanInterviewPhase::Brief => {
                if self.editor.text().trim().is_empty() {
                    return Err(PlanInterviewAdvanceError::BriefRequired);
                }
                self.brief = self.editor.text().to_string();
            }
            PlanInterviewPhase::StaticQuestions => self.save_current_draft(),
            PlanInterviewPhase::AiConsent => {}
            // Do not overlap paid calls or mutate a retryable completed
            // synthesis. The UI does not advertise this action while loading,
            // nor at the resume choice, which has no brief to synthesize yet,
            // nor at the handoff prompt, whose plan is already accepted.
            PlanInterviewPhase::ResumePrompt
            | PlanInterviewPhase::AiLoading
            | PlanInterviewPhase::SynthesisLoading
            | PlanInterviewPhase::Review
            | PlanInterviewPhase::Editing
            | PlanInterviewPhase::DirectedFeedback
            | PlanInterviewPhase::DirectedFeedbackLoading
            | PlanInterviewPhase::Investigation
            | PlanInterviewPhase::InvestigationLoading
            | PlanInterviewPhase::CritiqueLoading
            | PlanInterviewPhase::Critique
            | PlanInterviewPhase::KickoffHandoff
            | PlanInterviewPhase::Done => return Ok(()),
        }
        self.ai_round_started_at = None;
        self.skip_ai_rounds = true;
        self.synthesis_requested = true;
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
            self.phase = if self.ai_followups_opted_in {
                PlanInterviewPhase::Done
            } else {
                PlanInterviewPhase::AiConsent
            };
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
            "feat-1".into(),
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
            "feat-1".into(),
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
            "feat-1".into(),
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
    fn plan_interview_finish_early_skips_remaining_ai_rounds() {
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), Vec::new(), None);
        state.editor = TextEditor::new("A useful feature".into());
        state.finish_early().unwrap();

        assert_eq!(state.phase, PlanInterviewPhase::Done);
        assert!(state.skip_ai_rounds);
        assert!(state.synthesis_requested);
    }

    #[test]
    fn plan_interview_requires_explicit_opt_in_before_ai_rounds() {
        let questions = crate::plan_interview::builtin_questions()
            .into_iter()
            .take(1)
            .collect();
        let mut state = PlanInterviewState::new("feature".into(), "feat-1".into(), questions, None);
        state.editor = TextEditor::new("A useful feature".into());
        state.advance().unwrap();

        state.skip().unwrap();

        assert_eq!(state.phase, PlanInterviewPhase::AiConsent);
        assert!(!state.ai_followups_opted_in);
        assert!(!state.skip_ai_rounds);

        assert!(state.opt_in_ai_followups());

        assert_eq!(state.phase, PlanInterviewPhase::Done);
        assert!(state.ai_followups_opted_in);
    }

    #[test]
    fn plan_interview_ai_consent_can_be_declined_without_opt_in() {
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), Vec::new(), None);
        state.editor = TextEditor::new("A useful feature".into());
        state.advance().unwrap();

        assert_eq!(state.phase, PlanInterviewPhase::AiConsent);

        state.advance().unwrap();

        assert_eq!(state.phase, PlanInterviewPhase::Done);
        assert!(!state.ai_followups_opted_in);
        assert!(state.skip_ai_rounds);
        assert!(!state.synthesis_requested);
    }

    #[test]
    fn plan_interview_begin_ai_round_enters_loading_with_metadata() {
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), Vec::new(), None);

        state.begin_ai_round(1200);

        assert_eq!(state.phase, PlanInterviewPhase::AiLoading);
        assert!(state.ai_round_started_at.is_some());
        assert_eq!(state.ai_round_token_estimate, 1200);
        assert!(state.current_question().is_none());
    }

    #[test]
    fn plan_interview_synthesis_is_cached_and_opens_review() {
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), Vec::new(), None);

        state.begin_synthesis(900);

        assert_eq!(state.phase, PlanInterviewPhase::SynthesisLoading);
        assert!(state.synthesis_attempted);
        assert!(state.synthesis_started_at.is_some());
        assert_eq!(state.synthesis_token_estimate, 900);
        assert!(state.current_question().is_none());

        state.apply_synthesis("# Plan: feature\n".into());

        assert_eq!(state.phase, PlanInterviewPhase::Review);
        assert!(state.synthesis_started_at.is_none());
        assert_eq!(state.synthesized_plan.as_deref(), Some("# Plan: feature\n"));
    }

    #[test]
    fn plan_interview_plan_edits_are_staged_until_saved() {
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), Vec::new(), None);
        state.apply_synthesis("# Plan: original\n".into());

        assert!(state.begin_plan_edit());
        assert_eq!(state.phase, PlanInterviewPhase::Editing);
        state.editor = TextEditor::new("# Plan: changed".into());
        assert_eq!(
            state.synthesized_plan.as_deref(),
            Some("# Plan: original\n")
        );

        assert!(state.save_plan_edit());
        assert_eq!(state.phase, PlanInterviewPhase::Review);
        assert_eq!(state.synthesized_plan.as_deref(), Some("# Plan: changed\n"));
    }

    #[test]
    fn plan_interview_plan_edit_can_be_discarded_and_cannot_save_empty() {
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), Vec::new(), None);
        state.apply_synthesis("# Plan: original\n".into());

        assert!(state.begin_plan_edit());
        state.editor = TextEditor::new(String::new());
        assert!(!state.save_plan_edit());
        assert_eq!(state.phase, PlanInterviewPhase::Editing);
        assert!(state.cancel_plan_edit());
        assert_eq!(state.phase, PlanInterviewPhase::Review);
        assert_eq!(
            state.synthesized_plan.as_deref(),
            Some("# Plan: original\n")
        );
    }

    #[test]
    fn directed_feedback_preserves_the_plan_and_retryable_instruction() {
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), Vec::new(), None);
        state.apply_synthesis("# Plan: original\n".into());

        assert!(state.begin_directed_feedback());
        assert_eq!(state.phase, PlanInterviewPhase::DirectedFeedback);
        assert_eq!(
            state.synthesized_plan.as_deref(),
            Some("# Plan: original\n")
        );
        assert!(!state.begin_directed_feedback_loading(100));

        state.editor = TextEditor::new("Inspect the router and add exact paths.".into());
        assert!(state.begin_directed_feedback_loading(700));
        assert_eq!(state.phase, PlanInterviewPhase::DirectedFeedbackLoading);
        assert!(state.directed_feedback_started_at.is_some());
        assert_eq!(state.directed_feedback_token_estimate, 700);

        assert!(state.fail_directed_feedback());
        assert_eq!(state.phase, PlanInterviewPhase::DirectedFeedback);
        assert_eq!(
            state.editor.text(),
            "Inspect the router and add exact paths."
        );
        assert_eq!(
            state.synthesized_plan.as_deref(),
            Some("# Plan: original\n")
        );

        assert!(state.cancel_directed_feedback());
        assert_eq!(state.phase, PlanInterviewPhase::Review);
        assert_eq!(
            state.synthesized_plan.as_deref(),
            Some("# Plan: original\n")
        );
    }

    #[test]
    fn isolated_investigation_preserves_the_plan_and_retryable_focus() {
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), Vec::new(), None);
        state.apply_synthesis("# Plan: original\n".into());

        assert!(state.begin_investigation());
        assert_eq!(state.phase, PlanInterviewPhase::Investigation);
        assert!(!state.begin_investigation_loading(100));

        state.editor = TextEditor::new("Trace the session launch boundary.".into());
        assert!(state.begin_investigation_loading(1_200));
        assert_eq!(state.phase, PlanInterviewPhase::InvestigationLoading);
        assert!(state.investigation_started_at.is_some());
        assert_eq!(state.investigation_token_estimate, 1_200);

        assert!(state.fail_investigation());
        assert_eq!(state.phase, PlanInterviewPhase::Investigation);
        assert_eq!(state.editor.text(), "Trace the session launch boundary.");
        assert_eq!(
            state.synthesized_plan.as_deref(),
            Some("# Plan: original\n")
        );

        assert!(state.cancel_investigation());
        assert_eq!(state.phase, PlanInterviewPhase::Review);
    }

    #[test]
    fn plan_interview_finish_early_does_not_overlap_in_flight_ai_work() {
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), Vec::new(), None);
        state.begin_ai_round(500);

        state.finish_early().unwrap();

        assert_eq!(state.phase, PlanInterviewPhase::AiLoading);
        assert!(!state.synthesis_requested);
        assert!(!state.skip_ai_rounds);
    }

    #[test]
    fn plan_interview_apply_ai_round_with_no_questions_returns_to_done() {
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), Vec::new(), None);
        state.begin_ai_round(500);

        state.apply_ai_round(1, Vec::new());

        assert_eq!(state.phase, PlanInterviewPhase::Done);
        assert_eq!(state.ai_rounds_completed, 1);
        assert!(state.ai_round_started_at.is_none());
    }

    #[test]
    fn plan_interview_apply_ai_round_appends_questions_and_resumes_at_first_new_one() {
        let existing = crate::plan_interview::builtin_questions();
        let existing_count = existing.len();
        let mut state = PlanInterviewState::new("feature".into(), "feat-1".into(), existing, None);
        state.answers = vec![Some("answered".into()); existing_count];
        state.begin_ai_round(800);

        let follow_up = PlanQuestion {
            id: "retry-policy".into(),
            text: "How should retries behave?".into(),
            kind: PlanQuestionKind::FreeText,
            source: crate::plan_interview::QuestionSource::Ai { round: 1 },
            optional: true,
        };
        state.apply_ai_round(1, vec![follow_up.clone()]);

        assert_eq!(state.phase, PlanInterviewPhase::StaticQuestions);
        assert_eq!(state.ai_rounds_completed, 1);
        assert_eq!(state.question_index, existing_count);
        assert_eq!(state.questions.len(), existing_count + 1);
        assert_eq!(state.questions[existing_count], follow_up);
        assert_eq!(state.answers.len(), existing_count + 1);
        assert_eq!(state.answers[existing_count], None);
        assert_eq!(state.current_question(), Some(&follow_up));
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
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), vec![question], None);
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

    /// A record whose select answer names an option the question no longer
    /// offers — the config was edited between runs.
    fn record_with_retired_select_answer() -> crate::db::plan_interviews::PlanInterviewRecord {
        crate::db::plan_interviews::PlanInterviewRecord {
            feature_id: "feat-1".into(),
            feature_name: "feature".into(),
            brief: "Tighten the sidebar.".into(),
            questions: vec![PlanQuestion {
                id: "surface".into(),
                text: "Where should this appear?".into(),
                kind: PlanQuestionKind::Select(vec!["Dashboard".into(), "Overlay".into()]),
                source: crate::plan_interview::QuestionSource::Template,
                optional: true,
            }],
            answers: vec![Some("Overlay".into())],
            ..Default::default()
        }
    }

    /// The current bank asks the same question id with rewritten options.
    fn state_with_rewritten_select_options() -> PlanInterviewState {
        let question = PlanQuestion {
            id: "surface".into(),
            text: "Where should this appear?".into(),
            kind: PlanQuestionKind::Select(vec!["Dashboard".into(), "Session".into()]),
            source: crate::plan_interview::QuestionSource::Template,
            optional: true,
        };
        PlanInterviewState::new("feature".into(), "feat-1".into(), vec![question], None)
    }

    /// Matching by id alone would pre-fill an answer that is not one of the
    /// current options: unselectable in the UI, but still handed to the AI rounds
    /// and synthesis as this question's answer if the user never visits it.
    #[test]
    fn a_re_run_drops_a_select_answer_the_options_no_longer_offer() {
        let mut state = state_with_rewritten_select_options();

        assert!(state.apply_previous_transcript(&record_with_retired_select_answer()));

        assert_eq!(state.answers[0], None);
        assert!(!state.prior_answers.contains_key("surface"));
        // The brief still pre-fills, so the re-run is not blanked wholesale.
        assert_eq!(state.brief, "Tighten the sidebar.");

        // Nothing was pre-filled for this question, so it reports neither kept
        // nor changed, and there is nothing for Ctrl+R to put back.
        state.phase = PlanInterviewPhase::StaticQuestions;
        state.load_current_answer();
        assert_eq!(state.prior_answer_state(), None);
        assert!(!state.restore_prior_answer());

        // Finishing without ever visiting the question — the path that would
        // otherwise carry the stale answer straight into synthesis.
        state.phase = PlanInterviewPhase::Brief;
        state.editor = TextEditor::new(state.brief.clone());
        state.finish_early().unwrap();
        assert_eq!(state.phase, PlanInterviewPhase::Done);
        assert!(state.answers.iter().all(Option::is_none));
    }

    /// Same guard on the resume path: a draft is matched back by id too.
    #[test]
    fn resuming_a_draft_drops_a_select_answer_the_options_no_longer_offer() {
        let mut state = state_with_rewritten_select_options();
        state.offer_resume(record_with_retired_select_answer());

        assert!(state.resume_from_draft());

        assert_eq!(state.answers[0], None);
        // The question is unanswered again, so the resume lands on it.
        assert_eq!(state.phase, PlanInterviewPhase::StaticQuestions);
        assert_eq!(state.question_index, 0);
        assert_eq!(state.selected_option, 0);
    }

    /// A select answer the rewritten options still contain is pre-filled, and on
    /// a different index than it had before.
    #[test]
    fn a_re_run_keeps_a_select_answer_the_options_still_offer() {
        let mut state = state_with_rewritten_select_options();
        let mut record = record_with_retired_select_answer();
        record.answers = vec![Some("Session".into())];

        assert!(state.apply_previous_transcript(&record));

        assert_eq!(state.answers[0].as_deref(), Some("Session"));
        state.phase = PlanInterviewPhase::StaticQuestions;
        state.load_current_answer();
        assert_eq!(state.selected_option, 1);
        assert_eq!(state.prior_answer_state(), Some(PriorAnswerState::Kept));
    }

    /// A draft saved for a feature-creation interview is keyed by project and
    /// branch, because the feature it plans has no id until the accept.
    #[test]
    fn feature_creation_interview_is_keyed_by_project_and_branch() {
        let state = PlanInterviewState::for_feature_creation(
            prepared_launch("my-project", "planned-feature"),
            Vec::new(),
        );

        assert_eq!(state.interview_key, "pending:my-project/planned-feature");
        assert_eq!(state.feature_name, "planned-feature");
    }

    fn prepared_launch(project_name: &str, branch: &str) -> PreparedFeatureLaunch {
        PreparedFeatureLaunch {
            project_name: project_name.into(),
            branch: branch.into(),
            workdir: PathBuf::from("/tmp/does-not-matter"),
            is_worktree: false,
            mode: VibeMode::default(),
            review: false,
            plan_mode: true,
            agent: AgentKind::Claude,
            create_terminal: false,
            session_name: "Claude 1".into(),
            enable_chrome: false,
            remote_control: false,
            steering_enabled: false,
            hook_succeeded: None,
            startup_prompt: None,
        }
    }

    fn saved_draft(
        questions: Vec<PlanQuestion>,
        answers: Vec<Option<String>>,
    ) -> PlanInterviewRecord {
        PlanInterviewRecord {
            feature_id: "feat-1".into(),
            stage: PlanInterviewStage::Draft,
            feature_name: "feature".into(),
            brief: "Ship the interview.".into(),
            questions,
            answers,
            plan: None,
            ai_rounds_completed: 0,
            created_at: String::new(),
            updated_at: "2026-07-30 12:00:00".into(),
        }
    }

    fn template_question(id: &str) -> PlanQuestion {
        PlanQuestion {
            id: id.into(),
            text: format!("Question {id}?"),
            kind: PlanQuestionKind::FreeText,
            source: QuestionSource::Template,
            optional: true,
        }
    }

    #[test]
    fn resuming_a_draft_restores_answers_and_lands_on_the_first_unanswered() {
        let questions = vec![
            template_question("scope"),
            template_question("risks"),
            template_question("done"),
        ];
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), questions.clone(), None);
        state.offer_resume(saved_draft(
            questions,
            vec![
                Some("Just the TUI.".into()),
                None,
                Some("Tests pass.".into()),
            ],
        ));
        assert_eq!(state.phase, PlanInterviewPhase::ResumePrompt);

        assert!(state.resume_from_draft());

        assert_eq!(state.brief, "Ship the interview.");
        assert_eq!(state.answers[0].as_deref(), Some("Just the TUI."));
        assert_eq!(state.answers[2].as_deref(), Some("Tests pass."));
        assert_eq!(state.phase, PlanInterviewPhase::StaticQuestions);
        // The gap, not the end of the answered run: resuming should not make the
        // user walk back to the question they actually stopped at.
        assert_eq!(state.question_index, 1);
        assert!(state.editor.text().is_empty());
        assert!(state.resume_draft.is_none());
    }

    /// The question bank is config-driven and can change between runs, so
    /// answers are matched by id rather than carried over positionally.
    #[test]
    fn resuming_matches_answers_by_id_when_the_bank_changed() {
        let stored = vec![template_question("removed"), template_question("scope")];
        let mut state = PlanInterviewState::new(
            "feature".into(),
            "feat-1".into(),
            vec![template_question("scope"), template_question("added")],
            None,
        );
        state.offer_resume(saved_draft(
            stored,
            vec![Some("Gone.".into()), Some("Just the TUI.".into())],
        ));

        assert!(state.resume_from_draft());

        // "scope" keeps its answer despite having moved from index 1 to index 0;
        // the dropped question's answer is not smeared onto "added".
        assert_eq!(state.answers[0].as_deref(), Some("Just the TUI."));
        assert_eq!(state.answers[1], None);
        assert_eq!(state.question_index, 1);
    }

    /// AI-generated questions cost tokens and cannot be in the current bank, so
    /// a resume carries them (and the rounds they came from) rather than
    /// re-earning them.
    #[test]
    fn resuming_carries_ai_questions_and_spent_rounds() {
        let ai_question = PlanQuestion {
            id: "concurrency".into(),
            text: "How do concurrent interviews interact?".into(),
            kind: PlanQuestionKind::FreeText,
            source: QuestionSource::Ai { round: 1 },
            optional: true,
        };
        let mut stored = saved_draft(
            vec![template_question("scope"), ai_question.clone()],
            vec![Some("Just the TUI.".into()), None],
        );
        stored.ai_rounds_completed = 1;

        let mut state = PlanInterviewState::new(
            "feature".into(),
            "feat-1".into(),
            vec![template_question("scope")],
            None,
        );
        state.offer_resume(stored);

        assert!(state.resume_from_draft());

        assert_eq!(state.questions.len(), 2);
        assert_eq!(state.questions[1], ai_question);
        assert_eq!(state.ai_rounds_completed, 1);
        // A spent round implies the consent it required, so the interview does
        // not ask for it a second time.
        assert!(state.ai_followups_opted_in);
        assert_eq!(state.question_index, 1);
    }

    /// A draft abandoned at the review gate already paid for its plan.
    #[test]
    fn resuming_a_draft_with_a_plan_reopens_the_review_gate() {
        let questions = vec![template_question("scope")];
        let mut stored = saved_draft(questions.clone(), vec![Some("Just the TUI.".into())]);
        stored.plan = Some("# Plan: feature\n".into());

        let mut state = PlanInterviewState::new("feature".into(), "feat-1".into(), questions, None);
        state.offer_resume(stored);

        assert!(state.resume_from_draft());

        assert_eq!(state.phase, PlanInterviewPhase::Review);
        assert_eq!(state.synthesized_plan.as_deref(), Some("# Plan: feature\n"));
        // Nothing should re-synthesize a plan the user already has on screen.
        assert!(state.synthesis_attempted);
    }

    #[test]
    fn a_fully_answered_draft_resumes_at_the_choice_after_the_questions() {
        let questions = vec![template_question("scope")];
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), questions.clone(), None);
        state.offer_resume(saved_draft(questions, vec![Some("Just the TUI.".into())]));

        assert!(state.resume_from_draft());

        assert_eq!(state.phase, PlanInterviewPhase::AiConsent);
    }

    #[test]
    fn discarding_a_draft_starts_from_a_blank_brief() {
        let questions = vec![template_question("scope")];
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), questions.clone(), None);
        state.offer_resume(saved_draft(questions, vec![Some("Just the TUI.".into())]));

        assert!(state.discard_draft());

        assert_eq!(state.phase, PlanInterviewPhase::Brief);
        assert!(state.brief.is_empty());
        assert_eq!(state.answers, vec![None]);
        assert!(state.editor.text().is_empty());
        assert!(state.resume_draft.is_none());
        // Only reachable from the resume choice.
        assert!(!state.discard_draft());
    }

    #[test]
    fn draft_snapshot_carries_the_interview_key_and_collected_answers() {
        let questions = vec![template_question("scope"), template_question("risks")];
        let mut state = PlanInterviewState::new(
            "feature".into(),
            "pending:my-project/planned-feature".into(),
            questions,
            None,
        );
        state.editor = TextEditor::new("Ship it.".into());
        state.advance().unwrap();
        state.editor = TextEditor::new("Just the TUI.".into());
        state.advance().unwrap();

        let record = state.to_draft_record();

        assert_eq!(record.feature_id, "pending:my-project/planned-feature");
        assert_eq!(record.stage, PlanInterviewStage::Draft);
        assert_eq!(record.brief, "Ship it.");
        assert_eq!(record.answers[0].as_deref(), Some("Just the TUI."));
        assert_eq!(record.answers[1], None);
        assert!(record.plan.is_none());
    }
}
