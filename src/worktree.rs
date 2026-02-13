use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct WorktreeManager;

impl WorktreeManager {
    /// Check if a path is inside a git repository and return the repo root
    pub fn repo_root(path: &Path) -> Result<PathBuf> {
        let output = Command::new("git")
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(path)
            .output()
            .context("Failed to run git rev-parse")?;

        if !output.status.success() {
            bail!("{} is not inside a git repository", path.display());
        }

        let root = String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_string();
        Ok(PathBuf::from(root))
    }

    /// Check if a path is a git worktree (not the main working tree)
    pub fn is_worktree(path: &Path) -> bool {
        let output = Command::new("git")
            .args(["rev-parse", "--git-common-dir"])
            .current_dir(path)
            .output();

        let common = match output {
            Ok(o) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            }
            _ => return false,
        };

        let output = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(path)
            .output();

        let gitdir = match output {
            Ok(o) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).trim().to_string()
            }
            _ => return false,
        };

        common != gitdir
    }

    /// Create a new worktree for a branch
    pub fn create(
        repo: &Path,
        name: &str,
        branch: &str,
    ) -> Result<PathBuf> {
        let worktree_dir = repo.join(".worktrees");
        std::fs::create_dir_all(&worktree_dir)?;

        let worktree_path = worktree_dir.join(name);

        if worktree_path.exists() {
            bail!(
                "Worktree path already exists: {}",
                worktree_path.display()
            );
        }

        // Check if branch exists
        let branch_exists = Command::new("git")
            .args(["rev-parse", "--verify", branch])
            .current_dir(repo)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        let output = if branch_exists {
            Command::new("git")
                .args([
                    "worktree",
                    "add",
                    &worktree_path.to_string_lossy(),
                    branch,
                ])
                .current_dir(repo)
                .output()
                .context("Failed to create worktree")?
        } else {
            // Create new branch
            Command::new("git")
                .args([
                    "worktree",
                    "add",
                    "-b",
                    branch,
                    &worktree_path.to_string_lossy(),
                ])
                .current_dir(repo)
                .output()
                .context("Failed to create worktree with new branch")?
        };

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git worktree add failed: {}", stderr.trim());
        }

        // Merge .claude/settings.local.json into new worktree
        let src_settings =
            repo.join(".claude").join("settings.local.json");
        if src_settings.exists() {
            let dest_dir = worktree_path.join(".claude");
            let dest_settings = dest_dir.join("settings.local.json");
            std::fs::create_dir_all(&dest_dir)?;

            let src: serde_json::Value =
                serde_json::from_str(
                    &std::fs::read_to_string(&src_settings)?,
                )?;

            let mut dest: serde_json::Value =
                if dest_settings.exists() {
                    std::fs::read_to_string(&dest_settings)
                        .ok()
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_else(|| serde_json::json!({}))
                } else {
                    serde_json::json!({})
                };

            // Merge each top-level key from src into dest
            if let (Some(src_obj), Some(dest_obj)) =
                (src.as_object(), dest.as_object_mut())
            {
                for (key, src_val) in src_obj {
                    let entry = dest_obj
                        .entry(key.clone())
                        .or_insert_with(|| serde_json::json!({}));
                    // Deep-merge objects, overwrite scalars
                    if let (Some(sv), Some(ev)) = (
                        src_val.as_object(),
                        entry.as_object_mut(),
                    ) {
                        for (k, v) in sv {
                            ev.insert(k.clone(), v.clone());
                        }
                    } else {
                        *entry = src_val.clone();
                    }
                }
            }

            std::fs::write(
                &dest_settings,
                serde_json::to_string_pretty(&dest)? + "\n",
            )?;
        }

        Ok(worktree_path)
    }

    /// Remove a worktree
    pub fn remove(repo: &Path, worktree_path: &Path) -> Result<()> {
        let output = Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                &worktree_path.to_string_lossy(),
            ])
            .current_dir(repo)
            .output()
            .context("Failed to remove worktree")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!(
                "git worktree remove failed for {}: {}",
                worktree_path.display(),
                stderr.trim()
            );
        }

        Ok(())
    }

    /// List worktrees for a repo
    pub fn list(repo: &Path) -> Result<Vec<WorktreeInfo>> {
        let output = Command::new("git")
            .args(["worktree", "list", "--porcelain"])
            .current_dir(repo)
            .output()
            .context("Failed to list worktrees")?;

        if !output.status.success() {
            bail!("git worktree list failed");
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut worktrees = Vec::new();
        let mut current_path: Option<PathBuf> = None;
        let mut current_branch: Option<String> = None;

        for line in stdout.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                if let Some(prev_path) = current_path.take() {
                    worktrees.push(WorktreeInfo {
                        path: prev_path,
                        branch: current_branch.take(),
                    });
                }
                current_path = Some(PathBuf::from(path));
            } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                current_branch = Some(branch.to_string());
            }
        }

        if let Some(path) = current_path {
            worktrees.push(WorktreeInfo {
                path,
                branch: current_branch,
            });
        }

        Ok(worktrees)
    }

    /// Get the current branch name at a path
    pub fn current_branch(path: &Path) -> Result<Option<String>> {
        let output = Command::new("git")
            .args(["branch", "--show-current"])
            .current_dir(path)
            .output()
            .context("Failed to get current branch")?;

        if !output.status.success() {
            return Ok(None);
        }

        let branch = String::from_utf8_lossy(&output.stdout)
            .trim()
            .to_string();
        if branch.is_empty() {
            Ok(None)
        } else {
            Ok(Some(branch))
        }
    }
}

#[derive(Debug)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>,
}
