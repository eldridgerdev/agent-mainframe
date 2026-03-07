use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

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
    Terminal,
    Nvim,
    Vscode,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    #[default]
    Claude,
    Opencode,
    Codex,
}

impl AgentKind {
    pub fn display_name(&self) -> &str {
        match self {
            AgentKind::Claude => "Claude",
            AgentKind::Opencode => "Opencode",
            AgentKind::Codex => "Codex",
        }
    }

    pub const ALL: [AgentKind; 3] = [AgentKind::Claude, AgentKind::Opencode, AgentKind::Codex];
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureSession {
    pub id: String,
    pub kind: SessionKind,
    pub label: String,
    pub tmux_window: String,
    pub claude_session_id: Option<String>,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_stop: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre_check: Option<String>,
    #[serde(skip)]
    pub status_text: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum VibeMode {
    #[default]
    Vibeless,
    Vibe,
    SuperVibe,
    Review,
}

impl<'de> Deserialize<'de> for VibeMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "vibe" => VibeMode::Vibe,
            "supervibe" => VibeMode::SuperVibe,
            _ => VibeMode::Vibeless,
        })
    }
}

impl VibeMode {
    pub fn display_name(&self) -> &str {
        match self {
            VibeMode::Vibeless => "Vibeless",
            VibeMode::Vibe => "Vibe",
            VibeMode::SuperVibe => "SuperVibe",
            VibeMode::Review => "Review",
        }
    }

    pub fn cli_flags(&self, enable_chrome: bool) -> Vec<String> {
        let mut flags = match self {
            VibeMode::Vibeless => vec![],
            VibeMode::Vibe => {
                vec!["--permission-mode".into(), "acceptEdits".into()]
            }
            VibeMode::SuperVibe => {
                vec!["--dangerously-skip-permissions".into()]
            }
            VibeMode::Review => vec![],
        };
        if enable_chrome {
            flags.push("--chrome".into());
        }
        flags
    }

    pub const ALL: [VibeMode; 4] = [
        VibeMode::Vibeless,
        VibeMode::Vibe,
        VibeMode::SuperVibe,
        VibeMode::Review,
    ];
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub agent: AgentKind,
    #[serde(default)]
    pub enable_chrome: bool,
    #[serde(default)]
    pub has_notes: bool,
    pub status: ProjectStatus,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary_updated_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
}

impl Feature {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: String,
        branch: String,
        workdir: PathBuf,
        is_worktree: bool,
        mode: VibeMode,
        review: bool,
        agent: AgentKind,
        enable_chrome: bool,
        has_notes: bool,
    ) -> Self {
        let tmux_session = format!("amf-{}", name);
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
            agent,
            enable_chrome,
            has_notes,
            status: ProjectStatus::Stopped,
            created_at: now,
            last_accessed: now,
            summary: None,
            summary_updated_at: None,
            nickname: None,
        }
    }

    pub fn touch(&mut self) {
        self.last_accessed = Utc::now();
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
        }
    }

    /// Return the next tmux window name for a session of the
    /// given kind, avoiding collisions with existing windows.
    pub fn next_window_name(&self, kind: &SessionKind) -> String {
        let prefix = match kind {
            SessionKind::Claude => "claude",
            SessionKind::Opencode => "opencode",
            SessionKind::Codex => "codex",
            SessionKind::Terminal => "terminal",
            SessionKind::Nvim => "nvim",
            SessionKind::Vscode => "vscode",
            SessionKind::Custom => "custom",
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
        let window = self.next_window_name(&kind);
        let session = FeatureSession {
            id: Uuid::new_v4().to_string(),
            kind,
            label,
            tmux_window: window,
            claude_session_id: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
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
            created_at: Utc::now(),
            command,
            on_stop,
            pre_check,
            status_text: None,
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
    pub is_git: bool,
}

impl Project {
    pub fn new(name: String, repo: PathBuf, is_git: bool) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            repo,
            collapsed: false,
            features: Vec::new(),
            created_at: Utc::now(),
            is_git,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectStore {
    pub version: u32,
    pub projects: Vec<Project>,
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
                version: 4,
                projects: Vec::new(),
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
                let store = Self::migrate_from_v3(v3);
                store.save(path)?;
                Ok(store)
            }
            1 => {
                let v1: V1ProjectStore = serde_json::from_value(raw)
                    .with_context(|| "Failed to parse v1 project store")?;
                let v2 = Self::migrate_from_v1(v1);
                let v3 = Self::migrate_from_v2(v2);
                let store = Self::migrate_from_v3(v3);
                store.save(path)?;
                Ok(store)
            }
            2 => {
                let v2: ProjectStore =
                    serde_json::from_value(raw).with_context(|| "Failed to parse project store")?;
                let v3 = Self::migrate_from_v2(v2);
                let store = Self::migrate_from_v3(v3);
                store.save(path)?;
                Ok(store)
            }
            3 => {
                let v3: ProjectStore = serde_json::from_value(raw)
                    .with_context(|| "Failed to parse v3 project store")?;
                let store = Self::migrate_from_v3(v3);
                store.save(path)?;
                Ok(store)
            }
            4 => {
                let store: ProjectStore = serde_json::from_value(raw)
                    .with_context(|| "Failed to parse v4 project store")?;
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
        }
    }

    fn migrate_from_v3(v3: ProjectStore) -> Self {
        // Add nickname field to features (serde default handles this)
        Self {
            version: 4,
            projects: v3.projects,
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
                                created_at: f.created_at,
                                command: None,
                                on_stop: None,
                                pre_check: None,
                                status_text: None,
                            },
                            FeatureSession {
                                id: Uuid::new_v4().to_string(),
                                kind: SessionKind::Terminal,
                                label: "Terminal 1".into(),
                                tmux_window: "terminal".into(),
                                claude_session_id: None,
                                created_at: f.created_at,
                                command: None,
                                on_stop: None,
                                pre_check: None,
                                status_text: None,
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
                            agent: AgentKind::default(),
                            enable_chrome: false,
                            has_notes: false,
                            status: f.status,
                            created_at: f.created_at,
                            last_accessed: f.last_accessed,
                            summary: None,
                            summary_updated_at: None,
                            nickname: None,
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
                    is_git: true,
                }
            })
            .collect();

        Self {
            version: 2,
            projects,
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
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
}

pub fn amf_config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config")
        .join("amf")
}

pub fn store_path() -> PathBuf {
    amf_config_dir().join("projects.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    fn make_feature_session(kind: SessionKind, window: &str) -> FeatureSession {
        FeatureSession {
            id: "test-id".to_string(),
            kind,
            label: "test".to_string(),
            tmux_window: window.to_string(),
            claude_session_id: None,
            created_at: Utc::now(),
            command: None,
            on_stop: None,
            pre_check: None,
            status_text: None,
        }
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
            agent: AgentKind::default(),
            enable_chrome: false,
            has_notes: false,
            status: ProjectStatus::Stopped,
            created_at: Utc::now(),
            last_accessed: Utc::now(),
            summary: None,
            summary_updated_at: None,
            nickname: None,
        }
    }

    // ── ProjectStore serialization round-trip ────────────────

    #[test]
    fn projectstore_roundtrip() {
        let store = ProjectStore {
            version: 4,
            projects: vec![Project {
                id: "proj-id".to_string(),
                name: "my-project".to_string(),
                repo: PathBuf::from("/home/user/my-project"),
                collapsed: false,
                features: vec![],
                created_at: Utc::now(),
                is_git: true,
            }],
        };
        let tmp = NamedTempFile::new().unwrap();
        store.save(tmp.path()).unwrap();

        let loaded = ProjectStore::load(tmp.path()).unwrap();
        assert_eq!(loaded.version, 4);
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].name, "my-project");
        assert_eq!(
            loaded.projects[0].repo,
            PathBuf::from("/home/user/my-project")
        );
        assert!(loaded.projects[0].is_git);
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
        assert_eq!(store.version, 4);
        assert_eq!(store.projects.len(), 1);

        let proj = &store.projects[0];
        // project name derived from repo basename
        assert_eq!(proj.name, "my-repo");
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
        assert_eq!(store.version, 4);
        assert_eq!(store.projects.len(), 1);

        let proj = &store.projects[0];
        assert_eq!(proj.name, "my-project");

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

    #[test]
    fn vibe_mode_vibeless_flags() {
        assert_eq!(VibeMode::Vibeless.cli_flags(false), Vec::<String>::new());
        assert_eq!(VibeMode::Vibeless.cli_flags(true), vec!["--chrome"]);
    }

    #[test]
    fn vibe_mode_vibe_flags() {
        assert_eq!(
            VibeMode::Vibe.cli_flags(false),
            vec!["--permission-mode", "acceptEdits"]
        );
        assert_eq!(
            VibeMode::Vibe.cli_flags(true),
            vec!["--permission-mode", "acceptEdits", "--chrome"]
        );
    }

    #[test]
    fn vibe_mode_supervibe_flags() {
        assert_eq!(
            VibeMode::SuperVibe.cli_flags(false),
            vec!["--dangerously-skip-permissions"]
        );
        assert_eq!(
            VibeMode::SuperVibe.cli_flags(true),
            vec!["--dangerously-skip-permissions", "--chrome"]
        );
    }

    #[test]
    fn vibe_mode_review_flags() {
        assert_eq!(VibeMode::Review.cli_flags(false), Vec::<String>::new());
        assert_eq!(VibeMode::Review.cli_flags(true), vec!["--chrome"]);
    }
}

pub fn migrate_from_old_path() {
    let new_path = store_path();
    if new_path.exists() {
        return;
    }

    let old_paths = vec![
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("amf")
            .join("projects.json"),
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("claude-super-vibeless")
            .join("projects.json"),
    ];

    for old_path in old_paths {
        if old_path.exists() {
            if let Some(parent) = new_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&old_path, &new_path);
            return;
        }
    }
}
