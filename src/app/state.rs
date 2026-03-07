use ratatui_explorer::FileExplorer;
use std::path::PathBuf;
use std::process::Child;

use crate::extension::CustomSessionConfig;
use crate::project::{AgentKind, VibeMode};
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
    pub enable_notes: bool,
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
    pub fn clear(&mut self) {
        self.has_selection = false;
        self.is_selecting = false;
    }

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
    pub vibe_mode: VibeMode,
    pub review: bool,
    pub scroll_offset: usize,
    pub scroll_content: String,
    pub scroll_mode: bool,
    pub scroll_total_lines: usize,
    pub scroll_passthrough: bool,
    pub selection: TextSelection,
}

impl ViewState {
    pub fn new(
        project_name: String,
        feature_name: String,
        session: String,
        window: String,
        session_label: String,
        vibe_mode: VibeMode,
        review: bool,
    ) -> Self {
        Self {
            project_name,
            feature_name,
            session,
            window,
            session_label,
            vibe_mode,
            review,
            scroll_offset: 0,
            scroll_content: String::new(),
            scroll_mode: false,
            scroll_total_lines: 0,
            scroll_passthrough: false,
            selection: TextSelection::default(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingInput {
    pub session_id: String,
    pub cwd: String,
    pub message: String,
    pub notification_type: String,
    pub file_path: PathBuf,
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

#[derive(Debug, Clone)]
pub struct OpencodeSessionInfo {
    pub id: String,
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

#[derive(Clone)]
pub struct BookmarkPickerState {
    pub selected: usize,
    pub from_view: Option<ViewState>,
}

pub enum AppMode {
    Normal,
    CreatingProject(CreateProjectState),
    CreatingFeature(CreateFeatureState),
    DeletingProject(String),
    DeletingFeature(String, String),
    DeletingFeatureInProgress(DeletingFeatureState),
    Viewing(ViewState),
    Help(Option<ViewState>),
    NotificationPicker(usize, Option<ViewState>),
    SessionSwitcher(super::SessionSwitcherState),
    RenamingSession(RenameSessionState),
    RenamingFeature(RenameFeatureState),
    SessionConfig(SessionConfigState),
    BrowsingPath(Box<BrowsePathState>),
    CommandPicker(super::CommandPickerState),
    Searching(SearchState),
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
    BookmarkPicker(BookmarkPickerState),
    SessionPicker(SessionPickerState),
    ChangeReasonPrompt(ChangeReasonState),
    RunningHook(RunningHookState),
    HookPrompt(HookPromptState),
    LatestPrompt(String, ViewState),
    ForkingFeature(ForkFeatureState),
    ThemePicker(ThemePickerState),
    DebugLog(DebugLogState),
    CreatingBatchFeatures(CreateBatchFeaturesState),
}

#[derive(Debug, Clone)]
pub struct PendingSummary {
    pub tmux_session: String,
    pub workdir: PathBuf,
    pub agent: crate::project::AgentKind,
}

#[derive(Debug, Clone, Default)]
pub struct SummaryState {
    pub pending: Vec<PendingSummary>,
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
}

pub struct DebugLogState {
    pub scroll_offset: usize,
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

pub struct ChangeReasonState {
    pub session_id: String,
    pub file_path: String,
    pub relative_path: String,
    pub change_id: String,
    pub tool: String,
    pub old_snippet: String,
    pub new_snippet: String,
    pub reason: String,
    pub response_file: PathBuf,
    pub proceed_signal: PathBuf,
    pub request_id: Option<String>,
    pub reply_socket: Option<String>,
}

pub enum HookNext {
    WorktreeCreated {
        project_name: String,
        branch: String,
        mode: VibeMode,
        review: bool,
        agent: AgentKind,
        enable_chrome: bool,
        enable_notes: bool,
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
    pub agent: AgentKind,
    pub enable_chrome: bool,
    pub enable_notes: bool,
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
    pub error: Option<String>,
}

impl BackgroundDeletion {
    pub fn key(&self) -> String {
        format!("{}/{}", self.project_name, self.feature_name)
    }

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
            error: state.error,
        }
    }
}

pub struct BackgroundHook {
    pub script: String,
    pub workdir: PathBuf,
    pub project_name: String,
    pub branch: String,
    pub mode: VibeMode,
    pub review: bool,
    pub agent: AgentKind,
    pub enable_chrome: bool,
    pub enable_notes: bool,
    pub child: Option<Child>,
    pub output: String,
    pub success: Option<bool>,
    pub output_rx: Option<std::sync::mpsc::Receiver<String>>,
}

impl BackgroundHook {
    pub fn key(&self) -> String {
        format!("{}/{}", self.workdir.display(), self.script)
    }

    pub fn from_running_state(state: RunningHookState) -> Self {
        Self {
            script: state.script,
            workdir: state.workdir,
            project_name: state.project_name,
            branch: state.branch,
            mode: state.mode,
            review: state.review,
            agent: state.agent,
            enable_chrome: state.enable_chrome,
            enable_notes: state.enable_notes,
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
}

#[derive(Clone, PartialEq)]
pub enum CreateProjectStep {
    Name,
    Path,
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
    pub step: CreateFeatureStep,
    pub agent: AgentKind,
    pub agent_index: usize,
    pub mode: VibeMode,
    pub mode_index: usize,
    pub mode_focus: usize,
    pub review: bool,
    pub source_index: usize,
    pub worktrees: Vec<WorktreeInfo>,
    pub worktree_index: usize,
    pub use_worktree: bool,
    pub enable_chrome: bool,
    pub enable_notes: bool,
    pub preset_index: usize,
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
            step,
            agent: AgentKind::default(),
            agent_index: 0,
            mode: VibeMode::default(),
            mode_index: 0,
            mode_focus: 0,
            review: false,
            source_index: 0,
            worktrees,
            worktree_index: 0,
            use_worktree: !is_first_feature,
            enable_chrome: false,
            enable_notes: false,
            preset_index: 0,
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
    pub enable_notes: bool,
    pub step: CreateBatchFeaturesStep,
}

impl CreateBatchFeaturesState {
    pub fn new() -> Self {
        Self::with_workspace(None)
    }

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
            enable_notes: false,
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
    Memo,
}

impl SessionFilter {
    pub const ALL: [SessionFilter; 8] = [
        SessionFilter::All,
        SessionFilter::Claude,
        SessionFilter::Opencode,
        SessionFilter::Codex,
        SessionFilter::Terminal,
        SessionFilter::Nvim,
        SessionFilter::Vscode,
        SessionFilter::Memo,
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
            SessionFilter::Memo => "memo",
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
    fn session_filter_all_has_eight_variants() {
        assert_eq!(SessionFilter::ALL.len(), 8);
    }
}
