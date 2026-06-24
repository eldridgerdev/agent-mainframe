use ratatui_explorer::FileExplorer;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Child;
use std::time::Instant;

use super::PromptAnalysis;
use crate::editor::TextEditor;
use crate::extension::{CustomSessionConfig, FeaturePreset, LifecycleHooks};
use crate::project::{AgentKind, SessionKind, VibeMode};
use crate::worktree::WorktreeInfo;

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
}

impl ViewState {
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
        }
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
    Custom(CustomSessionConfig),
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

/// Per-file verdict in a final review. Absence of an entry means the file
/// was skipped (neither approved nor rejected).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewDecision {
    Approve,
    Reject { feedback: String },
}

#[derive(Clone)]
pub struct DiffViewerState {
    pub from_view: ViewState,
    pub workdir: PathBuf,
    pub branch: String,
    pub base_ref: String,
    pub base_commit: String,
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
    /// When true the developer-notes panel takes the full patch column.
    pub notes_expanded: bool,
    pub notes_scroll: usize,
}

impl DiffViewerState {
    pub fn new(from_view: ViewState, workdir: PathBuf) -> Self {
        Self {
            from_view,
            workdir,
            branch: String::new(),
            base_ref: String::new(),
            base_commit: String::new(),
            files: Vec::new(),
            selected_file: 0,
            patch_scroll: 0,
            focus: DiffViewerFocus::FileList,
            layout: DiffViewerLayout::Unified,
            error: None,
            review: false,
            decisions: std::collections::HashMap::new(),
            feedback_editing: false,
            editing_general: false,
            feedback_editor: TextEditor::new(String::new()),
            feedback_scroll: 0,
            feedback_sync_to_cursor: true,
            general_feedback: String::new(),
            review_notes: std::collections::HashMap::new(),
            notes_expanded: false,
            notes_scroll: 0,
        }
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

/// State for the full-screen PR comment-review pane.
#[derive(Debug, Clone)]
pub struct PrReviewState {
    /// Working directory of the feature whose PR we're reviewing. Retained for
    /// the manual-refresh action (next Epic A item), which re-fetches from here.
    #[allow(dead_code)]
    pub workdir: PathBuf,
    /// The fetched, normalized review.
    pub review: crate::app::pr_review::PrReview,
    /// Index into `review.comments` of the highlighted comment.
    pub selected: usize,
    /// Scroll offset (in lines) for the detail pane of the selected comment.
    pub detail_scroll: usize,
    /// When true, comments already resolved on GitHub are hidden from the list.
    pub hide_resolved: bool,
}

impl PrReviewState {
    pub fn selected_comment(&self) -> Option<&crate::app::pr_review::PrComment> {
        self.review.comments.get(self.selected)
    }

    /// Indices into `review.comments` that pass the current filter, in order.
    /// With `hide_resolved` on, GitHub-resolved comments are dropped.
    pub fn visible_indices(&self) -> Vec<usize> {
        self.review
            .comments
            .iter()
            .enumerate()
            .filter(|(_, c)| !self.hide_resolved || !c.is_resolved)
            .map(|(i, _)| i)
            .collect()
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

pub enum AppMode {
    Normal,
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
