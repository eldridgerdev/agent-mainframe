use crate::worktree::WorktreeManager;
use std::path::{Path, PathBuf};

pub fn shorten_path(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = path.strip_prefix(&home)
    {
        return format!("~/{}", rest.display());
    }
    path.display().to_string()
}

pub fn slugify(s: &str) -> String {
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

pub fn detect_repo_path() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    WorktreeManager::repo_root(&cwd)
        .unwrap_or(cwd)
        .to_string_lossy()
        .into_owned()
}

pub fn detect_branch() -> String {
    let cwd = std::env::current_dir().unwrap_or_default();
    WorktreeManager::current_branch(&cwd)
        .ok()
        .flatten()
        .unwrap_or_default()
}

pub fn latest_prompt_path(workdir: &Path) -> PathBuf {
    workdir.join(".claude").join("latest-prompt.txt")
}

pub fn read_latest_prompt(workdir: &Path) -> Option<String> {
    let paths = [
        latest_prompt_path(workdir),
        workdir.join(".codex").join("latest-prompt.txt"),
    ];

    paths
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .or_else(|| {
            super::codex_sessions::latest_prompt_for_workdir(workdir)
                .ok()
                .flatten()
        })
}
