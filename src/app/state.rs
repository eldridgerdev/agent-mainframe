pub use crate::app::learning::state::*;
pub use crate::app::pr_review::state::*;
pub use crate::app::review::state::*;
use ratatui_explorer::FileExplorer;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Child;
use std::time::{Duration, Instant};

use super::PromptAnalysis;
pub use super::compose_state::*;
pub use super::plan_interview_state::*;
pub use super::prompt_library_state::*;
pub use super::prompt_override_state::*;
pub use super::todo_state::*;
use crate::db::remote_devices::RemoteDevice;
use crate::editor::TextEditor;
use crate::extension::{CustomSessionConfig, FeaturePreset};
use crate::project::{AgentKind, SessionKind, VibeMode};
use crate::worktree::WorktreeInfo;

pub const STARTUP_MASK_MAX_DURATION: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, PartialEq)]
pub enum ForkFeatureStep {
    Branch,
    Agent,
}

pub struct ForkFeatureState {
    pub project_id: String,
    pub source_feature_id: String,
    pub source_workdir: PathBuf,
    pub source_agent: AgentKind,
    pub source_tmux_session: String,
    pub source_is_worktree: bool,
    pub source_status: crate::project::ProjectStatus,
    pub source_session_id: Option<String>,
    pub source_session_kind: Option<crate::project::SessionKind>,
    pub source_session_window: Option<String>,
    pub source_session_stopped: Option<bool>,
    pub source_claude_session_id: Option<String>,
    pub snapshot: crate::fork_snapshot::Snapshot,
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
    /// The feature's saved mode when it differs from the one this view's
    /// agent was launched with. `vibe_mode` stays on the launched mode -- the
    /// flags the live agent is really running under -- until a restart.
    pub pending_vibe_mode: Option<VibeMode>,
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
            pending_vibe_mode: None,
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

    /// Record a mode saved for the feature while this view is open. A live
    /// agent keeps its launch flags, so its badge stays put and the saved mode
    /// is shown as pending; any other session has no flags to go stale and
    /// simply takes the saved mode.
    pub fn note_saved_vibe_mode(&mut self, saved: &VibeMode) {
        if !self.session_kind.is_agent_harness() {
            self.vibe_mode = saved.clone();
            return;
        }
        self.pending_vibe_mode = (*saved != self.vibe_mode).then(|| saved.clone());
    }

    /// The header's "saved mode is waiting on a restart" marker, shared by the
    /// renderer and the mouse hit-test so their widths cannot disagree.
    pub fn pending_vibe_mode_badge(&self) -> Option<String> {
        self.pending_vibe_mode.as_ref().map(|mode| {
            format!(
                "\u{2192} {} on restart ",
                mode.display_name().to_lowercase()
            )
        })
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

        self.session_kind
            .is_agent_harness()
            .then(|| self.session_kind.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
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

impl PendingInput {
    /// Whether this notification describes the *session* as having stopped and
    /// being blocked on the user, rather than a discrete piece of work to act
    /// on.
    ///
    /// The distinction is what makes deduplication safe. A session can only be
    /// stopped once, so every report of it is the same standing fact restated:
    /// Claude's Stop hook fires at every turn boundary, Codex's notify hook on
    /// every prompt, and `sync.rs` infers the same stop again whenever a
    /// thinking marker drops. A diff review or a change reason is the
    /// opposite — each one is its own request about its own edit, and they
    /// must keep queueing one row apiece.
    pub fn is_session_wait(&self) -> bool {
        matches!(self.notification_type.as_str(), "input-request" | "stop")
    }

    /// Whether `other` is a re-report of the same session's stop, and so
    /// replaces this entry instead of queueing beside it.
    ///
    /// Identity is the feature, which is the granularity everything else in
    /// AMF already uses for this: a feature owns one tmux session, the
    /// attention layer is keyed by it, and `sync.rs` clears pending inputs by
    /// it. `session_id` is only the fallback for a notification that could not
    /// be resolved to a feature — it holds the AMF tmux session when the hook
    /// environment supplied one and the harness's own session id otherwise, so
    /// two reports of one stop can disagree about it.
    pub fn is_same_session_wait(&self, other: &PendingInput) -> bool {
        if !self.is_session_wait() || !other.is_session_wait() {
            return false;
        }
        match (self.feature_name.as_deref(), other.feature_name.as_deref()) {
            (Some(mine), Some(theirs)) => self.project_name == other.project_name && mine == theirs,
            _ => !self.session_id.is_empty() && self.session_id == other.session_id,
        }
    }
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

/// Which field of [`ContextSettingsState`] currently has input focus.
/// Declared in edit order so `next()`/`prev()` can wrap with simple
/// arithmetic instead of a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextSettingsField {
    WindowLimit,
    WarningPercent,
    CriticalPercent,
}

impl ContextSettingsField {
    const ALL: [Self; 3] = [
        Self::WindowLimit,
        Self::WarningPercent,
        Self::CriticalPercent,
    ];

    pub fn next(self) -> Self {
        let index = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let index = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// Global context-window/severity settings dialog (`w` on the dashboard).
/// Edits [`super::AppConfig::context_window_override`],
/// `context_warning_percent`, and `context_critical_percent` directly —
/// unlike `ConfigWizard` this has no project/global scope choice, since the
/// values it edits are process-wide by design.
pub struct ContextSettingsState {
    pub field: ContextSettingsField,
    /// Empty means "no override" (falls back to each harness's own default).
    pub window_limit_input: String,
    pub warning_input: String,
    pub critical_input: String,
    pub error: Option<String>,
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
    /// The `FeatureSession::id` this picker was opened from (or, when opened
    /// from the feature row rather than a specific session, the feature's
    /// first opencode-kind session) — the *only* session a restore may
    /// overwrite. Every other opencode session in the feature keeps its own
    /// saved id. `None` when the feature had no sessions at all yet (they
    /// are created fresh on restore, so the lone one created is the target).
    pub target_session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClaudeSessionPickerState {
    pub sessions: Vec<super::claude_sessions::ClaudeSessionInfo>,
    pub selected: usize,
    pub workdir: PathBuf,
    /// The `FeatureSession::id` this picker was opened from (or, when opened
    /// from the feature row rather than a specific session, the feature's
    /// first claude-kind session) — the *only* session a restore may
    /// overwrite. Every other claude session in the feature keeps its own
    /// saved id. `None` when the feature had no sessions at all yet (they
    /// are created fresh on restore, so the lone one created is the target).
    pub target_session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CodexSessionPickerState {
    pub sessions: Vec<super::codex_sessions::CodexSessionInfo>,
    pub selected: usize,
    pub workdir: PathBuf,
    /// The `FeatureSession::id` this picker was opened from (or, when opened
    /// from the feature row rather than a specific session, the feature's
    /// first codex-kind session) — the *only* session a restore may
    /// overwrite. Every other codex session in the feature keeps its own
    /// saved id. `None` when the feature had no sessions at all yet (they
    /// are created fresh on restore, so the lone one created is the target).
    pub target_session_id: Option<String>,
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
    /// A pull request at pinned revisions: `merge_base..head`, both sides read
    /// from git objects. Never the working tree, the checked-out branch, or
    /// `HEAD`, so the feature the viewer was opened from cannot leak in.
    PullRequest(Box<PrDiffTarget>),
}

/// The pull request a [`DiffScope::PullRequest`] viewer is reviewing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrDiffTarget {
    /// The base repository as `host/owner/name` (`GithubRepository::canonical`),
    /// or the raw fetch URL when that can't be parsed. Host-qualified so two
    /// GitHub hosts' `owner/name` never collide as a draft key.
    pub repo: String,
    /// The PR at the revisions that were fetched and verified: `base_oid` and
    /// `head_oid` are exactly what the private review refs point at.
    pub pr: crate::github::ReviewablePr,
    pub merge_base_oid: String,
}

pub struct DiffPickerState {
    pub from_view: ViewState,
    pub workdir: PathBuf,
    pub commits: Vec<crate::diff::DiffCommit>,
    /// Zero is "all current changes"; commit rows start at one.
    pub selected: usize,
    /// Why commits are unavailable, in one line (the raw git error goes to
    /// the debug log). `None` when commits listed normally.
    pub error: Option<String>,
    /// No base branch resolved, so "All current changes" is the working tree
    /// against `HEAD` and there are no branch commits to list.
    pub no_base_branch: bool,
    /// Listing failed because no base branch could be determined, the one
    /// failure that choosing a base ref with `b` can fix.
    pub base_unresolved: bool,
}

// Not `Clone`: holds a `std::process::Child` for the in-flight walkthrough
// generation (matching `DiffReviewState`). Nothing clones this state wholesale.

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

pub enum AppMode {
    Normal,
    Todos(TodoViewState),
    /// Read-only Learning Mode overlay: browse the project and ask an agent
    /// about what you're looking at (`crate::app::learning`).
    #[allow(dead_code)] // Entered by the plan's Epic 4 dashboard key.
    Learning(Box<LearningViewState>),
    TodoQuickCapture(TodoQuickCaptureState),
    /// Collect the user's instruction before starting a fresh-context agent
    /// session (see `FreshContextPromptState`).
    FreshContextPrompt(FreshContextPromptState),
    /// Re-home or delete a project's TODO list after its host feature is deleted.
    TodosHostReassign(TodosHostReassignState),
    /// "Implement next" landed on a TODO that already has work started for it.
    TodoImplementChoice(Box<TodoImplementChoiceState>),
    /// Pick the feature to put an agent on a project- or global-scoped TODO
    /// in. Those lists are not tied to one checkout, so there is no feature to
    /// infer — the user names one, and it supplies the agent and mode.
    TodoSpawnTarget(Box<TodoSpawnTargetState>),
    /// A feature is about to be deleted and its worktree list still has
    /// unfinished TODOs: move them to the project list, move them to the
    /// global list, delete them, or cancel the deletion outright.
    TodoDeleteDisposition(TodoDeleteDispositionState),
    /// Confirm completing the TODO explicitly referenced by the embedded
    /// agent session currently being viewed.
    ConfirmTodoReferenceCompletion(TodoReferenceCompletionState),
    CreatingProject(CreateProjectState),
    CreatingFeature(CreateFeatureState),
    #[allow(dead_code)] // Entered by the next Epic 1 feature-launch integration.
    PlanInterview(PlanInterviewState),
    ModelAnalysis(Box<super::model_analysis::State>),
    /// Temporary interactive mock; values are not persisted.
    ModelDefaultsPrototype {
        selected: usize,
        harness: usize,
        benchmark_notice: bool,
        models: [[usize; 4]; 4],
        reasoning: [[usize; 4]; 4],
    },
    DeletingProject(String),
    DeletingFeature(String, String),
    /// Dashboard `Esc`: ask before quitting AMF.
    ConfirmingQuit,
    /// Dashboard `d` on a session: ask before removing it for good.
    ConfirmingRemoveSession(RemoveSessionConfirm),
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
    /// A file browser opened from the plan-interview brief step to attach a
    /// reference document. Stashes the live [`PlanInterviewState`] so both
    /// confirm and cancel return to the interview exactly as it was.
    PlanInterviewAttachDoc(Box<AttachDocState>),
    CommandPicker(super::CommandPickerState),
    Searching(SearchState),
    NamingNewSession(NewSessionNameState),
    OpencodeSessionPicker(OpencodeSessionPickerState),
    ConfirmingOpencodeSession {
        session_id: String,
        workdir: PathBuf,
        target_session_id: Option<String>,
    },
    ClaudeSessionPicker(ClaudeSessionPickerState),
    ConfirmingClaudeSession {
        session_id: String,
        workdir: PathBuf,
        target_session_id: Option<String>,
    },
    CodexSessionPicker(CodexSessionPickerState),
    ConfirmingCodexSession {
        session_id: String,
        workdir: PathBuf,
        target_session_id: Option<String>,
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
    /// The PR picker's Review tab (see [`PrReviewListState`]).
    PrReviewList(Box<PrReviewListState>),
    /// Browse open issues for the selected project's canonical GitHub repo.
    #[allow(dead_code)] // Constructed by the issue-fixer dashboard entrypoint.
    IssueBrowser(crate::app::issue_fixer::IssueBrowserState),
    /// Configure the feature and editable prompt for the selected issue.
    IssueSetup(crate::app::issue_fixer::IssueSetupState),
    /// An existing feature already records the same canonical issue. The
    /// carried setup preserves all edits if the user cancels the override.
    IssueDuplicateWarning(crate::app::issue_fixer::IssueDuplicateWarningState),
    /// Fetching a PR's comments off the UI thread; shows a loading frame.
    PrReviewLoading(PrReviewLoadState),
    /// Triaging a PR's comments in the full-screen PR Triage pane.
    PrReview(PrReviewState),
    /// Running a strictly read-only investigation of one review comment off the
    /// UI thread; shows a modal loading frame over the stashed PR Triage pane.
    PrInvestigationLoading(PrInvestigationLoadState),
    /// Running the review-memory lookback bootstrap's fetch + distill pass off
    /// the UI thread; shows a loading frame with the current stage.
    ReviewMemoryBootstrapRunning(BootstrapRunState),
    /// Confirming the review-memory compact pass (`c` in the PR picker, PR
    /// Triage, or the dashboard leader key) before spending an agent pass on
    /// it. Reachable from several screens, so it stashes `prior_mode` — see
    /// [`ReviewMemoryCompactConfirmState`].
    ReviewMemoryCompactConfirm(Box<ReviewMemoryCompactConfirmState>),
    /// Running the review-memory compact pass off the UI thread ("prevent
    /// review-memory rot"); shows a loading frame with the current stage.
    /// The mode to restore lives in [`App::review_memory_compact_pending`]
    /// rather than here — see [`CompactRunView`].
    ReviewMemoryCompactRunning(CompactRunView),
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
    /// The headless-prompt override manager (`P` on the dashboard / a leader
    /// command). Lists every registry prompt with its effective scope/source
    /// and an inline template editor with scope + harness pickers on save.
    PromptOverrides(Box<PromptOverridesState>),
    /// The blocking pre-call notice shown before a user-initiated headless AI
    /// run: announces the prompt ID + harness, with view / edit / continue /
    /// cancel. Automated runs (Learning Mode answers, session summaries) show
    /// a toast instead and never reach this.
    PromptPrecall(Box<crate::app::precall::PendingPrecall>),
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
    /// Landing a companion review feature's commits back on the source feature's
    /// branch (push or cherry-pick). Opened with `t` on the dashboard for a
    /// feature carrying a [`crate::project::ReviewSource`] link. Reuses PR
    /// Triage's [`TriageIntegrateState`] (`triage_branch` = the companion
    /// branch, `pr_branch` = the source feature's branch).
    ReviewIntegrate(TriageIntegrateState),
    /// Soft warning shown before starting an agent when the machine is already
    /// at the concurrency cap and/or low on memory. Confirming starts anyway.
    ConfirmResourceStart(Box<ResourceConfirmState>),
    /// Features that are idle and unattended, with per-row reclaim actions.
    Dormant(DormantViewState),
    /// Global context-window/severity settings (`w` on the dashboard).
    ContextSettings(ContextSettingsState),
    /// Pairing a phone to the Remote Control companion app: a one-time code
    /// (shown as a QR + digits) that `App::process_pairing_exchange`
    /// validates against `POST /pair/exchange` requests relayed from the
    /// server thread (`docs/backlog/remote-control-companion-app-plan.md`,
    /// Epic 4). This struct *is* the pending-pairing state — there is no
    /// separate copy on `App`, so closing the dialog (which drops it)
    /// invalidates the code.
    RemotePairing(RemotePairingState),
}

impl AppMode {
    /// The mode this one stashed to restore verbatim on exit, if any — a
    /// sub-mode opened over an overlay (e.g. `TodoImplementChoice` over
    /// `Todos`) keeps that overlay alive inside it.
    pub fn stashed_mode(&self) -> Option<&AppMode> {
        match self {
            AppMode::TodoImplementChoice(state) => Some(&state.origin),
            AppMode::TodoSpawnTarget(state) => Some(&state.origin),
            AppMode::PromptEditor(state) => Some(&state.return_to),
            AppMode::SkillPicker(state) => Some(&state.return_to),
            AppMode::PromptPrecall(pending) => Some(&pending.prior_mode),
            AppMode::SyntaxLanguagePicker(state) => state.return_to.as_deref(),
            _ => None,
        }
    }

    /// Whether this mode is, or has stashed underneath it, the TODOs
    /// overlay — whose in-memory panes will be restored as they are.
    pub fn holds_todos_overlay(&self) -> bool {
        let mut mode = Some(self);
        while let Some(current) = mode {
            if matches!(current, AppMode::Todos(_)) {
                return true;
            }
            mode = current.stashed_mode();
        }
        false
    }
}

/// UI status for the active `RemotePairing` dialog.
pub enum PairingDialogStatus {
    /// No exchange attempt has resolved yet.
    Waiting,
    /// A device successfully paired; `Esc`/`Enter` closes the dialog.
    Paired { device_name: String },
    /// The most recent attempt failed, or the code expired/locked out —
    /// human-readable, shown directly, and cleared by `r` regenerating.
    Failed(String),
}

pub struct RemotePairingState {
    /// The current one-time code. Also embedded in `qr_payload`.
    pub code: String,
    /// The URL the QR encodes, minus the code: `AppConfig::remote_public_url`
    /// when set, else `http://<addr>`. Shown under the QR so the address can
    /// be typed when a camera isn't handy.
    pub url: String,
    /// Pre-rendered half-block QR glyphs (`crate::qr::render_qr_lines`),
    /// one `String` per row. Empty when encoding failed (shouldn't happen
    /// for this payload shape) — the dialog falls back to the digits alone.
    pub qr_lines: Vec<String>,
    pub expires_at: Instant,
    /// Failed exchange attempts against `code` so far.
    pub attempts: u32,
    /// Set once `attempts` hits the cap — the code is dead even if it
    /// hasn't expired yet; only `r` (a fresh code) recovers.
    pub locked: bool,
    pub status: PairingDialogStatus,
    /// Which sub-screen of the dialog is showing — the pairing code itself,
    /// or the paired-devices list (`v`) with per-device revoke. Lives here
    /// rather than as a separate `AppMode` because it's a view toggle on
    /// the same dialog, not a new destination — `Esc` from the devices list
    /// returns to `Pairing`, and only `Esc` from `Pairing` closes the dialog.
    pub view: PairingDialogView,
    /// Set when `url` is an address no phone can open — the server's own
    /// loopback or wildcard bind, because no `remote_public_url` is
    /// configured. The dialog says so instead of showing a QR that can
    /// only fail.
    pub url_unreachable: bool,
    /// Where `url` came from — shown under it, and what decides whether a
    /// later Tailscale probe may replace it (`App::refresh_pairing_url`).
    pub url_source: PairingUrlSource,
    /// Whether `url` answered as AMF when this computer asked it for
    /// `/health` (`App::check_pairing_reach`). Advisory: it is checked from
    /// here, not from the phone, so a pass doesn't promise the phone gets
    /// through — but a failure means the QR leads nowhere.
    pub reach: PairingReach,
    /// The session view the dialog was opened over (`Ctrl+Space Q` from a
    /// session), restored on close — the same shape as the bookmark
    /// picker's `from_view`.
    pub from_view: Option<ViewState>,
}

/// The `RemotePairing` dialog's current sub-screen.
pub enum PairingDialogView {
    Pairing,
    Devices(RemoteDevicesListState),
    /// Step-by-step Tailscale setup (`s`), scrolled by `scroll` rows.
    /// `max_scroll` is written by the draw, the only place that knows how
    /// many rows the wrapped steps take at the current size, so scrolling
    /// stops at the last step instead of running on past it.
    Setup {
        scroll: u16,
        max_scroll: std::cell::Cell<u16>,
    },
}

/// Where the pairing QR's address comes from, most explicit first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingUrlSource {
    /// `remote_public_url` in config.json — never second-guessed.
    Configured,
    /// The HTTPS address `tailscale serve` answers on for AMF's port.
    Tailscale,
    /// The server's own bind address: fine on the LAN or over `adb
    /// reverse`, never for install or push.
    Direct,
}

/// What asking the pairing address for AMF's `/health` from this computer
/// found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PairingReach {
    /// Not asked: `url_unreachable` already says no phone can open it.
    NotChecked,
    Checking,
    Reachable,
    /// Something answered, but not with AMF's `ok` — another service, or a
    /// tunnel whose own error page stands in for an AMF it can't reach.
    NotAmf(String),
    /// Nothing answered. Carries a short reason ("timed out").
    Unreachable(String),
}

impl PairingReach {
    pub fn failed(&self) -> bool {
        matches!(self, PairingReach::NotAmf(_) | PairingReach::Unreachable(_))
    }
}

/// The paired-devices list shown by pressing `v` in the pairing dialog.
pub struct RemoteDevicesListState {
    /// Loaded once on `v` — every paired device, most recently paired
    /// first (`AmfDb::list_remote_devices`'s own ordering). Revoking
    /// updates this copy directly rather than reloading, so the cursor
    /// position is stable across a revoke.
    pub devices: Vec<RemoteDevice>,
    pub selected: usize,
    /// Set by a first `d` press on a revocable row; a second `d` while set
    /// performs the revoke. Any other key clears it — mirrors the prompt
    /// overrides manager's `confirm_clear` (`app/prompt_overrides.rs`).
    pub confirm_revoke: bool,
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

pub use super::config_wizard::{
    ConfigCategory, ConfigScope, ConfigWizardFieldEditor, ConfigWizardIconPicker,
    ConfigWizardState, ConfigWizardStep,
};

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
    pub diagrams: crate::mermaid::DiagramDocument,
    pub return_to_picker: Option<MarkdownFilePickerState>,
    pub from_view: Option<ViewState>,
    pub current_plan: bool,
}

pub enum MarkdownLoadingOperation {
    DiscoverFromView {
        view: ViewState,
    },
    DiscoverFromViewer {
        viewer: MarkdownViewerState,
    },
    DiscoverPlan {
        view: ViewState,
        feature_id: String,
    },
    ReadPath {
        path: PathBuf,
        workdir: PathBuf,
        repo_root: Option<PathBuf>,
        view: ViewState,
        return_to_picker: Option<MarkdownFilePickerState>,
        current_plan: bool,
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
    pub purpose: MarkdownFilePickerPurpose,
    pub from_view: Option<ViewState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarkdownFilePickerPurpose {
    Browse,
    SelectPlan { feature_id: String },
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
    /// Recreating one session's tmux window (`c` on a session whose window
    /// was stopped individually with `x`) while the rest of its feature
    /// stays up.
    RestartSessionWindow { pi: usize, fi: usize, si: usize },
    /// Opening a stopped feature or session from the dashboard (`Enter`).
    /// Replayed against the current selection, which the dialog leaves alone.
    EnterView { auto_compose: bool },
    /// Jumping to a stopped feature from inside a session view (leader n/p).
    SwitchViewToFeature { pi: usize, fi: usize },
    /// Creating and starting a newly accepted plan-mode feature. Unlike the
    /// ordinary creation autostart, this operation can be parked because the
    /// resource dialog retains the completed interview below.
    PlannedFeature(Box<PendingPlanLaunch>),
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
    /// Completed plan review restored verbatim when a planned feature start is
    /// cancelled. Other resource-gate callers originate from the dashboard or
    /// a session view and leave this empty.
    pub plan_interview: Option<PlanInterviewState>,
}

// Hook continuations exist one at a time and are moved directly into the
// worker. Keeping the launch fields flat makes the recovery path explicit;
// boxing selected fields solely to equalize enum variants would obscure it.
#[allow(clippy::large_enum_variant)]
pub enum HookNext {
    WorktreeCreated {
        expected_feature_id: Option<String>,
        project_name: String,
        feature_name: Option<String>,
        branch: String,
        mode: VibeMode,
        review: bool,
        plan_mode: bool,
        quick_plan: bool,
        agent: AgentKind,
        create_terminal: bool,
        session_name: String,
        enable_chrome: bool,
        remote_control: bool,
        steering_enabled: bool,
        startup_prompt: Option<String>,
        /// Carried across the hook detour, which rebuilds the launch from
        /// scratch and would otherwise drop the TODO link on any project with
        /// an `on_worktree_created` hook.
        todo_origin: Option<TodoPlanOrigin>,
        issue_source: Option<crate::project::IssueSource>,
    },
    StartFeature {
        pi: usize,
        fi: usize,
        /// The session row the start was requested from, launched on purpose.
        session_id: Option<String>,
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
    pub expected_feature_id: Option<String>,
    pub script: String,
    pub workdir: PathBuf,
    pub project_name: String,
    pub feature_name: Option<String>,
    pub todo_origin: Option<TodoPlanOrigin>,
    /// Source issue supplied by the issue-fixer workflow.
    pub issue_source: Option<crate::project::IssueSource>,
    pub branch: String,
    pub mode: VibeMode,
    pub review: bool,
    pub plan_mode: bool,
    pub quick_plan: bool,
    pub agent: AgentKind,
    pub create_terminal: bool,
    pub session_name: String,
    pub enable_chrome: bool,
    pub remote_control: bool,
    pub steering_enabled: bool,
    pub startup_prompt: Option<String>,
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

#[derive(Debug, Clone)]
pub struct RemoveSessionConfirm {
    pub label: String,
    /// A live agent window exists, so removing kills a running agent.
    pub agent_running: bool,
    /// The session's tmux window is live (agent or not: terminals, dev
    /// servers and watchers too), so removing kills its process.
    pub window_live: bool,
    /// Removing the feature's only session kills the whole tmux session.
    pub closes_tmux_session: bool,
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
    pub expected_feature_id: Option<String>,
    #[allow(dead_code)] // retained for the background-hook key, not read directly
    pub script: String,
    pub workdir: PathBuf,
    pub project_name: String,
    pub feature_name: Option<String>,
    pub todo_origin: Option<TodoPlanOrigin>,
    pub issue_source: Option<crate::project::IssueSource>,
    pub branch: String,
    pub mode: VibeMode,
    pub review: bool,
    pub plan_mode: bool,
    pub quick_plan: bool,
    pub agent: AgentKind,
    pub create_terminal: bool,
    pub session_name: String,
    pub enable_chrome: bool,
    pub remote_control: bool,
    pub steering_enabled: bool,
    pub startup_prompt: Option<String>,
    pub child: Option<Child>,
    pub output: String,
    pub success: Option<bool>,
    pub output_rx: Option<std::sync::mpsc::Receiver<String>>,
}

impl BackgroundHook {
    pub fn from_running_state(state: RunningHookState) -> Self {
        Self {
            expected_feature_id: state.expected_feature_id,
            script: state.script,
            workdir: state.workdir,
            project_name: state.project_name,
            feature_name: state.feature_name,
            todo_origin: state.todo_origin,
            issue_source: state.issue_source,
            branch: state.branch,
            mode: state.mode,
            review: state.review,
            plan_mode: state.plan_mode,
            quick_plan: state.quick_plan,
            agent: state.agent,
            create_terminal: state.create_terminal,
            session_name: state.session_name,
            enable_chrome: state.enable_chrome,
            remote_control: state.remote_control,
            steering_enabled: state.steering_enabled,
            startup_prompt: state.startup_prompt,
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

/// A file-picking browser for attaching a reference document to a plan
/// interview. Kept apart from [`BrowsePathState`] on purpose: that one is a
/// directory picker welded to project creation, this one selects a file and
/// carries the interview it must return to.
pub struct AttachDocState {
    pub explorer: FileExplorer,
    /// The interview this picker was opened from, moved in whole so confirm
    /// and cancel can restore it without a rebuild.
    pub interview: PlanInterviewState,
    /// The reason the last selection was rejected, shown in the footer until
    /// the next keypress. `None` when nothing has been rejected.
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct CreateProjectState {
    pub step: CreateProjectStep,
    pub name: String,
    pub path: String,
    pub agent: AgentKind,
    pub agent_index: usize,
    /// Whether `path` is inside a git repository: `None` when the path does
    /// not name an existing directory (nothing to say yet). Refreshed when
    /// the path changes so the dialog never shells out while drawing.
    pub path_is_git: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CreateProjectStep {
    Name,
    Path,
    Agent,
}

impl CreateProjectState {
    /// An empty form. The repo path starts blank (the dialog shows a
    /// placeholder) rather than AMF's launch directory, which is rarely the
    /// project being added and used to be prefilled without saying so.
    pub fn blank() -> Self {
        Self {
            step: CreateProjectStep::Name,
            name: String::new(),
            path: String::new(),
            agent: AgentKind::default(),
            agent_index: 0,
            path_is_git: None,
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
    /// Set when the wizard was opened from a TODO, so the feature it creates
    /// can be linked back to that row. Lives on the wizard state (not on
    /// `App`) so cancelling the wizard drops it — a stale origin would attach
    /// the next unrelated feature to the wrong TODO.
    pub todo_origin: Option<TodoPlanOrigin>,
    pub issue_source: Option<crate::project::IssueSource>,
    /// Optional display name supplied by a workflow with separate feature and
    /// branch fields. Ordinary feature creation keeps this unset.
    pub feature_name: Option<String>,
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
    /// Set alongside `plan_mode` when the Plan field's 3-way cycle
    /// (`cycle_plan_choice`) lands on Quick Plan rather than full Plan mode.
    /// Meaningless when `plan_mode` is false. Kept as a second bool rather
    /// than replacing `plan_mode` with an enum because `plan_mode` is also
    /// the persisted `Feature`/DB/`FeaturePreset` field — an enum there would
    /// ripple into schema and automation-API surface this feature doesn't
    /// need to touch. Not threaded through the `on_worktree_created` hook
    /// continuation (`app/hooks.rs`) in v1: a feature created through that
    /// path with Quick Plan chosen falls back to full Plan mode, which is
    /// safe (still a guided interview) even though it isn't the requested one.
    pub quick_plan: bool,
    pub create_terminal: bool,
    pub session_name: String,
    pub source_index: usize,
    pub worktrees: Vec<WorktreeInfo>,
    pub worktree_index: usize,
    pub worktree_search_active: bool,
    pub worktree_query: String,
    pub use_worktree: bool,
    /// This is the project's first feature, whose defaults (name = current
    /// branch, no worktree) differ from later ones. The form says so.
    pub first_feature: bool,
    /// False when the project folder is not a git repository. Worktrees,
    /// branches and diffs need git, so the wizard disables the worktree
    /// "Yes" choice and explains why instead of failing at launch.
    pub git_available: bool,
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

/// The name a feature's first agent session gets unless the user types one:
/// "Claude 1", "Codex 1", ...
pub fn default_session_name(agent: &AgentKind) -> String {
    format!("{} 1", agent.display_name())
}

impl CreateFeatureState {
    pub fn new(
        project_name: String,
        project_repo: PathBuf,
        worktrees: Vec<WorktreeInfo>,
        is_first_feature: bool,
    ) -> Self {
        // A project's first feature runs in the repo folder itself, so it is
        // named after the branch checked out there (the project's, not AMF's
        // launch directory's). Later features get new worktrees and start
        // blank: reusing that branch name would only collide with it.
        let branch = if is_first_feature {
            crate::worktree::WorktreeManager::current_branch(&project_repo)
                .ok()
                .flatten()
                .unwrap_or_default()
        } else {
            String::new()
        };

        let step = if worktrees.is_empty() {
            CreateFeatureStep::Branch
        } else {
            CreateFeatureStep::Source
        };
        Self {
            project_name,
            project_repo,
            todo_origin: None,
            issue_source: None,
            feature_name: None,
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
            quick_plan: false,
            create_terminal: false,
            session_name: "Claude 1".to_string(),
            source_index: 0,
            worktrees,
            worktree_index: 0,
            worktree_search_active: false,
            worktree_query: String::new(),
            use_worktree: !is_first_feature,
            first_feature: is_first_feature,
            git_available: true,
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

    /// The last `mode_focus` index on the options step. Claude has two more
    /// rows (Chrome, Remote Control) than the other harnesses:
    /// Claude: 0=harness 1=mode 2=review 3=plan 4=chrome 5=remote 6=steering;
    /// others: 0=harness 1=mode 2=review 3=plan 4=steering.
    pub fn mode_max_focus(&self) -> usize {
        if self.agent == AgentKind::Claude {
            6
        } else {
            4
        }
    }

    /// The feature reuses a worktree picked on the Source step, so its name
    /// and worktree rows are fixed and not part of the editable form.
    pub fn uses_existing_worktree(&self) -> bool {
        self.source_index == 1 && !self.worktrees.is_empty()
    }

    /// Whether `step` is one of the rows of the single-page form (name,
    /// worktree, options, session name), as opposed to the pickers in front
    /// of it or the SuperVibe confirmation after it.
    pub fn is_form_step(step: &CreateFeatureStep) -> bool {
        matches!(
            step,
            CreateFeatureStep::Branch
                | CreateFeatureStep::Worktree
                | CreateFeatureStep::Mode
                | CreateFeatureStep::SessionName
        )
    }

    /// Move to the next (`forward`) or previous form row, wrapping at either
    /// end — the same Tab/Shift+Tab behaviour as the New Project dialog.
    /// Does nothing outside the form.
    pub fn move_form_focus(&mut self, forward: bool) {
        if !Self::is_form_step(&self.step) {
            return;
        }
        let mut rows: Vec<(CreateFeatureStep, usize)> = Vec::new();
        if !self.uses_existing_worktree() {
            rows.push((CreateFeatureStep::Branch, 0));
            rows.push((CreateFeatureStep::Worktree, 0));
        }
        for focus in 0..=self.mode_max_focus() {
            rows.push((CreateFeatureStep::Mode, focus));
        }
        rows.push((CreateFeatureStep::SessionName, 0));

        let current_focus = if self.step == CreateFeatureStep::Mode {
            self.mode_focus
        } else {
            0
        };
        let current = rows
            .iter()
            .position(|(step, focus)| *step == self.step && *focus == current_focus)
            .unwrap_or(0);
        let next = if forward {
            (current + 1) % rows.len()
        } else {
            (current + rows.len() - 1) % rows.len()
        };
        let (step, focus) = rows[next].clone();
        self.step = step;
        if self.step == CreateFeatureStep::Mode {
            self.mode_focus = focus;
        }
    }

    /// Select the harness at `index`. The session name follows the harness
    /// ("Claude 1" → "Codex 1") until the user has typed their own.
    pub fn select_agent(&mut self, index: usize) {
        let Some(agent) = self.allowed_agents.get(index).cloned() else {
            return;
        };
        self.set_agent(agent);
        self.agent_index = index;
    }

    /// Set the harness, keeping an untouched default session name in step.
    pub fn set_agent(&mut self, agent: AgentKind) {
        if self.session_name.trim() == default_session_name(&self.agent) {
            self.session_name = default_session_name(&agent);
        }
        self.agent = agent;
        if self.step == CreateFeatureStep::Mode && self.mode_focus > self.mode_max_focus() {
            self.mode_focus = self.mode_max_focus();
        }
    }

    pub fn focused_mode_description(&self) -> Option<&'static str> {
        match self.mode_focus {
            0 | 1 => None,
            2 => Some(
                "High token usage: writes developer notes with every code change for a detailed code review.",
            ),
            3 if self.plan_mode && self.quick_plan => Some(
                "Quick Plan: a dynamically-sized round of clarifying questions before work starts, or none at all for a trivial task.",
            ),
            3 if self.plan_mode => {
                Some("Start in planning mode so the agent discusses the approach before editing.")
            }
            3 => Some(
                "Cycle to Quick Plan or full Plan mode for a guided interview before work starts.",
            ),
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

    /// Cycle the Plan field's 3-state choice: None → Quick Plan → Full Plan →
    /// None (`forward`), or the reverse. `(plan_mode, quick_plan)` encodes the
    /// three states as `(false, false)`, `(true, true)`, `(true, false)`.
    pub fn cycle_plan_choice(&mut self, forward: bool) {
        let next = match (self.plan_mode, self.quick_plan, forward) {
            (false, _, true) => (true, true),
            (true, true, true) => (true, false),
            (true, false, true) => (false, false),
            (false, _, false) => (true, false),
            (true, false, false) => (true, true),
            (true, true, false) => (false, false),
        };
        (self.plan_mode, self.quick_plan) = next;
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

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedFeatureLaunch {
    pub(crate) model_selection: Option<super::model_analysis::Selection>,
    pub project_name: String,
    /// Separate display/persisted feature name for workflows whose branch is
    /// user-editable. Ordinary creation leaves this `None` and uses `branch`.
    pub feature_name: Option<String>,
    pub branch: String,
    pub workdir: PathBuf,
    pub is_worktree: bool,
    pub mode: VibeMode,
    pub review: bool,
    pub plan_mode: bool,
    /// Route `plan_mode`'s deferred launch through Quick Plan rather than the
    /// full Plan-mode interview. Meaningless when `plan_mode` is false;
    /// unread past `App::finish_feature_launch` — see the note on
    /// `CreateFeatureState::quick_plan`, including the `on_worktree_created`
    /// hook scope cut.
    pub quick_plan: bool,
    pub agent: AgentKind,
    pub create_terminal: bool,
    pub session_name: String,
    pub enable_chrome: bool,
    pub remote_control: bool,
    pub steering_enabled: bool,
    pub hook_succeeded: Option<bool>,
    /// Optional composer seed to show immediately after the agent starts.
    pub startup_prompt: Option<String>,
    /// Set when this launch was started from a TODO, so accepting the plan can
    /// link the created feature back to the row it came from.
    pub todo_origin: Option<TodoPlanOrigin>,
    pub issue_source: Option<crate::project::IssueSource>,
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
    /// The batch adds features to an existing project (the one selected when
    /// `B` was pressed) instead of creating a new project. The path and name
    /// steps are skipped: they would only re-ask what the selection says.
    pub existing_project: bool,
}

impl CreateBatchFeaturesState {
    /// A batch that also creates its project. `workspace_path` prefills the
    /// path; `None` leaves it blank rather than guessing AMF's launch
    /// directory.
    pub fn with_workspace(workspace_path: Option<String>) -> Self {
        let repo_path = workspace_path.unwrap_or_default();
        let workspace_name = Self::name_from_path(&repo_path);

        Self {
            workspace_path: repo_path,
            project_name: workspace_name,
            feature_count: 3,
            feature_prefix: "feature-".to_string(),
            agent: AgentKind::default(),
            agent_index: 0,
            mode: VibeMode::default(),
            mode_index: 0,
            mode_focus: 0,
            review: false,
            enable_chrome: false,
            step: CreateBatchFeaturesStep::WorkspacePath,
            existing_project: false,
        }
    }

    /// The most features one batch creates (typed digits are capped too).
    pub const MAX_FEATURES: usize = 50;

    /// A batch of features added to the existing project `project_name`,
    /// starting at the feature count.
    pub fn for_project(project_name: String, repo: &std::path::Path) -> Self {
        let mut state = Self::with_workspace(Some(repo.to_string_lossy().into_owned()));
        state.project_name = project_name;
        state.existing_project = true;
        state.step = CreateBatchFeaturesStep::FeatureCount;
        state
    }

    /// The folder name of `path`, used as the default project name.
    pub fn name_from_path(path: &str) -> String {
        std::path::Path::new(path.trim())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string()
    }

    /// The feature (and branch) names the batch will create, in order. Must
    /// match `App::planned_batch_feature_results`: prefix then number, with
    /// no separator added (`feature-` → `feature-1`).
    pub fn planned_names(&self) -> Vec<String> {
        (1..=self.feature_count)
            .map(|i| format!("{}{}", self.feature_prefix, i))
            .collect()
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
    use crate::db::plan_interviews::{PlanInterviewRecord, PlanInterviewStage};
    use crate::plan_interview::{
        CUSTOM_ANSWER_MAX_LEN, PlanQuestion, PlanQuestionKind, QuestionSource,
    };

    fn key_char(c: char) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char(c),
            crossterm::event::KeyModifiers::NONE,
        )
    }

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

        // From "nothing picked" the up-arrow wraps to the last option.
        state.select_previous_option();
        assert_eq!(state.selected_option, Some(1));
        state.advance().unwrap();
        assert_eq!(state.answers[0].as_deref(), Some("Session"));

        assert!(state.back());
        assert_eq!(state.selected_option, Some(1));
        state.select_next_option();
        assert_eq!(state.selected_option, Some(0));
    }

    #[test]
    fn plan_interview_clear_option_selection_returns_to_nothing_picked() {
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

        // No-op with nothing picked.
        assert!(!state.clear_option_selection());

        state.select_next_option();
        assert_eq!(state.selected_option, Some(0));

        // A stray pick can be undone in place.
        assert!(state.clear_option_selection());
        assert_eq!(state.selected_option, None);
        assert!(!state.clear_option_selection());
    }

    #[test]
    fn plan_interview_choice_question_takes_a_custom_answer_with_or_without_a_pick() {
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

        // Custom text alone answers a required choice question.
        assert!(state.open_custom_answer_editor());
        state.custom_answer_handle_key(key_char('t'));
        state.custom_answer_handle_key(key_char('u'));
        state.custom_answer_handle_key(key_char('i'));
        state.commit_custom_answer();
        assert!(!state.custom_answer_focused);
        assert_eq!(state.selected_option, None);
        state.advance().unwrap();
        assert_eq!(state.answers[0].as_deref(), Some("tui"));

        // Revisiting re-presents the structured control: no pick, custom text
        // back in the box.
        assert!(state.back());
        assert_eq!(state.selected_option, None);
        assert_eq!(state.editor.text(), "tui");

        // Pick an option and keep the elaboration: the two combine.
        state.select_next_option();
        assert_eq!(state.selected_option, Some(0));
        state.advance().unwrap();
        assert_eq!(state.answers[0].as_deref(), Some("Dashboard — tui"));

        // And that round-trips back to selection + custom text.
        assert!(state.back());
        assert_eq!(state.selected_option, Some(0));
        assert_eq!(state.editor.text(), "tui");
    }

    #[test]
    fn plan_interview_blank_custom_answer_and_no_pick_stays_unanswered() {
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

        // Nothing picked, custom text blank: a required question blocks submit.
        assert_eq!(
            state.advance(),
            Err(PlanInterviewAdvanceError::AnswerRequired)
        );
        assert_eq!(state.answers[0], None);

        // Esc restores the buffer the editor opened with.
        assert!(state.open_custom_answer_editor());
        state.custom_answer_handle_key(key_char('x'));
        state.cancel_custom_answer();
        assert_eq!(state.editor.text(), "");
        assert!(!state.custom_answer_focused);
    }

    #[test]
    fn plan_interview_custom_answer_enforces_the_length_cap() {
        let question = PlanQuestion {
            id: "surface".into(),
            text: "Where?".into(),
            kind: PlanQuestionKind::Select(vec!["A".into(), "B".into()]),
            source: crate::plan_interview::QuestionSource::Template,
            optional: true,
        };
        let mut state =
            PlanInterviewState::new("feature".into(), "feat-1".into(), vec![question], None);
        state.editor = TextEditor::new("brief".into());
        state.advance().unwrap();

        assert!(state.open_custom_answer_editor());
        state.editor = TextEditor::new("x".repeat(CUSTOM_ANSWER_MAX_LEN));
        // One more character is rejected; the buffer is left at the cap.
        state.custom_answer_handle_key(key_char('y'));
        assert_eq!(state.editor.text().chars().count(), CUSTOM_ANSWER_MAX_LEN);
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
        // The question is unanswered again, so the resume lands on it with
        // nothing picked.
        assert_eq!(state.phase, PlanInterviewPhase::StaticQuestions);
        assert_eq!(state.question_index, 0);
        assert_eq!(state.selected_option, None);
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
        assert_eq!(state.selected_option, Some(1));
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
            model_selection: None,
            project_name: project_name.into(),
            feature_name: None,
            branch: branch.into(),
            workdir: PathBuf::from("/tmp/does-not-matter"),
            is_worktree: false,
            mode: VibeMode::default(),
            review: false,
            plan_mode: true,
            quick_plan: false,
            agent: AgentKind::Claude,
            create_terminal: false,
            session_name: "Claude 1".into(),
            enable_chrome: false,
            remote_control: false,
            steering_enabled: false,
            hook_succeeded: None,
            startup_prompt: None,
            todo_origin: None,
            issue_source: None,
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
            custom_answers: Vec::new(),
            plan: None,
            ai_rounds_completed: 0,
            attached_docs: Vec::new(),
            expert_brief: None,
            preflight_fingerprint: None,
            preflight_status: None,
            preflight_model: None,
            preflight_token_estimate: 0,
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
        stored.preflight_model = Some("opus".into());

        let mut state = PlanInterviewState::new("feature".into(), "feat-1".into(), questions, None);
        state.offer_resume(stored);

        assert!(state.resume_from_draft());

        assert_eq!(state.phase, PlanInterviewPhase::Review);
        assert_eq!(state.synthesized_plan.as_deref(), Some("# Plan: feature\n"));
        assert_eq!(state.expert_model.as_deref(), Some("opus"));
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

    #[test]
    fn quick_plan_constructors_start_with_no_static_questions_and_the_quick_kind() {
        let creation = PlanInterviewState::for_feature_creation_quick(prepared_launch(
            "my-project",
            "planned-feature",
        ));
        assert_eq!(creation.kind, PlanInterviewMode::Quick);
        assert!(creation.questions.is_empty());
        assert_eq!(
            creation.max_ai_rounds(),
            crate::plan_interview::MAX_QUICK_AI_ROUNDS
        );

        let on_demand = PlanInterviewState::for_feature_quick(
            "feature".into(),
            "feat-1".into(),
            PathBuf::from("/tmp/does-not-matter"),
            AgentKind::Claude,
        );
        assert_eq!(on_demand.kind, PlanInterviewMode::Quick);
        assert!(on_demand.questions.is_empty());
    }

    #[test]
    fn full_plan_constructors_default_to_the_full_kind() {
        let creation = PlanInterviewState::for_feature_creation(
            prepared_launch("my-project", "planned-feature"),
            vec![template_question("scope")],
        );
        assert_eq!(creation.kind, PlanInterviewMode::Full);
        assert_eq!(
            creation.max_ai_rounds(),
            crate::plan_interview::MAX_AI_ROUNDS
        );
    }

    #[test]
    fn cycle_plan_choice_visits_none_quick_full_and_back_going_forward() {
        let mut state = CreateFeatureState::new(
            "my-project".into(),
            PathBuf::from("/tmp/does-not-matter"),
            Vec::new(),
            true,
        );
        assert_eq!((state.plan_mode, state.quick_plan), (false, false));

        state.cycle_plan_choice(true);
        assert_eq!(
            (state.plan_mode, state.quick_plan),
            (true, true),
            "None -> Quick Plan"
        );

        state.cycle_plan_choice(true);
        assert_eq!(
            (state.plan_mode, state.quick_plan),
            (true, false),
            "Quick Plan -> Full Plan"
        );

        state.cycle_plan_choice(true);
        assert_eq!(
            (state.plan_mode, state.quick_plan),
            (false, false),
            "Full Plan -> None"
        );
    }

    #[test]
    fn cycle_plan_choice_visits_the_same_states_in_reverse_going_backward() {
        let mut state = CreateFeatureState::new(
            "my-project".into(),
            PathBuf::from("/tmp/does-not-matter"),
            Vec::new(),
            true,
        );

        state.cycle_plan_choice(false);
        assert_eq!(
            (state.plan_mode, state.quick_plan),
            (true, false),
            "None -> Full Plan"
        );

        state.cycle_plan_choice(false);
        assert_eq!(
            (state.plan_mode, state.quick_plan),
            (true, true),
            "Full Plan -> Quick Plan"
        );

        state.cycle_plan_choice(false);
        assert_eq!(
            (state.plan_mode, state.quick_plan),
            (false, false),
            "Quick Plan -> None"
        );
    }
}
