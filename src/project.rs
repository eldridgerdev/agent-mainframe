use anyhow::{Context, Result};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Feature {
    pub id: String,
    pub name: String,
    pub branch: String,
    pub workdir: PathBuf,
    pub is_worktree: bool,
    pub tmux_session: String,
    pub claude_session_id: Option<String>,
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
            claude_session_id: None,
            status: ProjectStatus::Stopped,
            created_at: now,
            last_accessed: now,
        }
    }

    pub fn touch(&mut self) {
        self.last_accessed = Utc::now();
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
}

impl Project {
    pub fn new(name: String, repo: PathBuf) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            repo,
            collapsed: false,
            features: Vec::new(),
            created_at: Utc::now(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectStore {
    pub version: u32,
    pub projects: Vec<Project>,
}

// Old flat format for migration
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
                version: 1,
                projects: Vec::new(),
            });
        }
        let data = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;

        // Check if it has a version field
        let raw: serde_json::Value = serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse {}", path.display()))?;

        if raw.get("version").is_some() {
            // New format
            let store: ProjectStore = serde_json::from_value(raw)
                .with_context(|| "Failed to parse v1 project store")?;
            Ok(store)
        } else {
            // Old flat format — migrate
            let old: OldProjectStore = serde_json::from_value(raw)
                .with_context(|| "Failed to parse old project store")?;
            let store = Self::migrate_from_old(old);
            // Re-save immediately in new format
            store.save(path)?;
            Ok(store)
        }
    }

    fn migrate_from_old(old: OldProjectStore) -> Self {
        let mut repo_groups: HashMap<PathBuf, Vec<OldProject>> = HashMap::new();
        for proj in old.projects {
            repo_groups
                .entry(proj.repo.clone())
                .or_default()
                .push(proj);
        }

        let mut projects = Vec::new();
        for (repo, old_projects) in repo_groups {
            // Use the first project's name as the group name,
            // or derive from repo path
            let project_name = repo
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "unnamed".into());

            let mut project = Project::new(project_name, repo);

            // Use earliest created_at from the group
            if let Some(earliest) = old_projects.iter().map(|p| p.created_at).min()
            {
                project.created_at = earliest;
            }

            for old_proj in old_projects {
                let branch = old_proj
                    .branch
                    .unwrap_or_else(|| "main".into());
                let mut feature = Feature {
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
                };
                // Preserve the existing tmux session name
                // (already set from old_proj.tmux_session)
                let _ = &mut feature;
                project.features.push(feature);
            }

            projects.push(project);
        }

        Self {
            version: 1,
            projects,
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        fs::write(path, data)
            .with_context(|| format!("Failed to write {}", path.display()))?;
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

    pub fn add_feature(
        &mut self,
        project_name: &str,
        feature: Feature,
    ) -> bool {
        if let Some(project) = self.find_project_mut(project_name) {
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
        if let Some(project) = self.find_project_mut(project_name) {
            if let Some(idx) =
                project.features.iter().position(|f| f.name == feature_name)
            {
                return Some(project.features.remove(idx));
            }
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
