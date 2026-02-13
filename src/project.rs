use anyhow::{bail, Context, Result};
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
    Terminal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureSession {
    pub id: String,
    pub kind: SessionKind,
    pub label: String,
    pub tmux_window: String,
    pub claude_session_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(
    Debug, Clone, Serialize, Deserialize, PartialEq, Default,
)]
#[serde(rename_all = "lowercase")]
pub enum VibeMode {
    #[default]
    Vibeless,
    Vibe,
    SuperVibe,
}

impl VibeMode {
    pub fn display_name(&self) -> &str {
        match self {
            VibeMode::Vibeless => "Vibeless",
            VibeMode::Vibe => "Vibe",
            VibeMode::SuperVibe => "SuperVibe",
        }
    }

    pub fn cli_flags(&self) -> Vec<&str> {
        match self {
            VibeMode::Vibeless => vec![],
            VibeMode::Vibe => {
                vec!["--permission-mode", "acceptEdits"]
            }
            VibeMode::SuperVibe => {
                vec!["--dangerously-skip-permissions"]
            }
        }
    }

    pub const ALL: [VibeMode; 3] = [
        VibeMode::Vibeless,
        VibeMode::Vibe,
        VibeMode::SuperVibe,
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
    pub status: ProjectStatus,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
}

impl Feature {
    pub fn new(
        name: String,
        branch: String,
        workdir: PathBuf,
        is_worktree: bool,
        mode: VibeMode,
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
            status: ProjectStatus::Stopped,
            created_at: now,
            last_accessed: now,
        }
    }

    pub fn touch(&mut self) {
        self.last_accessed = Utc::now();
    }

    /// Return the next label for a session of the given kind.
    pub fn next_label(&self, kind: &SessionKind) -> String {
        let count = self
            .sessions
            .iter()
            .filter(|s| s.kind == *kind)
            .count();
        match kind {
            SessionKind::Claude => format!("Claude {}", count + 1),
            SessionKind::Terminal => {
                format!("Terminal {}", count + 1)
            }
        }
    }

    /// Return the next tmux window name for a session of the
    /// given kind, avoiding collisions with existing windows.
    pub fn next_window_name(&self, kind: &SessionKind) -> String {
        let prefix = match kind {
            SessionKind::Claude => "claude",
            SessionKind::Terminal => "terminal",
        };
        let count = self
            .sessions
            .iter()
            .filter(|s| s.kind == *kind)
            .count();
        if count == 0 {
            prefix.to_string()
        } else {
            let mut n = count + 1;
            loop {
                let candidate = format!("{}-{}", prefix, n);
                if !self
                    .sessions
                    .iter()
                    .any(|s| s.tmux_window == candidate)
                {
                    return candidate;
                }
                n += 1;
            }
        }
    }

    /// Create and append a new session of the given kind.
    pub fn add_session(
        &mut self,
        kind: SessionKind,
    ) -> &FeatureSession {
        let label = self.next_label(&kind);
        let window = self.next_window_name(&kind);
        let session = FeatureSession {
            id: Uuid::new_v4().to_string(),
            kind,
            label,
            tmux_window: window,
            claude_session_id: None,
            created_at: Utc::now(),
        };
        self.sessions.push(session);
        self.sessions.last().unwrap()
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
    pub fn new(
        name: String,
        repo: PathBuf,
        is_git: bool,
    ) -> Self {
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
                version: 2,
                projects: Vec::new(),
            });
        }
        let data = fs::read_to_string(path)
            .with_context(|| {
                format!("Failed to read {}", path.display())
            })?;

        let raw: serde_json::Value =
            serde_json::from_str(&data).with_context(|| {
                format!("Failed to parse {}", path.display())
            })?;

        let version = raw
            .get("version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        match version {
            0 => {
                // Old flat format -> v1 intermediary -> v2
                let old: OldProjectStore =
                    serde_json::from_value(raw).with_context(|| {
                        "Failed to parse old project store"
                    })?;
                let v1 = Self::migrate_from_old(old);
                let store = Self::migrate_from_v1(v1);
                store.save(path)?;
                Ok(store)
            }
            1 => {
                let v1: V1ProjectStore =
                    serde_json::from_value(raw).with_context(|| {
                        "Failed to parse v1 project store"
                    })?;
                let store = Self::migrate_from_v1(v1);
                store.save(path)?;
                Ok(store)
            }
            2 => {
                let store: ProjectStore =
                    serde_json::from_value(raw).with_context(|| {
                        "Failed to parse v2 project store"
                    })?;
                Ok(store)
            }
            _ => {
                bail!(
                    "Unknown project store version: {}",
                    version
                );
            }
        }
    }

    /// Migrate from old flat format to v1 intermediary.
    fn migrate_from_old(old: OldProjectStore) -> V1ProjectStore {
        let mut repo_groups: HashMap<PathBuf, Vec<OldProject>> =
            HashMap::new();
        for proj in old.projects {
            repo_groups
                .entry(proj.repo.clone())
                .or_default()
                .push(proj);
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
                    let branch = old_proj
                        .branch
                        .unwrap_or_else(|| "main".into());
                    V1Feature {
                        id: Uuid::new_v4().to_string(),
                        name: old_proj.name,
                        branch,
                        workdir: old_proj.workdir,
                        is_worktree: old_proj.is_worktree,
                        tmux_session: old_proj.tmux_session,
                        claude_session_id: old_proj
                            .claude_session_id,
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
                                claude_session_id: f
                                    .claude_session_id,
                                created_at: f.created_at,
                            },
                            FeatureSession {
                                id: Uuid::new_v4().to_string(),
                                kind: SessionKind::Terminal,
                                label: "Terminal 1".into(),
                                tmux_window: "terminal".into(),
                                claude_session_id: None,
                                created_at: f.created_at,
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
                            status: f.status,
                            created_at: f.created_at,
                            last_accessed: f.last_accessed,
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
        fs::write(path, data).with_context(|| {
            format!("Failed to write {}", path.display())
        })?;
        Ok(())
    }

    pub fn add_project(&mut self, project: Project) {
        self.projects.push(project);
    }

    pub fn remove_project(
        &mut self,
        name: &str,
    ) -> Option<Project> {
        if let Some(idx) =
            self.projects.iter().position(|p| p.name == name)
        {
            Some(self.projects.remove(idx))
        } else {
            None
        }
    }

    pub fn find_project(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.name == name)
    }

    pub fn find_project_mut(
        &mut self,
        name: &str,
    ) -> Option<&mut Project> {
        self.projects.iter_mut().find(|p| p.name == name)
    }

    pub fn add_feature(
        &mut self,
        project_name: &str,
        feature: Feature,
    ) -> bool {
        if let Some(project) =
            self.find_project_mut(project_name)
        {
            project.features.push(feature);
            true
        } else {
            false
        }
    }

    pub fn remove_feature(
        &mut self,
        project_name: &str,
        feature_name: &str,
    ) -> Option<Feature> {
        if let Some(project) =
            self.find_project_mut(project_name)
            && let Some(idx) = project
                .features
                .iter()
                .position(|f| f.name == feature_name)
            {
                return Some(project.features.remove(idx));
            }
        None
    }
}

pub fn store_path() -> PathBuf {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claude-super-vibeless");
    config_dir.join("projects.json")
}
