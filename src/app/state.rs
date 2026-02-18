use ratatui_explorer::FileExplorer;
use std::path::PathBuf;

use crate::project::{AgentKind, VibeMode};
use crate::worktree::WorktreeInfo;

#[derive(Debug, Clone)]
pub enum Selection {
    Project(usize),
    Feature(usize, usize),
    Session(usize, usize, usize),
}

pub struct ViewState {
    pub project_name: String,
    pub feature_name: String,
    pub session: String,
    pub window: String,
    pub session_label: String,
    pub vibe_mode: VibeMode,
    pub scroll_offset: usize,
    pub scroll_content: String,
    pub scroll_mode: bool,
    pub scroll_total_lines: usize,
}

impl ViewState {
    pub fn new(
        project_name: String,
        feature_name: String,
        session: String,
        window: String,
        session_label: String,
        vibe_mode: VibeMode,
    ) -> Self {
        Self {
            project_name,
            feature_name,
            session,
            window,
            session_label,
            vibe_mode,
            scroll_offset: 0,
            scroll_content: String::new(),
            scroll_mode: false,
            scroll_total_lines: 0,
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

pub enum AppMode {
    Normal,
    CreatingProject(CreateProjectState),
    CreatingFeature(CreateFeatureState),
    DeletingProject(String),
    DeletingFeature(String, String),
    Viewing(ViewState),
    Help,
    NotificationPicker(usize),
    SessionSwitcher(super::SessionSwitcherState),
    RenamingSession(RenameSessionState),
    BrowsingPath(Box<BrowsePathState>),
    CommandPicker(super::CommandPickerState),
    Searching(SearchState),
    OpencodeSessionPicker(OpencodeSessionPickerState),
    ConfirmingOpencodeSession {
        session_id: String,
        workdir: PathBuf,
    },
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
    Branch,
    Worktree,
    Mode,
    ConfirmSuperVibe,
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
    pub source_index: usize,
    pub worktrees: Vec<WorktreeInfo>,
    pub worktree_index: usize,
    pub use_worktree: bool,
    pub enable_chrome: bool,
    pub enable_notes: bool,
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
            source_index: 0,
            worktrees,
            worktree_index: 0,
            use_worktree: !is_first_feature,
            enable_chrome: false,
            enable_notes: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub enum SessionFilter {
    #[default]
    All,
    Claude,
    Opencode,
    Terminal,
    Nvim,
    Memo,
}

impl SessionFilter {
    pub const ALL: [SessionFilter; 6] = [
        SessionFilter::All,
        SessionFilter::Claude,
        SessionFilter::Opencode,
        SessionFilter::Terminal,
        SessionFilter::Nvim,
        SessionFilter::Memo,
    ];

    pub fn display_name(&self) -> &str {
        match self {
            SessionFilter::All => "all",
            SessionFilter::Claude => "claude",
            SessionFilter::Opencode => "opencode",
            SessionFilter::Terminal => "terminal",
            SessionFilter::Nvim => "nvim",
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
