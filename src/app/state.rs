pub use crate::app::learning::state::*;
pub use crate::app::pr_review::state::*;
pub use crate::app::review::state::*;
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
use crate::plan_interview::{
    CUSTOM_ANSWER_MAX_LEN, PlanQuestion, PlanQuestionKind, QuestionSource, serialize_choice_answer,
    split_choice_answer,
};
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

        self.session_kind
            .is_agent_harness()
            .then(|| self.session_kind.clone())
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

// ── Prompt-override manager (Editable Headless Prompts) ──────────────────

/// Which scope an override is being saved to. `Feature` is only offered when
/// a feature is in context and a database is present.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptOverrideScope {
    Feature,
    Project,
    Global,
}

impl PromptOverrideScope {
    pub fn label(self) -> &'static str {
        match self {
            PromptOverrideScope::Feature => "This feature",
            PromptOverrideScope::Project => "This project (amf.json)",
            PromptOverrideScope::Global => "Global (all projects)",
        }
    }
}

/// The step the editor is on: typing the template, then picking a scope, then
/// picking a harness variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PromptOverrideStep {
    Editing,
    ScopePicker,
    HarnessPicker,
}

/// One list row: a registry prompt and where its effective template (for the
/// shared/no-harness view) currently comes from.
#[derive(Clone)]
pub struct PromptOverrideRow {
    pub id: crate::prompts::PromptId,
    pub source: crate::prompts::PromptSource,
    /// True when the row has an override at that scope (any harness).
    pub has_feature: bool,
    pub has_project: bool,
    pub has_global: bool,
}

/// The editor sub-state, open for `rows[row]`.
pub struct PromptOverrideEditState {
    pub row: usize,
    pub editor: TextEditor,
    pub step: PromptOverrideStep,
    /// Scopes offered, narrowest first. `Feature` is present only with a
    /// feature in context and a DB.
    pub scopes: Vec<PromptOverrideScope>,
    pub scope_index: usize,
    /// 0 = shared; 1..=4 = `AgentKind::ALL[i-1]`.
    pub harness_index: usize,
}

impl PromptOverrideEditState {
    pub fn scope(&self) -> PromptOverrideScope {
        self.scopes
            .get(self.scope_index)
            .copied()
            .unwrap_or(PromptOverrideScope::Global)
    }

    /// `None` = shared across harnesses; `Some` = one specific harness.
    pub fn harness(&self) -> Option<crate::project::AgentKind> {
        self.harness_index
            .checked_sub(1)
            .and_then(|i| crate::project::AgentKind::ALL.get(i).cloned())
    }
}

/// The prompt-override manager overlay: a list of every registry prompt with
/// its effective source, an inline template editor, and scope / harness
/// pickers on save.
pub struct PromptOverridesState {
    /// One row per `crate::prompts::PromptId::ALL`, in that order.
    pub rows: Vec<PromptOverrideRow>,
    pub selected: usize,
    pub scroll: usize,
    pub edit: Option<PromptOverrideEditState>,
    pub help_open: bool,
    /// A second `d` in the list confirms clearing the selected row's override.
    pub confirm_clear: bool,
    pub from_view: Option<ViewState>,
}

impl PromptOverridesState {
    pub fn selected_row(&self) -> Option<&PromptOverrideRow> {
        self.rows.get(self.selected)
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

/// Which scope a pane of the TODOs overlay shows. The variants are in the
/// order the panes are laid out and the order ties between them resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoPaneKind {
    /// This feature's own checkout. Absent for a feature sitting on the repo
    /// root, which has no worktree of its own to scope a list to.
    Worktree,
    /// The project's list — the only scope that existed before scoping.
    Project,
    /// The machine-wide list, belonging to no project.
    Global,
}

impl TodoPaneKind {
    /// The pane header, and the noun used when a message has to name a scope.
    pub fn label(self) -> &'static str {
        match self {
            TodoPaneKind::Worktree => "Worktree",
            TodoPaneKind::Project => "Project",
            TodoPaneKind::Global => "Global",
        }
    }
}

/// One scope's list within the TODOs overlay: its own items, cursor, scroll,
/// and scratchpad, so switching focus never disturbs the pane being left.
pub struct TodoPane {
    pub kind: TodoPaneKind,
    /// The scope this pane reads and writes. Carries the project id and
    /// workdir, so it is the whole key to the pane's list.
    pub scope: crate::db::todos::TodoScope,
    /// Name shown in the pane header beside the scope label (the worktree's
    /// feature name, the project name, or nothing for global).
    pub title: String,
    /// The loaded list (scratchpad note, id). `None` until the list exists —
    /// with no DB (tests) it is synthesized on first write, and with one it is
    /// created lazily, so an untouched scope leaves no row behind.
    pub list: Option<crate::db::todos::TodoList>,
    /// Items in display order (open first, then by sort_order).
    pub todos: Vec<crate::db::todos::Todo>,
    /// Cursor into `todos`.
    pub selected: usize,
    /// Vertical scroll offset into this pane's list area.
    pub scroll_offset: usize,
}

impl TodoPane {
    pub fn selected_todo(&self) -> Option<&crate::db::todos::Todo> {
        self.todos.get(self.selected)
    }

    /// The list's free-form scratchpad note (stored in the legacy
    /// `carry_over` column), when it is not blank.
    pub fn scratchpad(&self) -> Option<&str> {
        self.list
            .as_ref()
            .and_then(|l| l.carry_over.as_deref())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
}

/// State for the native TODOs overlay (`AppMode::Todos`): up to three scoped
/// panes, which one has focus, and any in-progress edit or prompt layered over
/// them.
pub struct TodoViewState {
    /// Project / feature indices the TODOs session lives under, used to
    /// resolve the session for selection on close and to host new lists.
    pub pi: usize,
    pub fi: usize,
    /// Display labels for the header.
    pub project_name: String,
    pub feature_name: String,
    /// The scoped panes, always ordered worktree → project → global. The
    /// worktree pane is absent for a feature on the repo root, which is the
    /// only way this is shorter than three.
    pub panes: Vec<TodoPane>,
    /// Index into `panes` of the visible pane that owns the cursor. `None` is
    /// valid for a repository-root feature when both optional scopes are
    /// hidden.
    pub focus: Option<usize>,
    /// Active inline edit, if any (add/edit title/notes/scratchpad).
    pub editor: Option<TodoEditor>,
    /// Whether inline edits open with the vim keymap. Remembered on the state
    /// (not the editor, which is rebuilt for each edit) for the life of the
    /// overlay; a fresh overlay starts with vim off, matching the other opt-in
    /// editor surfaces (compose, final review). Toggled with `Ctrl+T`.
    pub todo_vim_enabled: bool,
    /// Set when a delete is awaiting y/n confirmation.
    pub pending_delete: bool,
    /// Active launch step (chooser / destination), layered over the list.
    pub launch: Option<TodoLaunchStep>,
    /// Active move/copy scope chooser (`M` / `C`), layered the same way.
    pub scope_move: Option<TodoScopeMoveState>,
}

impl TodoViewState {
    pub fn pane_is_visible(pane: &TodoPane, project_visible: bool, global_visible: bool) -> bool {
        match pane.kind {
            TodoPaneKind::Worktree => true,
            TodoPaneKind::Project => project_visible,
            TodoPaneKind::Global => global_visible,
        }
    }

    /// Indices of actionable panes in worktree → project → global order.
    pub fn visible_pane_indices(&self, project_visible: bool, global_visible: bool) -> Vec<usize> {
        self.panes
            .iter()
            .enumerate()
            .filter_map(|(index, pane)| {
                Self::pane_is_visible(pane, project_visible, global_visible).then_some(index)
            })
            .collect()
    }

    pub fn focused(&self) -> Option<&TodoPane> {
        self.focus.and_then(|focus| self.panes.get(focus))
    }

    pub fn focused_mut(&mut self) -> Option<&mut TodoPane> {
        self.focus.and_then(|focus| self.panes.get_mut(focus))
    }
}

/// The scope chooser raised by `M` (move) and `C` (copy) over the selected
/// TODO. Targets are pane indices rather than scopes so the in-memory pane and
/// the persisted list are updated from one lookup.
pub struct TodoScopeMoveState {
    /// `true` for a copy (leaves the original in place, unstarted), `false`
    /// for a move (re-files the same item, links and all).
    pub copy: bool,
    /// The item being re-filed, by id, so the list changing underneath the
    /// prompt is noticed rather than acted on stale.
    pub todo_id: String,
    pub todo_title: String,
    /// Candidate destinations as `(label, pane index)` — every pane but the
    /// one the item is already in.
    pub targets: Vec<(String, usize)>,
    pub selected: usize,
}

impl TodoScopeMoveState {
    pub fn move_cursor(&mut self, delta: isize) {
        if self.targets.is_empty() {
            return;
        }
        let last = self.targets.len() as isize - 1;
        self.selected = ((self.selected as isize) + delta).clamp(0, last) as usize;
    }
}

/// Single-line quick-capture of a TODO from inside a session view. The typed
/// title is appended to the session feature's own worktree list — falling back
/// to the project list when that feature sits on the repo root — auto-creating
/// the list (and a TODOs session under the current feature) when there is none
/// yet. `view` is the session view to return to on commit/cancel.
pub struct TodoQuickCaptureState {
    pub view: ViewState,
    /// Name of the project the TODO will be added to (shown in the dialog).
    pub project_name: String,
    /// Which list this capture will land in, named in the overlay so the
    /// target is never a guess (e.g. `"Worktree · add-login"`).
    pub list_label: String,
    /// The title being typed.
    pub input: String,
}

/// Collects the user's instruction before starting a fresh-context agent
/// session (`Ctrl+Space` then `Shift+F`, `crate::app::handoff`), so the
/// seeded prompt is complete on arrival instead of asking the user to type
/// over a placeholder in the new session's compose box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreshContextPromptSource {
    Manual,
    ContextHint,
}

pub struct FreshContextPromptState {
    pub view: ViewState,
    /// Feature the fresh session will be created in, shown in the dialog.
    pub feature_name: String,
    /// The instruction being typed.
    pub input: String,
    /// Whether the input came from the context-hint continuation generator.
    pub source: FreshContextPromptSource,
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

/// Which TODO a plan-mode run was started from, carried through the interview
/// so accepting the plan can link the result back to the row it came from.
///
/// The list id rides along with the todo id because the overlay may be closed
/// by the time the plan is accepted — a new-feature run leaves the Todos mode
/// entirely — so the link has to be written straight to the DB rather than
/// through the in-memory list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoPlanOrigin {
    pub todo_id: String,
    pub list_id: String,
    /// The TODO's title when the run started. Used for the interview header
    /// and to name the plan file, so it is captured rather than re-read: the
    /// title can be edited while the interview runs.
    pub todo_title: String,
    /// The feature hosting the TODO list when the run started. The host-feature
    /// destination spawns its session here; the new-feature destination
    /// ignores it.
    pub host_feature_id: String,
}

/// What `g`/`Enter` on an unlinked TODO offers: spawn in an existing feature,
/// spawn in a fresh worktree, or run a plan interview first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoLaunchAction {
    /// Today's behavior: an agent session in the host feature (or, for a
    /// project/global TODO, a feature the user picks), composer seeded with the
    /// TODO, editable and unsent.
    SpawnSession,
    /// Create a new AMF feature and git worktree for this TODO — no plan
    /// interview — then seed its agent with the TODO.
    SpawnInNewFeature,
    /// Run the guided plan interview with the TODO as its brief.
    PlanMode,
}

impl TodoLaunchAction {
    pub const ALL: [TodoLaunchAction; 3] = [
        TodoLaunchAction::SpawnSession,
        TodoLaunchAction::SpawnInNewFeature,
        TodoLaunchAction::PlanMode,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TodoLaunchAction::SpawnSession => "Start an agent on this TODO",
            TodoLaunchAction::SpawnInNewFeature => "Start an agent in a new feature",
            TodoLaunchAction::PlanMode => "Plan this TODO first",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            TodoLaunchAction::SpawnSession => {
                "Opens a session in this feature with the TODO in the composer, unsent."
            }
            TodoLaunchAction::SpawnInNewFeature => {
                "Creates a new branch and worktree, then seeds its agent with the TODO, unsent."
            }
            TodoLaunchAction::PlanMode => {
                "Runs the discovery interview, then starts work from the plan you accept."
            }
        }
    }
}

/// Where an accepted TODO plan lands. Chosen up front, before the interview, so
/// the plan is written into the worktree it is actually for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoPlanDestination {
    /// The feature hosting the TODO list. Accepting spawns a seeded session
    /// there; nothing new is checked out.
    HostFeature,
    /// A new AMF feature and git worktree, created through the ordinary
    /// create-feature wizard with plan mode on.
    NewFeature,
}

impl TodoPlanDestination {
    pub const ALL: [TodoPlanDestination; 2] = [
        TodoPlanDestination::HostFeature,
        TodoPlanDestination::NewFeature,
    ];
}

/// The four answers to "this TODO already has work started for it".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoImplementChoice {
    /// Go to the feature or session the earlier run created.
    Jump,
    /// Start a second agent on it anyway, in the host feature.
    SpawnNew,
    /// Leave this one alone and scan on for the next candidate.
    SkipToNext,
    /// Change nothing and return to where the key was pressed.
    Cancel,
}

impl TodoImplementChoice {
    pub const ALL: [TodoImplementChoice; 4] = [
        TodoImplementChoice::Jump,
        TodoImplementChoice::SpawnNew,
        TodoImplementChoice::SkipToNext,
        TodoImplementChoice::Cancel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TodoImplementChoice::Jump => "Go to the work already started",
            TodoImplementChoice::SpawnNew => "Start another agent on it",
            TodoImplementChoice::SkipToNext => "Skip it and take the next TODO",
            TodoImplementChoice::Cancel => "Cancel",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            TodoImplementChoice::Jump => {
                "Opens the feature or session this TODO was launched into."
            }
            TodoImplementChoice::SpawnNew => {
                "Opens a second session in the host feature, composer seeded and unsent."
            }
            TodoImplementChoice::SkipToNext => {
                "Leaves this one as it is and scans on for the next candidate."
            }
            TodoImplementChoice::Cancel => "Changes nothing.",
        }
    }
}

/// "Implement next" found that its best candidate already has work started for
/// it (`AppMode::TodoImplementChoice`).
///
/// Reached from two surfaces — a TODOs session row on the dashboard and the
/// TODOs overlay — so it is an [`AppMode`] of its own rather than a step
/// layered inside [`TodoViewState`], which only one of the two has. `origin`
/// is the mode the key was pressed in, restored verbatim on every exit: from
/// the overlay that returns the list with its cursor, scroll, and any unsaved
/// in-memory rows intact.
pub struct TodoImplementChoiceState {
    /// The mode to return to. Boxed because it may be a `Todos` overlay, whose
    /// state is large and (holding a `TextEditor`) not clonable.
    pub origin: Box<AppMode>,
    /// Project and fallback feature indices the scan ran under.
    pub pi: usize,
    pub fallback_fi: usize,
    /// The list's host feature id, when the list could be loaded.
    pub host_feature_id: Option<String>,
    /// Which scope the candidate came from. Decides whether *Start another
    /// agent on it* can spawn straight away (worktree) or has to ask which
    /// feature to spawn in (project / global).
    pub pane_kind: TodoPaneKind,
    /// The candidate: its id, so it is re-resolved by id alone on confirm
    /// (not trusted, and not tied to a remembered list — the item may have
    /// been moved to a different list while this prompt was open), and its
    /// title for display.
    pub todo_id: String,
    pub todo_title: String,
    /// TODOs already passed over by *Skip to next*, carried so a resumed scan
    /// does not offer the same item again. Ids, not indices: the list may be
    /// reordered underneath.
    pub skipped_ids: Vec<String>,
    pub selected: usize,
}

impl TodoImplementChoiceState {
    pub fn move_cursor(&mut self, delta: isize) {
        let last = TodoImplementChoice::ALL.len().saturating_sub(1) as isize;
        self.selected = ((self.selected as isize) + delta).clamp(0, last) as usize;
    }

    pub fn choice(&self) -> TodoImplementChoice {
        TodoImplementChoice::ALL[self.selected.min(TodoImplementChoice::ALL.len() - 1)]
    }
}

/// A feature to put an agent on a project- or global-scoped TODO in
/// (`AppMode::TodoSpawnTarget`).
///
/// A whole [`AppMode`] rather than a step inside [`TodoViewState`] for the
/// same reason [`TodoImplementChoiceState`] is one: the dashboard's "implement
/// next" reaches it with no overlay open. `origin` is the mode the key was
/// pressed in, restored verbatim on cancel, so the prompt never costs the user
/// their place in the list.
pub struct TodoSpawnTargetState {
    pub origin: Box<AppMode>,
    /// The item to spawn on, carried by value: the overlay it came from may
    /// not be open, and a global TODO's list is not reachable from `pi`.
    pub todo: crate::db::todos::Todo,
    /// Which scope the TODO came from, shown so the user knows why they are
    /// being asked.
    pub pane_kind: TodoPaneKind,
    /// Candidates as `(label, pi, fi)`. A project-scoped TODO lists that
    /// project's features; a global one lists every project's.
    pub candidates: Vec<(String, usize, usize)>,
    pub selected: usize,
    /// Set when the user explicitly asked for a second agent on an item that
    /// already has one, so the spawn does not reuse the existing session.
    pub force_new: bool,
}

impl TodoSpawnTargetState {
    pub fn move_cursor(&mut self, delta: isize) {
        if self.candidates.is_empty() {
            return;
        }
        let last = self.candidates.len() as isize - 1;
        self.selected = ((self.selected as isize) + delta).clamp(0, last) as usize;
    }

    pub fn selection(&self) -> Option<(usize, usize)> {
        self.candidates
            .get(self.selected)
            .map(|(_, pi, fi)| (*pi, *fi))
    }
}

/// What to do with the unfinished TODOs in a worktree list whose feature is
/// being deleted (`AppMode::TodoDeleteDisposition`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TodoDeleteDisposition {
    MoveToProject,
    MoveToGlobal,
    Delete,
    Cancel,
}

impl TodoDeleteDisposition {
    pub const ALL: [TodoDeleteDisposition; 4] = [
        TodoDeleteDisposition::MoveToProject,
        TodoDeleteDisposition::MoveToGlobal,
        TodoDeleteDisposition::Delete,
        TodoDeleteDisposition::Cancel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            TodoDeleteDisposition::MoveToProject => "Move them to the project list",
            TodoDeleteDisposition::MoveToGlobal => "Move them to the global list",
            TodoDeleteDisposition::Delete => "Delete them with the worktree",
            TodoDeleteDisposition::Cancel => "Cancel — keep the feature",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            TodoDeleteDisposition::MoveToProject => {
                "They stay with the project and show up in its pane."
            }
            TodoDeleteDisposition::MoveToGlobal => {
                "They leave the project for the machine-wide list."
            }
            TodoDeleteDisposition::Delete => "The items go away for good, with the list.",
            TodoDeleteDisposition::Cancel => {
                "Nothing is deleted — the feature and its worktree stay."
            }
        }
    }
}

/// The prompt raised before a feature is deleted while its worktree list still
/// has unfinished items. Deleting a worktree is hard to reverse, so this is a
/// blocking prompt: nothing is killed or removed until a choice is made, and
/// *Cancel* returns to the dashboard with the feature intact.
pub struct TodoDeleteDispositionState {
    pub project_name: String,
    pub feature_name: String,
    /// The doomed feature's id. Carried so *Move to the project list* can host
    /// a newly-created project list on a feature that will still be there
    /// afterwards — hosting it on this one would hand the items straight to
    /// the orphaned-list cleanup that runs once the deletion completes.
    pub feature_id: String,
    /// The worktree list and where it lives, so the disposition can be applied
    /// without re-deriving it from indices that the deletion will invalidate.
    pub project_id: String,
    pub workdir: String,
    pub list_id: String,
    /// How many items are still open, stated in the prompt.
    pub unfinished: usize,
    pub selected: usize,
}

impl TodoDeleteDispositionState {
    pub fn move_cursor(&mut self, delta: isize) {
        let last = TodoDeleteDisposition::ALL.len() as isize - 1;
        self.selected = ((self.selected as isize) + delta).clamp(0, last) as usize;
    }

    pub fn choice(&self) -> TodoDeleteDisposition {
        TodoDeleteDisposition::ALL[self.selected.min(TodoDeleteDisposition::ALL.len() - 1)]
    }
}

/// A modal step layered over the TODO list, the way `pending_delete` and
/// `editor` are: the list, cursor, and scroll stay intact underneath, so `Esc`
/// returns to exactly what the user was looking at.
///
/// A separate [`AppMode`] would replace [`TodoViewState`] wholesale and force
/// the list to be reloaded — and the in-memory list is the overlay's source of
/// truth, so it would also lose unsaved state on a DB-less run.
#[derive(Debug, Clone)]
pub enum TodoLaunchStep {
    /// `g`/`Enter` on a TODO with no existing link: spawn, or plan first.
    Choice {
        origin: TodoPlanOrigin,
        selected: usize,
    },
    /// Where an accepted plan should land. Asked before the interview so the
    /// plan is written into the worktree it is actually for.
    Destination {
        origin: TodoPlanOrigin,
        /// Name of the feature hosting the list, shown on the first option.
        host_feature_name: String,
        /// False when the project has no git repository, which makes a new
        /// worktree impossible. The option is still shown, and says why.
        can_create_worktree: bool,
        selected: usize,
    },
}

impl TodoLaunchStep {
    pub fn origin(&self) -> &TodoPlanOrigin {
        match self {
            TodoLaunchStep::Choice { origin, .. } => origin,
            TodoLaunchStep::Destination { origin, .. } => origin,
        }
    }

    pub fn selected(&self) -> usize {
        match self {
            TodoLaunchStep::Choice { selected, .. } => *selected,
            TodoLaunchStep::Destination { selected, .. } => *selected,
        }
    }

    pub fn option_count(&self) -> usize {
        match self {
            TodoLaunchStep::Choice { .. } => TodoLaunchAction::ALL.len(),
            TodoLaunchStep::Destination { .. } => TodoPlanDestination::ALL.len(),
        }
    }

    /// Move the cursor, clamped rather than wrapped: with two options, wrapping
    /// makes `j` and `k` the same key and the highlight appears not to move.
    pub fn move_cursor(&mut self, delta: isize) {
        let last = self.option_count().saturating_sub(1);
        let current = self.selected() as isize;
        let next = (current + delta).clamp(0, last as isize) as usize;
        match self {
            TodoLaunchStep::Choice { selected, .. } => *selected = next,
            TodoLaunchStep::Destination { selected, .. } => *selected = next,
        }
    }

    /// The chosen action, or `None` on the destination step.
    pub fn action(&self) -> Option<TodoLaunchAction> {
        match self {
            TodoLaunchStep::Choice { selected, .. } => {
                Some(TodoLaunchAction::ALL[(*selected).min(TodoLaunchAction::ALL.len() - 1)])
            }
            TodoLaunchStep::Destination { .. } => None,
        }
    }

    /// The chosen destination, or `None` on the chooser step.
    pub fn destination(&self) -> Option<TodoPlanDestination> {
        match self {
            TodoLaunchStep::Destination { selected, .. } => {
                Some(TodoPlanDestination::ALL[(*selected).min(TodoPlanDestination::ALL.len() - 1)])
            }
            TodoLaunchStep::Choice { .. } => None,
        }
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
    /// Running a strictly read-only investigation of one review comment off the
    /// UI thread; shows a modal loading frame over the stashed PR Triage pane.
    PrInvestigationLoading(PrInvestigationLoadState),
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
}

/// The view to return to plus the stable TODO identity to complete.
pub struct TodoReferenceCompletionState {
    pub view: ViewState,
    pub todo_id: String,
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
        /// Carried across the hook detour, which rebuilds the launch from
        /// scratch and would otherwise drop the TODO link on any project with
        /// an `on_worktree_created` hook.
        todo_origin: Option<TodoPlanOrigin>,
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
    pub todo_origin: Option<TodoPlanOrigin>,
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
    pub todo_origin: Option<TodoPlanOrigin>,
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
            todo_origin: state.todo_origin,
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
    /// Set when the wizard was opened from a TODO, so the feature it creates
    /// can be linked back to that row. Lives on the wizard state (not on
    /// `App`) so cancelling the wizard drops it — a stale origin would attach
    /// the next unrelated feature to the wrong TODO.
    pub todo_origin: Option<TodoPlanOrigin>,
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
            todo_origin: None,
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
    pub project_name: String,
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
}

/// An accepted plan's exact deferred feature launch.
///
/// The resource confirmation owns this after the interview has written the
/// plan but before the feature exists. Keeping the prepared launch and the
/// accepted markdown together lets confirmation resume without rebuilding the
/// wizard state, regenerating the plan, or losing the kickoff prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct PendingPlanLaunch {
    pub prepared: PreparedFeatureLaunch,
    pub interview_key: String,
    pub plan: String,
}

/// Which interview this [`PlanInterviewState`] is running.
///
/// The two share every phase, the round/synthesis machinery, and the Q&A UI
/// — `kind` only steers which [`crate::prompts::PromptId`] is dispatched and
/// how the synthesis response is interpreted (a single markdown plan for
/// `Full`, a three-way outcome for `Quick`). Escalation
/// (`App::escalate_quick_plan_to_full`) flips a live `Quick` interview to
/// `Full` in place — nothing about the state is reset, so the brief and any
/// answers already given carry forward into the full round.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanInterviewMode {
    Full,
    Quick,
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
    /// plans does not exist yet. Unread for a `Quick` interview: Quick Plan
    /// skips the `plan_interviews` table entirely (see [`Self::kind`]).
    pub interview_key: String,
    /// Full Plan-mode interview or Quick Plan. See [`PlanInterviewMode`].
    pub kind: PlanInterviewMode,
    pub phase: PlanInterviewPhase,
    pub questions: Vec<PlanQuestion>,
    pub question_index: usize,
    pub brief: String,
    pub answers: Vec<Option<String>>,
    pub editor: TextEditor,
    /// The highlighted/picked option for the current choice question, or `None`
    /// when nothing is picked — the state a user is in when they answer purely
    /// with custom text. Parallel `selected_option`/`editor` are transient
    /// scratch for the question on screen; the durable record is `answers` (the
    /// serialized combined string) and `custom_answers` (the raw custom text).
    pub selected_option: Option<usize>,
    /// Raw free-text custom answer per question, positionally paired with
    /// `questions`. Empty for a question with no custom text and for every
    /// free-text question. Kept alongside `answers` so revisiting a choice
    /// question restores the radio selection *and* the custom text rather than
    /// a flat editable string, and persisted so a resumed/re-run interview can
    /// do the same.
    pub custom_answers: Vec<String>,
    /// Whether the inline custom-answer editor (the `editor` buffer, reused for
    /// a choice question) currently has focus. `e` opens it; committing or
    /// cancelling returns focus to the option list without submitting.
    pub custom_answer_focused: bool,
    /// The custom-answer buffer captured when the editor was opened, restored
    /// verbatim if the edit is cancelled with `Esc`.
    pub custom_answer_backup: Option<String>,
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
    /// The custom-text half of a re-run's pre-filled choice answers, keyed by
    /// question id, so [`Self::restore_prior_answer`] can put back both the
    /// selection and the elaboration the previous interview accepted.
    pub prior_custom_answers: HashMap<String, String>,
    /// The live session an accepted on-demand plan is being offered to. Only
    /// set in [`PlanInterviewPhase::KickoffHandoff`], which is only reached
    /// after the plan file is already on disk.
    pub kickoff_handoff: Option<PlanKickoffTarget>,
    /// The TODO this interview was started from, if any. Carried so accepting
    /// the plan can record the result on that row — and so the header can say
    /// which TODO is being planned.
    pub todo_origin: Option<TodoPlanOrigin>,
    /// Reference documents the feature owner attached on the brief step, as
    /// canonical absolute paths. Empty is the norm; a non-empty list is the
    /// explicit, opt-in trigger that switches the round / synthesis / critique
    /// passes from a no-tools run to a read-only one so the interviewer can
    /// read those documents (and the surrounding codebase). The files are read
    /// at dispatch time, never here, so an edit between attaching and running
    /// is picked up. Persisted in the interview draft and accepted record.
    pub attached_docs: Vec<PathBuf>,
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

    /// The Quick Plan sibling of [`Self::for_feature_creation`]: always an
    /// empty static question bank (Quick Plan has no built-in question bank;
    /// every question comes from the dynamically-sized adaptive round) and
    /// `kind: PlanInterviewMode::Quick`.
    pub fn for_feature_creation_quick(pending_launch: PreparedFeatureLaunch) -> Self {
        let mut state = Self::for_feature_creation(pending_launch, Vec::new());
        state.kind = PlanInterviewMode::Quick;
        state
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

    /// The Quick Plan sibling of [`Self::for_feature`]: always an empty
    /// static question bank and `kind: PlanInterviewMode::Quick`.
    pub fn for_feature_quick(
        feature_name: String,
        feature_id: String,
        workdir: PathBuf,
        agent: AgentKind,
    ) -> Self {
        let mut state = Self::for_feature(feature_name, feature_id, Vec::new(), workdir, agent);
        state.kind = PlanInterviewMode::Quick;
        state
    }

    /// An interview started from a TODO that plans work into the TODO's
    /// **host feature**, which already exists.
    ///
    /// Keyed by the TODO rather than the feature
    /// ([`crate::plan_interview::todo_interview_key`]): the host feature has
    /// its own `P` draft and accepted transcript, and planning a TODO against
    /// it must not overwrite them or be pre-filled from them.
    ///
    /// The brief is pre-filled and the interview opens on it, so the composed
    /// text is something the user edits rather than something they are handed.
    pub fn for_todo(
        feature_name: String,
        origin: TodoPlanOrigin,
        questions: Vec<PlanQuestion>,
        workdir: PathBuf,
        agent: AgentKind,
        brief: String,
    ) -> Self {
        let interview_key = crate::plan_interview::todo_interview_key(&origin.todo_id);
        let mut state = Self::new(feature_name, interview_key, questions, None);
        state.workdir = workdir;
        state.preferred_harness = agent;
        state.editor = TextEditor::new(brief);
        state.todo_origin = Some(origin);
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
            kind: PlanInterviewMode::Full,
            phase: PlanInterviewPhase::Brief,
            questions,
            question_index: 0,
            brief: String::new(),
            answers: vec![None; answer_count],
            editor: TextEditor::new(String::new()),
            selected_option: None,
            custom_answers: vec![String::new(); answer_count],
            custom_answer_focused: false,
            custom_answer_backup: None,
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
            prior_custom_answers: HashMap::new(),
            kickoff_handoff: None,
            todo_origin: None,
            attached_docs: Vec::new(),
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

    /// Validate a candidate reference document and, on success, add it to
    /// [`Self::attached_docs`], returning its basename for the confirmation
    /// message. Rejections carry the reason.
    pub fn attach_doc(
        &mut self,
        path: &std::path::Path,
    ) -> Result<String, crate::plan_interview::AttachError> {
        let canonical = crate::plan_interview::validate_attachment(path, &self.attached_docs)?;
        let label = canonical
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| canonical.to_string_lossy().into_owned());
        self.attached_docs.push(canonical);
        Ok(label)
    }

    /// Drop the most recently attached reference document, returning its
    /// basename if there was one to drop.
    pub fn remove_last_attached_doc(&mut self) -> Option<String> {
        let path = self.attached_docs.pop()?;
        Some(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned()),
        )
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
        // Restore the attached reference documents verbatim. The paths are only
        // read at headless-dispatch time, where `prepare_attached_docs` drops
        // and reports any that have since moved — re-checking here would just
        // duplicate that, and a still-listed missing file is itself a useful
        // signal on the brief step.
        self.attached_docs = draft
            .attached_docs
            .iter()
            .map(std::path::PathBuf::from)
            .collect();

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
    /// the question no longer offers. That part of the answer is dropped rather
    /// than pre-filled, because it is unselectable in the UI and would otherwise
    /// reach the AI rounds and synthesis attached to the current question text.
    /// A choice answer's custom-text half survives an option rewrite; only the
    /// selection is re-validated.
    fn adopt_recorded_answers(&mut self, record: &PlanInterviewRecord) {
        // Carried AI questions are appended first so the structured pass below
        // covers them too — the record still holds their options, answer, and
        // custom text under the same id.
        let known: HashSet<&str> = self.questions.iter().map(|q| q.id.as_str()).collect();
        let carried: Vec<PlanQuestion> = record
            .questions
            .iter()
            .filter(|question| {
                matches!(question.source, QuestionSource::Ai { .. })
                    && !known.contains(question.id.as_str())
            })
            .cloned()
            .collect();
        self.questions.extend(carried);

        let adopted: Vec<(Option<String>, String)> = self
            .questions
            .iter()
            .map(|question| {
                let Some(raw) = record.answer_for(&question.id) else {
                    return (None, String::new());
                };
                match &question.kind {
                    PlanQuestionKind::FreeText => (Some(raw.to_string()), String::new()),
                    PlanQuestionKind::Select(options) => {
                        let stored_custom = record.custom_answer_for(&question.id);
                        let (indices, custom) = split_choice_answer(raw, stored_custom, options);
                        let labels: Vec<&str> = indices
                            .iter()
                            .filter_map(|&index| options.get(index))
                            .map(String::as_str)
                            .collect();
                        (serialize_choice_answer(&labels, &custom), custom)
                    }
                }
            })
            .collect();
        self.answers = adopted.iter().map(|(answer, _)| answer.clone()).collect();
        self.custom_answers = adopted.into_iter().map(|(_, custom)| custom).collect();
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
        // The custom-text half of a pre-filled choice answer, so Ctrl+R can put
        // back the elaboration as well as the selection.
        self.prior_custom_answers = self
            .questions
            .iter()
            .zip(self.custom_answers.iter())
            .filter(|(_, custom)| !custom.trim().is_empty())
            .map(|(question, custom)| (question.id.clone(), custom.clone()))
            .collect();

        self.brief = self.prior_brief.clone().unwrap_or_default();
        self.editor = TextEditor::new(self.brief.clone());
        // Carry the previously attached reference documents as this run's
        // starting set, the same way the brief and answers are pre-filled; the
        // user can drop any on the brief step before the first pass.
        self.attached_docs = record
            .attached_docs
            .iter()
            .map(std::path::PathBuf::from)
            .collect();
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
        self.custom_answers = self
            .questions
            .iter()
            .map(|question| {
                self.prior_custom_answers
                    .get(&question.id)
                    .cloned()
                    .unwrap_or_default()
            })
            .collect();
        self.question_index = 0;
        self.selected_option = None;
        self.custom_answer_focused = false;
        self.custom_answer_backup = None;
        self.phase = PlanInterviewPhase::Brief;
        self.editor = TextEditor::new(self.brief.clone());
        true
    }

    /// How the step on screen compares with the previously accepted answer for
    /// it, or `None` when there is no previous answer to compare against.
    pub fn prior_answer_state(&self) -> Option<PriorAnswerState> {
        let (prior, current): (String, String) = match self.phase {
            PlanInterviewPhase::Brief => (
                self.prior_brief.as_deref()?.to_string(),
                self.editor.text().to_string(),
            ),
            PlanInterviewPhase::StaticQuestions => {
                let question = self.questions.get(self.question_index)?;
                let prior = self.prior_answers.get(&question.id)?.clone();
                match &question.kind {
                    PlanQuestionKind::FreeText => (prior, self.editor.text().to_string()),
                    // The whole choice answer — selection plus custom text —
                    // compared as the one serialized string it is stored as, so
                    // adding an elaboration to a kept option reads as "changed".
                    PlanQuestionKind::Select(options) => {
                        let labels: Vec<&str> = self
                            .selected_option
                            .and_then(|index| options.get(index))
                            .map(String::as_str)
                            .into_iter()
                            .collect();
                        let current = serialize_choice_answer(&labels, self.editor.text())
                            .unwrap_or_default();
                        (prior, current)
                    }
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
                    // Restore both halves the previous interview accepted: the
                    // radio selection and the custom-text elaboration. Adoption
                    // keeps only a selection the question still offers, so an
                    // answer that survives as neither is reported as "nothing
                    // restored" rather than moving the highlight to option 0.
                    PlanQuestionKind::Select(options) => {
                        let prior_custom = self.prior_custom_answers.get(&id).map(String::as_str);
                        let (indices, custom) = split_choice_answer(&prior, prior_custom, options);
                        if indices.is_empty() && custom.trim().is_empty() {
                            return false;
                        }
                        self.selected_option = indices.first().copied();
                        self.editor = TextEditor::new(custom);
                        self.custom_answer_focused = false;
                        self.custom_answer_backup = None;
                        true
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
            // The custom-text half of every choice answer, so a resumed or
            // re-run interview can restore the selection and the elaboration
            // together. Blank entries persist as `None`.
            custom_answers: self
                .custom_answers
                .iter()
                .map(|custom| (!custom.trim().is_empty()).then(|| custom.clone()))
                .collect(),
            // A draft holds the plan only once one has been generated, so
            // resuming after synthesis does not silently re-spend those tokens.
            plan: self.synthesized_plan.clone(),
            ai_rounds_completed: self.ai_rounds_completed,
            attached_docs: self
                .attached_docs
                .iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    /// The round cap `continue_plan_interview_after_done` checks
    /// `ai_rounds_completed` against — [`crate::plan_interview::MAX_QUICK_AI_ROUNDS`]
    /// while `kind` is `Quick`, [`crate::plan_interview::MAX_AI_ROUNDS`] once
    /// escalated (or for a `Full` interview from the start).
    pub fn max_ai_rounds(&self) -> usize {
        match self.kind {
            PlanInterviewMode::Quick => crate::plan_interview::MAX_QUICK_AI_ROUNDS,
            PlanInterviewMode::Full => crate::plan_interview::MAX_AI_ROUNDS,
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
        self.custom_answers
            .extend(new_questions.iter().map(|_| String::new()));
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

    fn current_option_count(&self) -> usize {
        self.current_question()
            .and_then(|question| match &question.kind {
                PlanQuestionKind::Select(options) => Some(options.len()),
                PlanQuestionKind::FreeText => None,
            })
            .unwrap_or(0)
    }

    /// Move the option highlight up, wrapping. From "nothing picked" this lands
    /// on the last option — the arrows always settle on a real choice; leaving
    /// the options untouched is how a user answers with custom text alone.
    pub fn select_previous_option(&mut self) {
        let option_count = self.current_option_count();
        if option_count == 0 {
            return;
        }
        self.selected_option = Some(match self.selected_option {
            None | Some(0) => option_count - 1,
            Some(index) => index - 1,
        });
    }

    pub fn select_next_option(&mut self) {
        let option_count = self.current_option_count();
        if option_count == 0 {
            return;
        }
        self.selected_option = Some(match self.selected_option {
            None => 0,
            Some(index) => (index + 1) % option_count,
        });
    }

    /// Return a choice question to the "nothing picked" state. Because the arrow
    /// keys only ever move between real options, this is the sole way back once
    /// a pick has been made — and "nothing picked" is a real answer: it is how a
    /// user submits with custom text alone. Returns `false` when the current
    /// question is not a choice or nothing was picked.
    pub fn clear_option_selection(&mut self) -> bool {
        if self.current_option_count() == 0 || self.selected_option.is_none() {
            return false;
        }
        self.selected_option = None;
        true
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
                self.selected_option = None;
                self.custom_answer_focused = false;
                self.custom_answer_backup = None;
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
            // A choice answer is the picked option label(s) and the trimmed
            // custom text, combined into one plain string. Nothing picked and
            // blank custom text records as no answer — which the gate below
            // blocks for a required question, exactly as for free text.
            PlanQuestionKind::Select(options) => {
                let custom = self.editor.text().trim().to_string();
                if let Some(slot) = self.custom_answers.get_mut(self.question_index) {
                    *slot = custom.clone();
                }
                let labels: Vec<&str> = self
                    .selected_option
                    .and_then(|index| options.get(index))
                    .map(String::as_str)
                    .into_iter()
                    .collect();
                serialize_choice_answer(&labels, &custom)
            }
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

    /// Load the answer stored for the question now on screen into the transient
    /// `selected_option` / `editor` scratch. For a choice question this rebuilds
    /// the structured control from the serialized string and the stored custom
    /// text, so revisiting an answered question shows the radio selection and
    /// the custom-text box — never a flat editable string.
    fn load_current_answer(&mut self) {
        let existing = self
            .answers
            .get(self.question_index)
            .and_then(|answer| answer.clone());
        let stored_custom = self
            .custom_answers
            .get(self.question_index)
            .cloned()
            .unwrap_or_default();
        self.custom_answer_focused = false;
        self.custom_answer_backup = None;
        match self.questions.get(self.question_index).map(|q| &q.kind) {
            Some(PlanQuestionKind::FreeText) => {
                self.editor = TextEditor::new(existing.unwrap_or_default());
                self.selected_option = None;
            }
            Some(PlanQuestionKind::Select(options)) => {
                let stored = (!stored_custom.is_empty()).then_some(stored_custom.as_str());
                let (indices, custom) = match existing.as_deref() {
                    Some(combined) => split_choice_answer(combined, stored, options),
                    None => (Vec::new(), stored_custom.clone()),
                };
                self.selected_option = indices.first().copied();
                self.editor = TextEditor::new(custom);
            }
            None => {}
        }
    }

    /// Open the inline custom-answer editor for the choice question on screen.
    /// The current buffer is stashed so `Esc` can restore it; `false` when the
    /// phase or question is wrong or the editor is already focused.
    pub fn open_custom_answer_editor(&mut self) -> bool {
        if self.phase != PlanInterviewPhase::StaticQuestions || self.custom_answer_focused {
            return false;
        }
        if !matches!(
            self.current_question().map(|q| &q.kind),
            Some(PlanQuestionKind::Select(_))
        ) {
            return false;
        }
        self.custom_answer_backup = Some(self.editor.text().to_string());
        self.custom_answer_focused = true;
        true
    }

    /// Commit the custom-answer edit: trim the buffer, record it, and return
    /// focus to the option list without submitting the question. The serialized
    /// answer is refreshed too, so a draft persisted right after a commit
    /// round-trips even though the question has not been advanced through.
    pub fn commit_custom_answer(&mut self) {
        if !self.custom_answer_focused {
            return;
        }
        let trimmed = self.editor.text().trim().to_string();
        self.editor = TextEditor::new(trimmed.clone());
        if let Some(slot) = self.custom_answers.get_mut(self.question_index) {
            *slot = trimmed.clone();
        }
        let recorded = match self.questions.get(self.question_index).map(|q| &q.kind) {
            Some(PlanQuestionKind::Select(options)) => {
                let labels: Vec<&str> = self
                    .selected_option
                    .and_then(|index| options.get(index))
                    .map(String::as_str)
                    .into_iter()
                    .collect();
                Some(serialize_choice_answer(&labels, &trimmed))
            }
            _ => None,
        };
        if let Some(answer) = recorded
            && let Some(slot) = self.answers.get_mut(self.question_index)
        {
            *slot = answer;
        }
        self.custom_answer_focused = false;
        self.custom_answer_backup = None;
    }

    /// Abandon the custom-answer edit, restoring the buffer captured when it was
    /// opened.
    pub fn cancel_custom_answer(&mut self) {
        if !self.custom_answer_focused {
            return;
        }
        let restore = self.custom_answer_backup.take().unwrap_or_default();
        self.editor = TextEditor::new(restore);
        self.custom_answer_focused = false;
    }

    /// Forward a key to the focused custom-answer editor, enforcing the
    /// character cap by reverting any edit that would exceed it (a paste, or a
    /// keystroke at the limit).
    pub fn custom_answer_handle_key(&mut self, key: crossterm::event::KeyEvent) {
        if !self.custom_answer_focused {
            return;
        }
        let before = self.editor.text().to_string();
        self.editor.handle_key(key);
        if self.editor.text().chars().count() > CUSTOM_ANSWER_MAX_LEN {
            self.editor = TextEditor::new(before);
        }
    }

    /// Whether the question on screen is a choice question (so the custom-answer
    /// box is shown and `e` is bound).
    pub fn current_question_is_choice(&self) -> bool {
        matches!(
            self.current_question().map(|q| &q.kind),
            Some(PlanQuestionKind::Select(_))
        )
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
            project_name: project_name.into(),
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
