use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub repo: PathBuf,
    pub workdir: PathBuf,
    pub branch: Option<String>,
    pub is_worktree: bool,
    pub tmux_session: String,
    pub claude_session_id: Option<String>,
    pub status: ProjectStatus,
    pub created_at: DateTime<Utc>,
    pub last_accessed: DateTime<Utc>,
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

#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectStore {
    pub projects: Vec<Project>,
}

impl ProjectStore {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                projects: Vec::new(),
            });
        }
        let data = fs::read_to_string(path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        let store: ProjectStore = serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse {}", path.display()))?;
        Ok(store)
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

    pub fn add(&mut self, project: Project) {
        self.projects.push(project);
    }

    pub fn remove(&mut self, name: &str) -> Option<Project> {
        if let Some(idx) = self.projects.iter().position(|p| p.name == name) {
            Some(self.projects.remove(idx))
        } else {
            None
        }
    }

    pub fn find(&self, name: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.name == name)
    }

    pub fn find_mut(&mut self, name: &str) -> Option<&mut Project> {
        self.projects.iter_mut().find(|p| p.name == name)
    }

    pub fn has_project_in_repo(&self, repo: &Path) -> bool {
        self.projects.iter().any(|p| p.repo == repo)
    }

    pub fn list(&self) -> &[Project] {
        &self.projects
    }
}

impl Project {
    pub fn new(
        name: String,
        repo: PathBuf,
        workdir: PathBuf,
        branch: Option<String>,
        is_worktree: bool,
    ) -> Self {
        let tmux_session = format!("amf-{}", name);
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            repo,
            workdir,
            branch,
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

pub fn store_path() -> PathBuf {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("claude-super-vibeless");
    config_dir.join("projects.json")
}
