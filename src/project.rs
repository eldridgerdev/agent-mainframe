use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::token_tracking::{SessionTokenUsage, TokenUsageSource};

pub(crate) const CURRENT_PROJECT_STORE_VERSION: u32 = 5;

fn slugify_component(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

pub fn normalized_feature_name(name: &str) -> String {
    slugify_component(name)
}

pub fn tmux_session_name(project_name: &str, feature_name: &str) -> String {
    let project = slugify_component(project_name);
    let feature = slugify_component(feature_name);

    match (project.is_empty(), feature.is_empty()) {
        (false, false) => format!("amf-{project}-{feature}"),
        (false, true) => format!("amf-{project}"),
        (true, false) => format!("amf-{feature}"),
        (true, true) => "amf-feature".to_string(),
    }
}

pub fn worktree_name(project_name: &str, feature_name: &str) -> String {
    let project = slugify_component(project_name);
    let feature = slugify_component(feature_name);

    match (project.is_empty(), feature.is_empty()) {
        (false, false) => format!("{project}-{feature}"),
        (false, true) => project,
        (true, false) => feature,
        (true, true) => "feature".to_string(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ProjectStatus {
    Active,
    Idle,
    Stopped,
}

impl std::fmt::Display for ProjectStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProjectStatus::Active => write!(f, "active"),
            ProjectStatus::Idle => write!(f, "idle"),
            ProjectStatus::Stopped => write!(f, "stopped"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SessionKind {
    Claude,
    Opencode,
    Codex,
    Pi,
    Terminal,
    Nvim,
    Vscode,
    Custom,
    /// A per-project TODO list. Native AMF UI, not tmux-backed; at most one
    /// per project. See `docs/backlog/feature-todos-plan.md`.
    Todos,
}

impl SessionKind {
    /// Whether this window hosts one of AMF's built-in agent harnesses.
    pub fn is_agent_harness(&self) -> bool {
        matches!(self, Self::Claude | Self::Opencode | Self::Codex | Self::Pi)
    }

    /// Whether opening this session attaches to a tmux pane. `Todos` is a
    /// native overlay with no tmux window, so this is `false` for it.
    pub fn is_tmux_backed(&self) -> bool {
        !matches!(self, Self::Todos)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Claude,
    Opencode,
    Codex,
    Pi,
}

impl AgentKind {
    pub fn display_name(&self) -> &str {
        match self {
            AgentKind::Claude => "Claude",
            AgentKind::Opencode => "Opencode",
            AgentKind::Codex => "Codex",
            AgentKind::Pi => "Pi",
        }
    }

    pub const ALL: [AgentKind; 4] = [
        AgentKind::Claude,
        AgentKind::Opencode,
        AgentKind::Codex,
        AgentKind::Pi,
    ];

    pub fn allowed_list(configured: Option<&[AgentKind]>) -> Vec<AgentKind> {
        Self::ALL
            .iter()
            .filter(|agent| {
                configured.is_none_or(|allowed| allowed.is_empty() || allowed.contains(agent))
            })
            .cloned()
            .collect()
    }

    pub fn index_in(agents: &[AgentKind], target: &AgentKind) -> usize {
        agents.iter().position(|agent| agent == target).unwrap_or(0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureSession {
    pub id: String,
    pub kind: SessionKind,
    pub label: String,
    pub tmux_window: String,
    pub claude_session_id: Option<String>,
    /// The TODO that initiated this agent session, when it was launched from
    /// the TODO menu.  The identity is stable across TODO list moves; the
    /// current TODO data is always resolved from SQLite rather than copied.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub todo_reference: Option<TodoSessionReference>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage_source: Option<TokenUsageSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_usage_source_match: Option<TokenUsageSourceMatch>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_stop: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_check: Option<String>,
    #[serde(skip)]
    pub status_text: Option<String>,
    #[serde(skip)]
    pub token_usage: Option<SessionTokenUsage>,
}

/// Provenance retained on an agent session started through the TODO menu.
///
/// `launched_from_todo_menu` is deliberately stored alongside the TODO id so
/// legacy TODO/session associations cannot be mistaken for this feature's
/// explicit sidebar reference.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TodoSessionReference {
    pub todo_id: String,
    pub launched_from_todo_menu: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TokenUsageSourceMatch {
    Exact,
    Inferred,
}

impl FeatureSession {
    pub fn set_token_usage_source_exact(&mut self, source: TokenUsageSource) {
        self.token_usage_source = Some(source);
        self.token_usage_source_match = Some(TokenUsageSourceMatch::Exact);
    }

    pub fn set_token_usage_source_inferred(&mut self, source: TokenUsageSource) {
        self.token_usage_source = Some(source);
        self.token_usage_source_match = Some(TokenUsageSourceMatch::Inferred);
    }

    pub fn clear_token_usage_source(&mut self) {
        self.token_usage_source = None;
        self.token_usage_source_match = None;
        self.token_usage = None;
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum VibeMode {
    #[default]
    Vibeless,
    Vibe,
    SuperVibe,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum StoredVibeMode {
    #[default]
    Vibeless,
    Vibe,
    SuperVibe,
    Review,
}

impl StoredVibeMode {
    pub(crate) fn into_mode_and_review(self) -> (VibeMode, bool) {
        match self {
            StoredVibeMode::Vibeless => (VibeMode::Vibeless, false),
            StoredVibeMode::Vibe => (VibeMode::Vibe, false),
            StoredVibeMode::SuperVibe => (VibeMode::SuperVibe, false),
            StoredVibeMode::Review => (VibeMode::Vibeless, true),
        }
    }
}

impl<'de> Deserialize<'de> for VibeMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Ok(StoredVibeMode::deserialize(deserializer)?
            .into_mode_and_review()
            .0)
    }
}

impl VibeMode {
    pub fn display_name(&self) -> &str {
        match self {
            VibeMode::Vibeless => "Vibeless",
            VibeMode::Vibe => "Vibe",
            VibeMode::SuperVibe => "SuperVibe",
        }
    }

    pub fn description(&self) -> &str {
        match self {
            VibeMode::Vibeless => "asks for approval for every change",
            VibeMode::Vibe => "auto-accepts edits",
            VibeMode::SuperVibe => "skips all permission prompts",
        }
    }

    pub fn cli_flags(&self, opts: LaunchOpts) -> Vec<String> {
        let mut flags = match self {
            VibeMode::Vibeless => vec![],
            VibeMode::Vibe => {
                vec!["--permission-mode".into(), "acceptEdits".into()]
            }
            VibeMode::SuperVibe => {
                vec!["--dangerously-skip-permissions".into()]
            }
        };
        if opts.enable_chrome {
            flags.push("--chrome".into());
        }
        // Remote Control requires claude.ai OAuth and is not compatible
        // with z.ai / third-party provider sessions. The z.ai guard is
        // applied at the call site before constructing LaunchOpts.
        //
        // TODO(remote-control): also gate on API-key / Bedrock / Vertex /
        // Foundry auth (plan section 3.8). For now only the z.ai case is
        // blocked at the call site.
        if opts.remote_control {
            flags.push("--remote-control".into());
            if let Some(name) = opts.session_name
                && !name.is_empty()
            {
                flags.push(name);
            }
        }
        flags
    }

    pub const ALL: [VibeMode; 3] = [VibeMode::Vibeless, VibeMode::Vibe, VibeMode::SuperVibe];
}

fn default_true() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Options passed to `VibeMode::cli_flags` when building the
/// argument list for a Claude session launch.
#[derive(Debug, Clone, Default)]
pub struct LaunchOpts {
    pub enable_chrome: bool,
    pub remote_control: bool,
    /// Human-readable session name passed to `--remote-control`.
    /// Ignored when `remote_control` is false.
    pub session_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Feature {
    pub id: String,
    pub name: String,
    pub branch: String,
    pub workdir: PathBuf,
    pub is_worktree: bool,
    pub tmux_session: String,
    #[serde(default)]
    pub sessions: Vec<FeatureSession>,
    #[serde(default = "default_true")]
    pub collapsed: bool,
    #[serde(default)]
    pub mode: VibeMode,
    #[serde(default)]
    pub review: bool,
    #[serde(default)]
    pub plan_mode: bool,
    #[serde(default)]
    pub agent: AgentKind,
    #[serde(default)]
    pub enable_chrome: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub remote_control: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pending_worktree_script: bool,
    #[serde(default)]
    pub ready: bool,
    pub status: ProjectStatus,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    /// User-selected plan file for this feature. The effective-plan resolver
    /// may prefer the worktree's conventional `AMF_PLAN.md` over this value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_plan_path: Option<PathBuf>,
    /// Set only on a **companion triage feature**: the isolated worktree PR
    /// Triage creates when the user picks the `New feature…` fix target. Git
    /// can't check out the PR's branch in two worktrees at once, so the
    /// companion sits on its own branch and this link — not branch-based PR
    /// auto-detection — is what ties it back to the PR and the feature the
    /// triage was started from. `None` for every ordinary feature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage_source: Option<TriageSource>,
    /// Set only on a **companion review feature**: the isolated worktree the
    /// final review creates when the reviewer picks the "New feature…"
    /// destination. Like `triage_source` this ties the companion back to the
    /// feature the review was run from and records the commit it was branched
    /// from (the base of the integration commit range). `None` for every
    /// ordinary feature and for PR-triage companions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_source: Option<ReviewSource>,
}

/// The PR and source feature a companion triage feature was created for. Also
/// records the commit the companion was branched from, which is the base of
/// the commit range offered to the integration flow (`I` in PR Triage).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriageSource {
    /// The PR whose review comments this feature was created to fix.
    pub pr_number: u32,
    /// `Feature::id` of the feature PR Triage was opened from.
    pub source_feature_id: String,
    /// The PR's own head branch — the ref integration pushes onto. Recorded
    /// from `PrRef::head_ref` rather than the source feature's checked-out
    /// branch: the two diverge whenever a PR is triaged from a feature sitting
    /// on some other branch, and pushing to the wrong one would land review
    /// fixes on a branch nobody asked about.
    #[serde(alias = "source_branch")]
    pub pr_branch: String,
    /// Commit the companion worktree was branched from. Everything after it on
    /// the triage branch is what integration pushes or cherry-picks back.
    pub base_sha: String,
}

/// The source feature a **companion review feature** was created from (the
/// final review's "New feature…" destination). Kept separate from
/// [`TriageSource`] because there is no PR involved: integration lands the
/// companion's commits on the source feature's own branch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewSource {
    /// `Feature::id` of the feature the final review was run from.
    pub source_feature_id: String,
    /// The source feature's branch — the ref integration pushes onto or
    /// cherry-picks into.
    pub target_branch: String,
    /// Commit the companion worktree was branched from. Everything after it on
    /// the companion branch is what integration pushes or cherry-picks back.
    pub base_sha: String,
}

#[derive(Deserialize)]
struct FeatureDe {
    id: String,
    name: String,
    branch: String,
    workdir: PathBuf,
    is_worktree: bool,
    tmux_session: String,
    #[serde(default)]
    sessions: Vec<FeatureSession>,
    #[serde(default = "default_true")]
    collapsed: bool,
    #[serde(default)]
    mode: StoredVibeMode,
    #[serde(default)]
    review: bool,
    #[serde(default)]
    plan_mode: bool,
    #[serde(default)]
    agent: AgentKind,
    #[serde(default)]
    enable_chrome: bool,
    #[serde(default)]
    remote_control: bool,
    #[serde(default)]
    ready: bool,
    status: ProjectStatus,
    created_at: DateTime<Utc>,
    last_accessed: DateTime<Utc>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    summary_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    nickname: Option<String>,
    #[serde(default)]
    selected_plan_path: Option<PathBuf>,
    #[serde(default)]
    triage_source: Option<TriageSource>,
    #[serde(default)]
    review_source: Option<ReviewSource>,
}

impl<'de> Deserialize<'de> for Feature {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let feature = FeatureDe::deserialize(deserializer)?;
        let (mode, legacy_review) = feature.mode.into_mode_and_review();
        Ok(Self {
            id: feature.id,
            name: feature.name,
            branch: feature.branch,
            workdir: feature.workdir,
            is_worktree: feature.is_worktree,
            tmux_session: feature.tmux_session,
            sessions: feature.sessions,
            collapsed: feature.collapsed,
            mode,
            review: feature.review || legacy_review,
            plan_mode: feature.plan_mode,
            agent: feature.agent,
            enable_chrome: feature.enable_chrome,
            remote_control: feature.remote_control,
            pending_worktree_script: false,
            ready: feature.ready,
            status: feature.status,
            created_at: feature.created_at,
            last_accessed: feature.last_accessed,
            summary: feature.summary,
            summary_updated_at: feature.summary_updated_at,
            nickname: feature.nickname,
            selected_plan_path: feature.selected_plan_path,
            triage_source: feature.triage_source,
            review_source: feature.review_source,
        })
    }
}

impl Feature {
    /// The feature's TODOs session, if it has one.
    ///
    /// One per **feature**, not one per project: each checkout has its own
    /// worktree list to open, and the editor reaches the project and global
    /// lists as side panes from there.
    pub fn todos_session(&self) -> Option<&FeatureSession> {
        self.sessions.iter().find(|s| s.kind == SessionKind::Todos)
    }

    /// Whether this feature already has a TODOs session.
    pub fn has_todos_session(&self) -> bool {
        self.todos_session().is_some()
    }

    #[allow(clippy::too_many_arguments)]
    #[allow(dead_code)] // exercised only by unit tests
    pub fn new(
        name: String,
        branch: String,
        workdir: PathBuf,
        is_worktree: bool,
        mode: VibeMode,
        review: bool,
        plan_mode: bool,
        agent: AgentKind,
        enable_chrome: bool,
        remote_control: bool,
    ) -> Self {
        let tmux_session = format!("amf-{}", name);
        Self::new_with_tmux_session(
            name,
            branch,
            workdir,
            is_worktree,
            mode,
            review,
            plan_mode,
            agent,
            enable_chrome,
            remote_control,
            tmux_session,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_for_project(
        project_name: &str,
        name: String,
        branch: String,
        workdir: PathBuf,
        is_worktree: bool,
        mode: VibeMode,
        review: bool,
        plan_mode: bool,
        agent: AgentKind,
        enable_chrome: bool,
        remote_control: bool,
    ) -> Self {
        let tmux_session = tmux_session_name(project_name, &name);
        Self::new_with_tmux_session(
            name,
            branch,
            workdir,
            is_worktree,
            mode,
            review,
            plan_mode,
            agent,
            enable_chrome,
            remote_control,
            tmux_session,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_tmux_session(
        name: String,
        branch: String,
        workdir: PathBuf,
        is_worktree: bool,
        mode: VibeMode,
        review: bool,
        plan_mode: bool,
        agent: AgentKind,
        enable_chrome: bool,
        remote_control: bool,
        tmux_session: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            branch,
            workdir,
            is_worktree,
            tmux_session,
            sessions: Vec::new(),
            collapsed: true,
            mode,
            review,
            plan_mode,
            agent,
            enable_chrome,
            remote_control,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Stopped,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
            selected_plan_path: None,
            triage_source: None,
            review_source: None,
        }
    }

    pub fn touch(&mut self) {
        self.last_accessed = Utc::now();
    }

    pub fn normalize_legacy_review_mode(&mut self) -> bool {
        false
    }

    /// Return the next label for a session of the given kind.
    pub fn next_label(&self, kind: &SessionKind) -> String {
        let count = self.sessions.iter().filter(|s| s.kind == *kind).count();
        match kind {
            SessionKind::Claude => format!("Claude {}", count + 1),
            SessionKind::Opencode => {
                format!("Opencode {}", count + 1)
            }
            SessionKind::Codex => {
                format!("Codex {}", count + 1)
            }
            SessionKind::Pi => {
                format!("Pi {}", count + 1)
            }
            SessionKind::Terminal => {
                format!("Terminal {}", count + 1)
            }
            SessionKind::Nvim => {
                format!("Nvim {}", count + 1)
            }
            SessionKind::Vscode => {
                format!("VSCode {}", count + 1)
            }
            SessionKind::Custom => {
                format!("Custom {}", count + 1)
            }
            SessionKind::Todos => "TODOs".to_string(),
        }
    }

    /// Return the next tmux window name for a session of the
    /// given kind, avoiding collisions with existing windows.
    pub fn next_window_name(&self, kind: &SessionKind) -> String {
        let prefix = match kind {
            SessionKind::Claude => "claude",
            SessionKind::Opencode => "opencode",
            SessionKind::Codex => "codex",
            SessionKind::Pi => "pi",
            SessionKind::Terminal => "terminal",
            SessionKind::Nvim => "nvim",
            SessionKind::Vscode => "vscode",
            SessionKind::Custom => "custom",
            SessionKind::Todos => "todos",
        };
        let count = self.sessions.iter().filter(|s| s.kind == *kind).count();
        if count == 0 {
            prefix.to_string()
        } else {
            let mut n = count + 1;
            loop {
                let candidate = format!("{}-{}", prefix, n);
                if !self.sessions.iter().any(|s| s.tmux_window == candidate) {
                    return candidate;
                }
                n += 1;
            }
        }
    }

    /// Create and append a new session of the given kind.
    pub fn add_session(&mut self, kind: SessionKind) -> &mut FeatureSession {
        let label = self.next_label(&kind);
        self.add_session_named(kind, label)
    }

    /// Create and append a new session of the given kind with a
    /// caller-provided label.
    pub fn add_session_named(&mut self, kind: SessionKind, label: String) -> &mut FeatureSession {
        let window = self.next_window_name(&kind);
        let session = FeatureSession {
            id: Uuid::new_v4().to_string(),
            kind,
            label,
            tmux_window: window,
            claude_session_id: None,
            todo_reference: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
        };
        self.sessions.push(session);
        self.sessions.last_mut().unwrap()
    }

    /// Create and append a custom session with a user-provided
    /// name, preferred window name, and optional command.
    /// Collision-avoids the window name against existing sessions.
    pub fn add_custom_session_named(
        &mut self,
        name: String,
        window_name_hint: String,
        command: Option<String>,
        on_stop: Option<String>,
        pre_check: Option<String>,
    ) -> &mut FeatureSession {
        let mut window = window_name_hint.clone();
        let mut n = 2u32;
        while self.sessions.iter().any(|s| s.tmux_window == window) {
            window = format!("{}-{}", window_name_hint, n);
            n += 1;
        }
        let session = FeatureSession {
            id: Uuid::new_v4().to_string(),
            kind: SessionKind::Custom,
            label: name,
            tmux_window: window,
            claude_session_id: None,
            todo_reference: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command,
            on_stop,
            pre_check,
            status_text: None,
            token_usage: None,
        };
        self.sessions.push(session);
        self.sessions.last_mut().unwrap()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub repo: PathBuf,
    pub collapsed: bool,
    pub features: Vec<Feature>,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub preferred_agent: AgentKind,
    #[serde(default)]
    pub is_git: bool,
}

impl Project {
    pub fn new(name: String, repo: PathBuf, is_git: bool, preferred_agent: AgentKind) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            repo,
            collapsed: false,
            features: Vec::new(),
            created_at: Utc::now(),
            preferred_agent,
            is_git,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionBookmark {
    pub project_id: String,
    pub feature_id: String,
    pub session_id: String,
}

fn default_session_bookmarks() -> Vec<SessionBookmark> {
    Vec::new()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectStore {
    pub version: u32,
    pub projects: Vec<Project>,
    #[serde(default = "default_session_bookmarks")]
    pub session_bookmarks: Vec<SessionBookmark>,
    #[serde(default)]
    pub available_harnesses: Vec<AgentKind>,
    #[serde(default)]
    pub prompt_templates: Vec<crate::prompt_library::PromptTemplate>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_json::Value>,
}

impl ProjectStore {
    /// A store with nothing in it, at the current version — what a reader gets
    /// when there is no database to read (`amf doctor` on a fresh machine).
    pub fn empty() -> Self {
        Self {
            version: CURRENT_PROJECT_STORE_VERSION,
            projects: Vec::new(),
            session_bookmarks: default_session_bookmarks(),
            available_harnesses: Vec::new(),
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        }
    }

    pub fn has_any_harnesses(&self) -> bool {
        !self.available_harnesses.is_empty()
    }

    pub fn merge_from(&mut self, other: ProjectStore) {
        self.version = self.version.max(other.version);

        for kind in other.available_harnesses {
            if !self.available_harnesses.contains(&kind) {
                self.available_harnesses.push(kind);
            }
        }

        for bookmark in other.session_bookmarks {
            if !self.session_bookmarks.iter().any(|existing| {
                existing.project_id == bookmark.project_id
                    && existing.feature_id == bookmark.feature_id
                    && existing.session_id == bookmark.session_id
            }) {
                self.session_bookmarks.push(bookmark);
            }
        }

        for template in other.prompt_templates {
            if !self
                .prompt_templates
                .iter()
                .any(|existing| existing.id == template.id)
            {
                self.prompt_templates.push(template);
            }
        }

        for (key, value) in other.extra {
            self.extra.insert(key, value);
        }

        for project in other.projects {
            if let Some(existing) = self.projects.iter_mut().find(|p| p.id == project.id) {
                merge_project(existing, project);
            } else {
                self.projects.push(project);
            }
        }
    }
}

fn merge_project(target: &mut Project, incoming: Project) {
    target.name = incoming.name;
    target.repo = incoming.repo;
    target.collapsed = incoming.collapsed;
    target.features = merge_feature_vec(std::mem::take(&mut target.features), incoming.features);
    target.created_at = incoming.created_at;
    target.preferred_agent = incoming.preferred_agent;
    target.is_git = incoming.is_git;
}

fn merge_feature_vec(mut existing: Vec<Feature>, incoming: Vec<Feature>) -> Vec<Feature> {
    for feature in incoming {
        if let Some(existing_feature) = existing.iter_mut().find(|f| f.id == feature.id) {
            merge_feature(existing_feature, feature);
        } else {
            existing.push(feature);
        }
    }
    existing
}

fn merge_feature(target: &mut Feature, incoming: Feature) {
    target.name = incoming.name;
    target.branch = incoming.branch;
    target.workdir = incoming.workdir;
    target.is_worktree = incoming.is_worktree;
    target.tmux_session = incoming.tmux_session;
    target.sessions = merge_session_vec(std::mem::take(&mut target.sessions), incoming.sessions);
    target.collapsed = incoming.collapsed;
    target.mode = incoming.mode;
    target.review = incoming.review;
    target.plan_mode = incoming.plan_mode;
    target.agent = incoming.agent;
    target.enable_chrome = incoming.enable_chrome;
    target.remote_control = incoming.remote_control;
    target.pending_worktree_script = incoming.pending_worktree_script;
    target.ready = incoming.ready;
    target.status = incoming.status;
    target.created_at = incoming.created_at;
    target.last_accessed = incoming.last_accessed;
    target.summary = incoming.summary;
    target.summary_updated_at = incoming.summary_updated_at;
    target.nickname = incoming.nickname;
    target.selected_plan_path = incoming.selected_plan_path;
}

fn merge_session_vec(
    mut existing: Vec<FeatureSession>,
    incoming: Vec<FeatureSession>,
) -> Vec<FeatureSession> {
    for session in incoming {
        if let Some(existing_session) = existing.iter_mut().find(|s| s.id == session.id) {
            *existing_session = session;
        } else {
            existing.push(session);
        }
    }
    existing
}

// --- V1 types for migration ---

#[derive(Debug, Deserialize)]
struct V1Feature {
    id: String,
    name: String,
    branch: String,
    workdir: PathBuf,
    is_worktree: bool,
    tmux_session: String,
    claude_session_id: Option<String>,
    status: ProjectStatus,
    created_at: DateTime<Utc>,
    last_accessed: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct V1Project {
    id: String,
    name: String,
    repo: PathBuf,
    collapsed: bool,
    features: Vec<V1Feature>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct V1ProjectStore {
    #[allow(dead_code)]
    version: u32,
    projects: Vec<V1Project>,
}

// --- Old flat format for migration (pre-v1) ---

#[derive(Debug, Deserialize)]
struct OldProject {
    #[allow(dead_code)]
    id: String,
    name: String,
    repo: PathBuf,
    workdir: PathBuf,
    branch: Option<String>,
    is_worktree: bool,
    tmux_session: String,
    claude_session_id: Option<String>,
    status: ProjectStatus,
    created_at: DateTime<Utc>,
    last_accessed: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct OldProjectStore {
    projects: Vec<OldProject>,
}

impl ProjectStore {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                version: CURRENT_PROJECT_STORE_VERSION,
                projects: Vec::new(),
                session_bookmarks: default_session_bookmarks(),
                available_harnesses: Vec::new(),
                prompt_templates: Vec::new(),
                extra: HashMap::new(),
            });
        }
        let data = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        let raw: serde_json::Value = serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse {}", path.display()))?;

        let version = raw.get("version").and_then(|v| v.as_u64()).unwrap_or(0);

        match version {
            0 => {
                // Old flat format -> v1 -> v2 -> v3 -> v4
                let old: OldProjectStore = serde_json::from_value(raw)
                    .with_context(|| "Failed to parse old project store")?;
                let v1 = Self::migrate_from_old(old);
                let v2 = Self::migrate_from_v1(v1);
                let v3 = Self::migrate_from_v2(v2);
                let mut store = Self::migrate_from_v3(v3);
                store.normalize_legacy_review_modes();
                store.save(path)?;
                Ok(store)
            }
            1 => {
                let v1: V1ProjectStore = serde_json::from_value(raw)
                    .with_context(|| "Failed to parse v1 project store")?;
                let v2 = Self::migrate_from_v1(v1);
                let v3 = Self::migrate_from_v2(v2);
                let mut store = Self::migrate_from_v3(v3);
                store.normalize_legacy_review_modes();
                store.save(path)?;
                Ok(store)
            }
            2 => {
                let v2: ProjectStore =
                    serde_json::from_value(raw).with_context(|| "Failed to parse project store")?;
                let v3 = Self::migrate_from_v2(v2);
                let mut store = Self::migrate_from_v3(v3);
                store.normalize_legacy_review_modes();
                store.save(path)?;
                Ok(store)
            }
            3 => {
                let v3: ProjectStore = serde_json::from_value(raw)
                    .with_context(|| "Failed to parse v3 project store")?;
                let mut store = Self::migrate_from_v3(v3);
                store.normalize_legacy_review_modes();
                store.save(path)?;
                Ok(store)
            }
            4 => {
                let mut store: ProjectStore = serde_json::from_value(raw)
                    .with_context(|| "Failed to parse v4 project store")?;
                let mut needs_save = store.normalize_legacy_review_modes();
                if store.version < CURRENT_PROJECT_STORE_VERSION {
                    store.version = CURRENT_PROJECT_STORE_VERSION;
                    needs_save = true;
                }
                if needs_save {
                    store.save(path)?;
                }
                Ok(store)
            }
            version if version >= 5 => {
                let mut store: ProjectStore = serde_json::from_value(raw)
                    .with_context(|| format!("Failed to parse v{} project store", version))?;
                if store.normalize_legacy_review_modes() {
                    store.save(path)?;
                }
                Ok(store)
            }
            _ => {
                bail!("Unknown project store version: {}", version);
            }
        }
    }

    fn migrate_from_v2(v2: ProjectStore) -> Self {
        // Add summary fields to features (serde default handles this)
        Self {
            version: 3,
            projects: v2.projects,
            session_bookmarks: default_session_bookmarks(),
            available_harnesses: Vec::new(),
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        }
    }

    fn migrate_from_v3(v3: ProjectStore) -> Self {
        // Add nickname field to features (serde default handles this)
        Self {
            version: CURRENT_PROJECT_STORE_VERSION,
            projects: v3.projects,
            session_bookmarks: default_session_bookmarks(),
            available_harnesses: Vec::new(),
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        }
    }

    /// Migrate from old flat format to v1 intermediary.
    fn migrate_from_old(old: OldProjectStore) -> V1ProjectStore {
        let mut repo_groups: HashMap<PathBuf, Vec<OldProject>> = HashMap::new();
        for proj in old.projects {
            repo_groups.entry(proj.repo.clone()).or_default().push(proj);
        }

        let mut projects = Vec::new();
        for (repo, old_projects) in repo_groups {
            let project_name = repo
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unnamed".into());

            let earliest = old_projects
                .iter()
                .map(|p| p.created_at)
                .min()
                .unwrap_or_else(Utc::now);

            let features = old_projects
                .into_iter()
                .map(|old_proj| {
                    let branch = old_proj.branch.unwrap_or_else(|| "main".into());
                    V1Feature {
                        id: Uuid::new_v4().to_string(),
                        name: old_proj.name,
                        branch,
                        workdir: old_proj.workdir,
                        is_worktree: old_proj.is_worktree,
                        tmux_session: old_proj.tmux_session,
                        claude_session_id: old_proj.claude_session_id,
                        status: old_proj.status,
                        created_at: old_proj.created_at,
                        last_accessed: old_proj.last_accessed,
                    }
                })
                .collect();

            projects.push(V1Project {
                id: Uuid::new_v4().to_string(),
                name: project_name,
                repo,
                collapsed: false,
                features,
                created_at: earliest,
            });
        }

        V1ProjectStore {
            version: 1,
            projects,
        }
    }

    /// Migrate from v1 to v2: add FeatureSessions to each
    /// feature, preserving existing tmux window names.
    fn migrate_from_v1(v1: V1ProjectStore) -> Self {
        let projects = v1
            .projects
            .into_iter()
            .map(|p| {
                let features = p
                    .features
                    .into_iter()
                    .map(|f| {
                        let sessions = vec![
                            FeatureSession {
                                id: Uuid::new_v4().to_string(),
                                kind: SessionKind::Claude,
                                label: "Claude 1".into(),
                                tmux_window: "claude".into(),
                                claude_session_id: f.claude_session_id,
                                todo_reference: None,
                                token_usage_source: None,
                                token_usage_source_match: None,
                                created_at: f.created_at,
                                command: None,
                                on_stop: None,
                                pre_check: None,
                                status_text: None,
                                token_usage: None,
                            },
                            FeatureSession {
                                id: Uuid::new_v4().to_string(),
                                kind: SessionKind::Terminal,
                                label: "Terminal 1".into(),
                                tmux_window: "terminal".into(),
                                claude_session_id: None,
                                todo_reference: None,
                                token_usage_source: None,
                                token_usage_source_match: None,
                                created_at: f.created_at,
                                command: None,
                                on_stop: None,
                                pre_check: None,
                                status_text: None,
                                token_usage: None,
                            },
                        ];
                        Feature {
                            id: f.id,
                            name: f.name,
                            branch: f.branch,
                            workdir: f.workdir,
                            is_worktree: f.is_worktree,
                            tmux_session: f.tmux_session,
                            sessions,
                            collapsed: true,
                            mode: VibeMode::default(),
                            review: false,
                            plan_mode: false,
                            agent: AgentKind::default(),
                            enable_chrome: false,
                            remote_control: false,
                            pending_worktree_script: false,
                            ready: false,
                            status: f.status,
                            created_at: f.created_at,
                            last_accessed: f.last_accessed,
                            summary: None,
                            summary_updated_at: None,
                            nickname: None,
                            selected_plan_path: None,
                            triage_source: None,
                            review_source: None,
                        }
                    })
                    .collect();
                Project {
                    id: p.id,
                    name: p.name,
                    repo: p.repo,
                    collapsed: p.collapsed,
                    features,
                    created_at: p.created_at,
                    preferred_agent: AgentKind::default(),
                    is_git: true,
                }
            })
            .collect();

        Self {
            version: 2,
            projects,
            session_bookmarks: default_session_bookmarks(),
            available_harnesses: Vec::new(),
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut persisted = self.clone();
        for project in &mut persisted.projects {
            project
                .features
                .retain(|feature| !feature.pending_worktree_script);
        }
        let data = serde_json::to_string_pretty(&persisted)?;
        fs::write(path, data).with_context(|| format!("Failed to write {}", path.display()))?;
        Ok(())
    }

    pub fn add_project(&mut self, project: Project) {
        self.projects.push(project);
    }

    pub fn remove_project(&mut self, name: &str) -> Option<Project> {
        if let Some(idx) = self.projects.iter().position(|p| p.name == name) {
            Some(self.projects.remove(idx))
        } else {
            None
        }
    }

    pub fn find_project(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.name == name)
    }

    pub fn find_project_mut(&mut self, name: &str) -> Option<&mut Project> {
        self.projects.iter_mut().find(|p| p.name == name)
    }

    pub fn add_feature(&mut self, project_name: &str, feature: Feature) -> bool {
        if let Some(project) = self.find_project_mut(project_name) {
            project.features.push(feature);
            true
        } else {
            false
        }
    }

    pub fn remove_feature(&mut self, project_name: &str, feature_name: &str) -> Option<Feature> {
        if let Some(project) = self.find_project_mut(project_name)
            && let Some(idx) = project.features.iter().position(|f| f.name == feature_name)
        {
            return Some(project.features.remove(idx));
        }
        None
    }

    fn normalize_legacy_review_modes(&mut self) -> bool {
        let mut changed = false;
        for project in &mut self.projects {
            for feature in &mut project.features {
                changed |= feature.normalize_legacy_review_mode();
            }
        }
        changed
    }
}

/// Resolve the AMF config directory, honoring `XDG_CONFIG_HOME` (via
/// `dirs::config_dir()`) while falling back to the legacy hardcoded
/// `~/.config/amf` path for installs that already have data there.
///
/// Falling back rather than migrating means an existing install keeps
/// working unchanged after an `amf` upgrade, even on platforms where
/// `dirs::config_dir()` differs from `~/.config` (e.g. macOS); only
/// fresh installs (or users who set `XDG_CONFIG_HOME` before ever
/// running `amf`) land in the XDG-correct location.
pub fn amf_config_dir() -> PathBuf {
    #[cfg(test)]
    return test_sandbox_root().join(".config").join("amf");
    #[cfg(not(test))]
    amf_config_dir_with(dirs::config_dir(), dirs::home_dir())
}

/// Per-process stand-in for the user's home directory, used by the path
/// resolvers below while the test binary is running.
///
/// This exists because `ensure_notify_scripts` writes executable hook scripts
/// *unconditionally* — that is its job. Without isolation the suite overwrites
/// the developer's live `~/.amf/hooks/` and `~/.config/amf/`, and a hook script
/// newer than the `amf` binary on `$PATH` will break every running Claude Code
/// session on the machine until it is repaired by hand. That is not
/// hypothetical: it happened, and it locked a session out of every tool.
///
/// Deliberately not an environment variable: `std::env::set_var` is `unsafe` in
/// the 2024 edition and process-global, so it would race across the parallel
/// test threads. A `OnceLock` needs no ordering guarantee — the first resolver
/// call in the process wins and every later one agrees.
#[cfg(test)]
fn test_sandbox_root() -> &'static PathBuf {
    static ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("amf-test-home-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&root);
        root
    })
}

/// Directory for generated Claude hook executables.
///
/// Keep this independent from the platform config directory because macOS
/// resolves that directory under `~/Library/Application Support`, while some
/// Claude versions still route command hooks through a shell even when they
/// use exec-form arguments.
pub fn amf_claude_hooks_dir() -> PathBuf {
    // The `.amf/hooks` shape is load-bearing even under test: it is the
    // structural invariant `is_amf_claude_hook_command` uses to recognise a
    // script as AMF-managed.
    #[cfg(test)]
    return test_sandbox_root().join(".amf").join("hooks");
    #[cfg(not(test))]
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".amf")
        .join("hooks")
}

fn amf_config_dir_with(xdg_config_dir: Option<PathBuf>, home_dir: Option<PathBuf>) -> PathBuf {
    let legacy = home_dir
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("amf");

    let Some(xdg_config_dir) = xdg_config_dir else {
        return legacy;
    };
    let xdg = xdg_config_dir.join("amf");

    if xdg != legacy && legacy.exists() && !xdg.exists() {
        return legacy;
    }
    xdg
}

#[allow(dead_code)] // exercised only by unit tests
pub fn global_store_path() -> PathBuf {
    amf_config_dir().join("projects.json")
}

pub fn global_db_path() -> PathBuf {
    amf_config_dir().join("amf.db")
}

pub fn db_path() -> PathBuf {
    global_db_path()
}

#[allow(dead_code)] // exercised only by unit tests
pub fn store_path() -> PathBuf {
    global_store_path()
}

#[cfg(test)]
mod tests {
    /// The suite writes real executable hook scripts through
    /// `ensure_notify_scripts`. If these resolvers ever point back at the
    /// developer's actual home directory, running `cargo test` silently
    /// rewrites their live hooks — and a script newer than the `amf` binary on
    /// `$PATH` breaks every running Claude Code session on the machine.
    #[test]
    fn path_resolvers_are_sandboxed_away_from_the_real_home() {
        let home = dirs::home_dir().expect("test host should have a home directory");
        let sandbox = super::test_sandbox_root();

        assert!(
            !sandbox.starts_with(&home),
            "test sandbox {sandbox:?} must not live under the real home {home:?}"
        );
        for dir in [super::amf_claude_hooks_dir(), super::amf_config_dir()] {
            assert!(
                dir.starts_with(sandbox),
                "{dir:?} must resolve inside the test sandbox {sandbox:?}"
            );
            assert!(
                !dir.starts_with(&home),
                "{dir:?} must not resolve under the real home {home:?}"
            );
        }

        // The `.amf/hooks` shape is what `is_amf_claude_hook_command` matches
        // on, so the sandbox has to preserve it rather than use a flat path.
        assert!(super::amf_claude_hooks_dir().ends_with(".amf/hooks"));
    }

    /// Feature JSON written before the companion-triage link existed still
    /// deserializes — it simply isn't a triage feature.
    #[test]
    fn feature_without_triage_source_deserializes_as_an_ordinary_feature() {
        let json = serde_json::json!({
            "id": "feat-1",
            "name": "my-feat",
            "branch": "my-feat",
            "workdir": "/tmp/wd",
            "is_worktree": false,
            "tmux_session": "amf-my-feat",
            "status": "stopped",
            "created_at": "2026-01-01T00:00:00Z",
            "last_accessed": "2026-01-01T00:00:00Z",
        });
        let feature: Feature = serde_json::from_value(json).unwrap();
        assert!(feature.triage_source.is_none());
        assert!(feature.selected_plan_path.is_none());
    }

    #[test]
    fn feature_triage_source_round_trips_through_json() {
        let mut feature = Feature::new(
            "main-triage".to_string(),
            "main-triage".to_string(),
            PathBuf::from("/tmp/wd"),
            true,
            VibeMode::Vibeless,
            false,
            false,
            AgentKind::Codex,
            false,
            false,
        );
        feature.triage_source = Some(TriageSource {
            pr_number: 7,
            source_feature_id: "feat-source".to_string(),
            pr_branch: "main".to_string(),
            base_sha: "abc".to_string(),
        });

        let round_tripped: Feature =
            serde_json::from_str(&serde_json::to_string(&feature).unwrap()).unwrap();

        assert_eq!(round_tripped.triage_source, feature.triage_source);
    }

    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;
    use tempfile::NamedTempFile;

    fn make_feature_session(kind: SessionKind, window: &str) -> FeatureSession {
        FeatureSession {
            id: "test-id".to_string(),
            kind,
            label: "test".to_string(),
            tmux_window: window.to_string(),
            claude_session_id: None,
            todo_reference: None,
            token_usage_source: None,
            token_usage_source_match: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
            token_usage: None,
        }
    }

    #[test]
    fn session_todo_reference_is_backward_compatible_and_serializes_when_present() {
        let legacy = r#"{
            "id":"session-1",
            "kind":"claude",
            "label":"Claude 1",
            "tmux_window":"claude",
            "claude_session_id":null,
            "created_at":"2025-01-01T00:00:00Z"
        }"#;

        let legacy_session: FeatureSession = serde_json::from_str(legacy).unwrap();
        assert!(legacy_session.todo_reference.is_none());

        let referenced = FeatureSession {
            todo_reference: Some(TodoSessionReference {
                todo_id: "todo-1".to_string(),
                launched_from_todo_menu: true,
            }),
            ..legacy_session
        };
        let serialized = serde_json::to_value(referenced).unwrap();
        assert_eq!(serialized["todo_reference"]["todo_id"], "todo-1");
        assert_eq!(
            serialized["todo_reference"]["launched_from_todo_menu"],
            true
        );
    }

    fn make_feature() -> Feature {
        Feature {
            id: "feat-id".to_string(),
            name: "test-feature".to_string(),
            branch: "test-branch".to_string(),
            workdir: PathBuf::from("/tmp/test"),
            is_worktree: false,
            tmux_session: "amf-test".to_string(),
            sessions: vec![],
            collapsed: true,
            mode: VibeMode::default(),
            review: false,
            plan_mode: false,
            agent: AgentKind::default(),
            enable_chrome: false,
            remote_control: false,
            pending_worktree_script: false,
            ready: false,
            status: ProjectStatus::Stopped,
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            summary: None,
            summary_updated_at: None,
            nickname: None,
            selected_plan_path: None,
            triage_source: None,
            review_source: None,
        }
    }

    #[test]
    fn db_path_is_global_db_path() {
        assert_eq!(db_path(), global_db_path());
    }

    #[test]
    fn store_path_is_global_store_path() {
        assert_eq!(store_path(), global_store_path());
    }

    #[test]
    fn project_store_merge_unions_entities_by_id() {
        let mut base = ProjectStore {
            version: 4,
            projects: vec![Project {
                id: "project-1".to_string(),
                name: "alpha".to_string(),
                repo: PathBuf::from("/repo/alpha"),
                collapsed: true,
                features: vec![Feature {
                    id: "feature-1".to_string(),
                    name: "feature-one".to_string(),
                    branch: "branch-a".to_string(),
                    workdir: PathBuf::from("/repo/alpha/.worktrees/a"),
                    is_worktree: true,
                    tmux_session: "amf-a".to_string(),
                    sessions: vec![FeatureSession {
                        id: "session-1".to_string(),
                        kind: SessionKind::Claude,
                        label: "Claude 1".to_string(),
                        tmux_window: "claude".to_string(),
                        claude_session_id: None,
                        todo_reference: None,
                        token_usage_source: None,
                        token_usage_source_match: None,
                        created_at: Utc::now(),
                        command: None,
                        on_stop: None,
                        pre_check: None,
                        status_text: None,
                        token_usage: None,
                    }],
                    collapsed: true,
                    mode: VibeMode::Vibeless,
                    review: false,
                    plan_mode: false,
                    agent: AgentKind::Claude,
                    enable_chrome: false,
                    remote_control: false,
                    pending_worktree_script: false,
                    ready: false,
                    status: ProjectStatus::Stopped,
                    created_at: Utc::now(),
                    last_accessed: Utc::now(),
                    summary: None,
                    summary_updated_at: None,
                    nickname: None,
                    selected_plan_path: None,
                    triage_source: None,
                    review_source: None,
                }],
                created_at: Utc::now(),
                preferred_agent: AgentKind::Claude,
                is_git: true,
            }],
            session_bookmarks: vec![SessionBookmark {
                project_id: "project-1".to_string(),
                feature_id: "feature-1".to_string(),
                session_id: "session-1".to_string(),
            }],
            available_harnesses: vec![AgentKind::Claude],
            prompt_templates: Vec::new(),
            extra: HashMap::from([(String::from("alpha"), serde_json::json!(1))]),
        };

        let incoming = ProjectStore {
            version: 5,
            projects: vec![Project {
                id: "project-1".to_string(),
                name: "alpha-renamed".to_string(),
                repo: PathBuf::from("/repo/alpha-renamed"),
                collapsed: false,
                features: vec![
                    Feature {
                        id: "feature-1".to_string(),
                        name: "feature-one-renamed".to_string(),
                        branch: "branch-b".to_string(),
                        workdir: PathBuf::from("/repo/alpha/.worktrees/b"),
                        is_worktree: false,
                        tmux_session: "amf-b".to_string(),
                        sessions: vec![
                            FeatureSession {
                                id: "session-1".to_string(),
                                kind: SessionKind::Terminal,
                                label: "Terminal 1".to_string(),
                                tmux_window: "terminal".to_string(),
                                claude_session_id: Some("claude-123".to_string()),
                                todo_reference: None,
                                token_usage_source: None,
                                token_usage_source_match: None,
                                created_at: Utc::now(),
                                command: Some("echo hi".to_string()),
                                on_stop: None,
                                pre_check: None,
                                status_text: None,
                                token_usage: None,
                            },
                            FeatureSession {
                                id: "session-2".to_string(),
                                kind: SessionKind::Claude,
                                label: "Claude 2".to_string(),
                                tmux_window: "claude-2".to_string(),
                                claude_session_id: None,
                                todo_reference: None,
                                token_usage_source: None,
                                token_usage_source_match: None,
                                created_at: Utc::now(),
                                command: None,
                                on_stop: None,
                                pre_check: None,
                                status_text: None,
                                token_usage: None,
                            },
                        ],
                        collapsed: false,
                        mode: VibeMode::Vibe,
                        review: true,
                        plan_mode: true,
                        agent: AgentKind::Codex,
                        enable_chrome: true,
                        remote_control: false,
                        pending_worktree_script: true,
                        ready: true,
                        status: ProjectStatus::Active,
                        created_at: Utc::now(),
                        last_accessed: Utc::now(),
                        summary: Some("summary".to_string()),
                        summary_updated_at: Some(Utc::now()),
                        nickname: Some("nick".to_string()),
                        selected_plan_path: None,
                        triage_source: None,
                        review_source: None,
                    },
                    Feature {
                        id: "feature-2".to_string(),
                        name: "feature-two".to_string(),
                        branch: "branch-c".to_string(),
                        workdir: PathBuf::from("/repo/alpha/.worktrees/c"),
                        is_worktree: true,
                        tmux_session: "amf-c".to_string(),
                        sessions: vec![],
                        collapsed: true,
                        mode: VibeMode::SuperVibe,
                        review: false,
                        plan_mode: false,
                        agent: AgentKind::Pi,
                        enable_chrome: false,
                        remote_control: false,
                        pending_worktree_script: false,
                        ready: false,
                        status: ProjectStatus::Idle,
                        created_at: Utc::now(),
                        last_accessed: Utc::now(),
                        summary: None,
                        summary_updated_at: None,
                        nickname: None,
                        selected_plan_path: None,
                        triage_source: None,
                        review_source: None,
                    },
                ],
                created_at: Utc::now(),
                preferred_agent: AgentKind::Codex,
                is_git: false,
            }],
            session_bookmarks: vec![
                SessionBookmark {
                    project_id: "project-1".to_string(),
                    feature_id: "feature-1".to_string(),
                    session_id: "session-1".to_string(),
                },
                SessionBookmark {
                    project_id: "project-1".to_string(),
                    feature_id: "feature-2".to_string(),
                    session_id: "session-2".to_string(),
                },
            ],
            available_harnesses: vec![AgentKind::Codex, AgentKind::Pi],
            prompt_templates: Vec::new(),
            extra: HashMap::from([
                (String::from("alpha"), serde_json::json!(2)),
                (String::from("beta"), serde_json::json!(true)),
            ]),
        };

        base.merge_from(incoming);

        assert_eq!(base.version, 5);
        assert_eq!(
            base.available_harnesses,
            vec![AgentKind::Claude, AgentKind::Codex, AgentKind::Pi]
        );
        assert_eq!(base.session_bookmarks.len(), 2);
        assert_eq!(base.extra.get("alpha"), Some(&serde_json::json!(2)));
        assert_eq!(base.extra.get("beta"), Some(&serde_json::json!(true)));
        assert_eq!(base.projects.len(), 1);
        assert_eq!(base.projects[0].name, "alpha-renamed");
        assert_eq!(base.projects[0].repo, PathBuf::from("/repo/alpha-renamed"));
        assert!(!base.projects[0].collapsed);
        assert_eq!(base.projects[0].preferred_agent, AgentKind::Codex);
        assert!(!base.projects[0].is_git);
        assert_eq!(base.projects[0].features.len(), 2);
        let merged_feature = base.projects[0]
            .features
            .iter()
            .find(|feature| feature.id == "feature-1")
            .unwrap();
        assert_eq!(merged_feature.name, "feature-one-renamed");
        assert_eq!(merged_feature.tmux_session, "amf-b");
        assert_eq!(merged_feature.mode, VibeMode::Vibe);
        assert!(merged_feature.review);
        assert!(merged_feature.pending_worktree_script);
        assert_eq!(merged_feature.sessions.len(), 2);
    }

    // ── ProjectStore serialization round-trip ────────────────

    #[test]
    fn projectstore_roundtrip() {
        let store = ProjectStore {
            version: CURRENT_PROJECT_STORE_VERSION,
            projects: vec![Project {
                id: "proj-id".to_string(),
                name: "my-project".to_string(),
                repo: PathBuf::from("/home/user/my-project"),
                collapsed: false,
                features: vec![],
                created_at: Utc::now(),
                preferred_agent: AgentKind::Codex,
                is_git: true,
            }],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        };
        let tmp = NamedTempFile::new().unwrap();
        store.save(tmp.path()).unwrap();

        let loaded = ProjectStore::load(tmp.path()).unwrap();
        assert_eq!(loaded.version, CURRENT_PROJECT_STORE_VERSION);
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].name, "my-project");
        assert_eq!(
            loaded.projects[0].repo,
            PathBuf::from("/home/user/my-project")
        );
        assert_eq!(loaded.projects[0].preferred_agent, AgentKind::Codex);
        assert!(loaded.projects[0].is_git);
    }

    #[test]
    fn projectstore_load_defaults_missing_preferred_agent() {
        let json = r#"{
            "version": 4,
            "projects": [
                {
                    "id": "proj-id",
                    "name": "my-project",
                    "repo": "/home/user/my-project",
                    "collapsed": false,
                    "features": [],
                    "created_at": "2024-01-01T00:00:00Z",
                    "is_git": true
                }
            ],
            "session_bookmarks": []
        }"#;
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), json).unwrap();

        let loaded = ProjectStore::load(tmp.path()).unwrap();
        assert_eq!(loaded.projects[0].preferred_agent, AgentKind::Claude);
    }

    #[test]
    fn projectstore_loads_version_5_and_preserves_unknown_top_level_fields() {
        let json = r#"{
            "version": 5,
            "projects": [
                {
                    "id": "proj-id",
                    "name": "my-project",
                    "repo": "/home/user/my-project",
                    "collapsed": false,
                    "features": [],
                    "created_at": "2024-01-01T00:00:00Z",
                    "preferred_agent": "codex",
                    "is_git": true
                }
            ],
            "session_bookmarks": [],
            "guided_tours": {
                "proj-id": {
                    "summary": "tour",
                    "highlights": [],
                    "stops": [],
                    "created_at": "2024-01-01T00:00:00Z",
                    "updated_at": "2024-01-01T00:00:00Z"
                }
            }
        }"#;
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), json).unwrap();

        let loaded = ProjectStore::load(tmp.path()).unwrap();
        assert_eq!(loaded.version, 5);
        assert!(loaded.extra.contains_key("guided_tours"));

        loaded.save(tmp.path()).unwrap();

        let reloaded_json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(tmp.path()).unwrap()).unwrap();
        assert!(reloaded_json.get("guided_tours").is_some());
    }

    #[test]
    fn migration_v4_to_v5() {
        let store = ProjectStore {
            version: 4,
            projects: vec![Project {
                id: "proj-id".to_string(),
                name: "my-project".to_string(),
                repo: PathBuf::from("/home/user/my-project"),
                collapsed: false,
                features: vec![],
                created_at: Utc::now(),
                preferred_agent: AgentKind::Claude,
                is_git: true,
            }],
            session_bookmarks: vec![],
            available_harnesses: vec![],
            prompt_templates: Vec::new(),
            extra: HashMap::new(),
        };
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), serde_json::to_string_pretty(&store).unwrap()).unwrap();

        let loaded = ProjectStore::load(tmp.path()).unwrap();
        assert_eq!(loaded.version, CURRENT_PROJECT_STORE_VERSION);

        let reloaded: ProjectStore =
            serde_json::from_str(&std::fs::read_to_string(tmp.path()).unwrap()).unwrap();
        assert_eq!(reloaded.version, CURRENT_PROJECT_STORE_VERSION);
    }

    // ── Migration v0 → v2 ────────────────────────────────────

    #[test]
    fn migration_v0_to_v2() {
        let v0_json = r#"{
            "projects": [
                {
                    "id": "old-id",
                    "name": "my-feature",
                    "repo": "/home/user/my-repo",
                    "workdir": "/home/user/my-repo",
                    "branch": "main",
                    "is_worktree": false,
                    "tmux_session": "amf-my-feature",
                    "claude_session_id": null,
                    "status": "stopped",
                    "created_at": "2024-01-01T00:00:00Z",
                    "last_accessed": "2024-01-01T00:00:00Z"
                }
            ]
        }"#;
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), v0_json).unwrap();

        let store = ProjectStore::load(tmp.path()).unwrap();
        assert_eq!(store.version, CURRENT_PROJECT_STORE_VERSION);
        assert_eq!(store.projects.len(), 1);

        let proj = &store.projects[0];
        // project name derived from repo basename
        assert_eq!(proj.name, "my-repo");
        assert_eq!(proj.preferred_agent, AgentKind::Claude);
        assert_eq!(proj.features.len(), 1);

        let feat = &proj.features[0];
        assert_eq!(feat.name, "my-feature");
        assert_eq!(feat.branch, "main");
        // v0 → v1 → v2 → v3 → v4 adds Claude + Terminal sessions + summary + nickname
        assert_eq!(feat.sessions.len(), 2);
        assert!(feat.sessions.iter().any(|s| s.kind == SessionKind::Claude));
        assert!(
            feat.sessions
                .iter()
                .any(|s| s.kind == SessionKind::Terminal)
        );
    }

    // ── Migration v1 → v2 ────────────────────────────────────

    #[test]
    fn migration_v1_to_v2() {
        let v1_json = r#"{
            "version": 1,
            "projects": [
                {
                    "id": "proj-id",
                    "name": "my-project",
                    "repo": "/home/user/my-repo",
                    "collapsed": false,
                    "features": [
                        {
                            "id": "feat-id",
                            "name": "my-feature",
                            "branch": "feat/my-feature",
                            "workdir": "/home/user/my-repo/.worktrees/my-feature",
                            "is_worktree": true,
                            "tmux_session": "amf-my-feature",
                            "claude_session_id": "sess-123",
                            "status": "idle",
                            "created_at": "2024-06-01T12:00:00Z",
                            "last_accessed": "2024-06-01T12:00:00Z"
                        }
                    ],
                    "created_at": "2024-06-01T00:00:00Z"
                }
            ]
        }"#;
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), v1_json).unwrap();

        let store = ProjectStore::load(tmp.path()).unwrap();
        assert_eq!(store.version, CURRENT_PROJECT_STORE_VERSION);
        assert_eq!(store.projects.len(), 1);

        let proj = &store.projects[0];
        assert_eq!(proj.name, "my-project");
        assert_eq!(proj.preferred_agent, AgentKind::Claude);

        let feat = &proj.features[0];
        assert_eq!(feat.name, "my-feature");
        assert_eq!(feat.sessions.len(), 2);

        let claude_sess = feat
            .sessions
            .iter()
            .find(|s| s.kind == SessionKind::Claude)
            .unwrap();
        assert_eq!(claude_sess.claude_session_id, Some("sess-123".to_string()));
        assert_eq!(claude_sess.tmux_window, "claude");

        let term_sess = feat
            .sessions
            .iter()
            .find(|s| s.kind == SessionKind::Terminal)
            .unwrap();
        assert_eq!(term_sess.tmux_window, "terminal");
    }

    #[test]
    fn load_normalizes_legacy_review_mode() {
        let v4_json = r#"{
            "version": 4,
            "projects": [
                {
                    "id": "proj-id",
                    "name": "my-project",
                    "repo": "/home/user/my-repo",
                    "collapsed": false,
                    "features": [
                        {
                            "id": "feat-id",
                            "name": "my-feature",
                            "branch": "feat/my-feature",
                            "workdir": "/home/user/my-repo/.worktrees/my-feature",
                            "is_worktree": true,
                            "tmux_session": "amf-my-feature",
                            "sessions": [],
                            "collapsed": true,
                            "mode": "review",
                            "review": false,
                            "agent": "claude",
                            "enable_chrome": false,
                            "ready": false,
                            "status": "idle",
                            "created_at": "2024-06-01T12:00:00Z",
                            "last_accessed": "2024-06-01T12:00:00Z"
                        }
                    ],
                    "created_at": "2024-06-01T00:00:00Z",
                    "is_git": true
                }
            ],
            "session_bookmarks": []
        }"#;
        let tmp = NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), v4_json).unwrap();

        let store = ProjectStore::load(tmp.path()).unwrap();
        let feature = &store.projects[0].features[0];
        assert_eq!(feature.mode, VibeMode::Vibeless);
        assert!(feature.review);

        let saved = std::fs::read_to_string(tmp.path()).unwrap();
        assert!(saved.contains("\"mode\": \"vibeless\""));
        assert!(saved.contains("\"review\": true"));
    }

    // ── Feature::next_label ───────────────────────────────────

    #[test]
    fn next_label_empty_sessions() {
        let feat = make_feature();
        assert_eq!(feat.next_label(&SessionKind::Claude), "Claude 1");
        assert_eq!(feat.next_label(&SessionKind::Terminal), "Terminal 1");
        assert_eq!(feat.next_label(&SessionKind::Nvim), "Nvim 1");
    }

    #[test]
    fn next_label_one_claude_session() {
        let mut feat = make_feature();
        feat.sessions
            .push(make_feature_session(SessionKind::Claude, "claude"));
        assert_eq!(feat.next_label(&SessionKind::Claude), "Claude 2");
        // Terminal count unaffected
        assert_eq!(feat.next_label(&SessionKind::Terminal), "Terminal 1");
    }

    #[test]
    fn next_label_mixed_sessions() {
        let mut feat = make_feature();
        feat.sessions
            .push(make_feature_session(SessionKind::Claude, "claude"));
        feat.sessions
            .push(make_feature_session(SessionKind::Terminal, "terminal"));
        feat.sessions
            .push(make_feature_session(SessionKind::Terminal, "terminal-2"));
        assert_eq!(feat.next_label(&SessionKind::Claude), "Claude 2");
        assert_eq!(feat.next_label(&SessionKind::Terminal), "Terminal 3");
    }

    // ── Feature::next_window_name ─────────────────────────────

    #[test]
    fn next_window_name_empty_sessions() {
        let feat = make_feature();
        assert_eq!(feat.next_window_name(&SessionKind::Claude), "claude");
        assert_eq!(feat.next_window_name(&SessionKind::Terminal), "terminal");
    }

    #[test]
    fn next_window_name_one_existing_session() {
        let mut feat = make_feature();
        feat.sessions
            .push(make_feature_session(SessionKind::Claude, "claude"));
        assert_eq!(feat.next_window_name(&SessionKind::Claude), "claude-2");
        // Terminal still empty → just prefix
        assert_eq!(feat.next_window_name(&SessionKind::Terminal), "terminal");
    }

    #[test]
    fn next_window_name_collision_avoidance() {
        let mut feat = make_feature();
        feat.sessions
            .push(make_feature_session(SessionKind::Claude, "claude"));
        // Manually add "claude-2" to force a collision
        feat.sessions
            .push(make_feature_session(SessionKind::Claude, "claude-2"));
        // Should skip "claude-2" and return "claude-3"
        assert_eq!(feat.next_window_name(&SessionKind::Claude), "claude-3");
    }

    // ── VibeMode::cli_flags ───────────────────────────────────

    fn opts(enable_chrome: bool) -> LaunchOpts {
        LaunchOpts {
            enable_chrome,
            remote_control: false,
            session_name: None,
        }
    }

    fn opts_rc(name: Option<&str>) -> LaunchOpts {
        LaunchOpts {
            enable_chrome: false,
            remote_control: true,
            session_name: name.map(str::to_string),
        }
    }

    #[test]
    fn vibe_mode_vibeless_flags() {
        assert_eq!(
            VibeMode::Vibeless.cli_flags(opts(false)),
            Vec::<String>::new()
        );
        assert_eq!(VibeMode::Vibeless.cli_flags(opts(true)), vec!["--chrome"]);
    }

    #[test]
    fn vibe_mode_vibe_flags() {
        assert_eq!(
            VibeMode::Vibe.cli_flags(opts(false)),
            vec!["--permission-mode", "acceptEdits"]
        );
        assert_eq!(
            VibeMode::Vibe.cli_flags(opts(true)),
            vec!["--permission-mode", "acceptEdits", "--chrome"]
        );
    }

    #[test]
    fn vibe_mode_supervibe_flags() {
        assert_eq!(
            VibeMode::SuperVibe.cli_flags(opts(false)),
            vec!["--dangerously-skip-permissions"]
        );
        assert_eq!(
            VibeMode::SuperVibe.cli_flags(opts(true)),
            vec!["--dangerously-skip-permissions", "--chrome"]
        );
    }

    // ── LaunchOpts / remote_control flag tests ────────────────

    #[test]
    fn launch_opts_rc_off_produces_no_rc_flag() {
        let flags = VibeMode::Vibeless.cli_flags(opts(false));
        assert!(!flags.iter().any(|f| f == "--remote-control"));
    }

    #[test]
    fn launch_opts_rc_on_no_name() {
        let flags = VibeMode::Vibeless.cli_flags(opts_rc(None));
        assert_eq!(flags, vec!["--remote-control"]);
    }

    #[test]
    fn launch_opts_rc_on_with_name() {
        let flags = VibeMode::Vibeless.cli_flags(opts_rc(Some("my-feature")));
        assert_eq!(flags, vec!["--remote-control", "my-feature"]);
    }

    #[test]
    fn launch_opts_rc_on_with_chrome_and_name() {
        let flags = VibeMode::Vibeless.cli_flags(LaunchOpts {
            enable_chrome: true,
            remote_control: true,
            session_name: Some("my-feature".to_string()),
        });
        assert_eq!(flags, vec!["--chrome", "--remote-control", "my-feature"]);
    }

    #[test]
    fn launch_opts_supervibe_rc_on() {
        let flags = VibeMode::SuperVibe.cli_flags(opts_rc(Some("feat")));
        assert_eq!(
            flags,
            vec!["--dangerously-skip-permissions", "--remote-control", "feat"]
        );
    }

    #[test]
    fn launch_opts_rc_empty_name_omitted() {
        // An empty name string should not be appended.
        let flags = VibeMode::Vibeless.cli_flags(LaunchOpts {
            enable_chrome: false,
            remote_control: true,
            session_name: Some(String::new()),
        });
        assert_eq!(flags, vec!["--remote-control"]);
    }

    #[test]
    fn legacy_review_feature_migrates_to_review_flag() {
        let feature: Feature = serde_json::from_str(
            r#"{
                "id": "feat-id",
                "name": "legacy-review",
                "branch": "legacy-review",
                "workdir": "/tmp/test",
                "is_worktree": false,
                "tmux_session": "amf-legacy-review",
                "mode": "review",
                "status": "stopped",
                "created_at": "2024-01-01T00:00:00Z",
                "last_accessed": "2024-01-01T00:00:00Z"
            }"#,
        )
        .unwrap();

        assert_eq!(feature.mode, VibeMode::Vibeless);
        assert!(feature.review);
    }

    #[test]
    fn config_dir_uses_xdg_dir_when_no_legacy_data_exists() {
        let home = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();

        let resolved = amf_config_dir_with(
            Some(xdg.path().to_path_buf()),
            Some(home.path().to_path_buf()),
        );

        assert_eq!(resolved, xdg.path().join("amf"));
    }

    #[test]
    fn config_dir_falls_back_to_legacy_path_when_only_legacy_exists() {
        let home = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();
        let legacy = home.path().join(".config").join("amf");
        fs::create_dir_all(&legacy).unwrap();

        let resolved = amf_config_dir_with(
            Some(xdg.path().to_path_buf()),
            Some(home.path().to_path_buf()),
        );

        assert_eq!(resolved, legacy);
    }

    #[test]
    fn config_dir_prefers_xdg_path_when_both_exist() {
        let home = tempfile::tempdir().unwrap();
        let xdg = tempfile::tempdir().unwrap();
        fs::create_dir_all(home.path().join(".config").join("amf")).unwrap();
        fs::create_dir_all(xdg.path().join("amf")).unwrap();

        let resolved = amf_config_dir_with(
            Some(xdg.path().to_path_buf()),
            Some(home.path().to_path_buf()),
        );

        assert_eq!(resolved, xdg.path().join("amf"));
    }

    #[test]
    fn config_dir_falls_back_to_legacy_home_config_when_no_xdg_dir_available() {
        let home = tempfile::tempdir().unwrap();

        let resolved = amf_config_dir_with(None, Some(home.path().to_path_buf()));

        assert_eq!(resolved, home.path().join(".config").join("amf"));
    }
}
