use anyhow::{Context, Result, bail};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use crate::traits::WorktreeOps;

pub struct WorktreeManager;

pub struct SpawnedWorktreeCommand {
    pub child: Child,
    pub output_rx: Receiver<String>,
}

impl WorktreeManager {
    /// Create a checkout for a fork without modifying its contents. Setup must
    /// happen after the approved source snapshot has been transferred.
    pub fn create_fork_base(
        repo: &Path,
        name: &str,
        branch: &str,
        commit: &str,
    ) -> Result<PathBuf> {
        use std::path::Component;
        if !matches!(commit.len(), 40 | 64) || !commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            bail!("Fork base must be an exact Git commit ID");
        }
        let repo = repo
            .canonicalize()
            .context("Cannot resolve project checkout")?;
        let relative = Path::new(name);
        if relative.components().next().is_none()
            || relative
                .components()
                .any(|c| !matches!(c, Component::Normal(n) if n != ".git"))
        {
            bail!("Invalid fork checkout name: {name:?}");
        }
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        use std::os::unix::fs::MetadataExt;
        let mut parent = repo.join(".worktrees");
        let mut directory = std::fs::File::open(&repo)?;
        let components = std::iter::once(std::ffi::OsStr::new(".worktrees")).chain(
            relative
                .parent()
                .into_iter()
                .flat_map(|p| p.components())
                .map(|c| c.as_os_str()),
        );
        for component in components {
            let component = std::ffi::CString::new(component.as_bytes())?;
            let rc = unsafe { libc::mkdirat(directory.as_raw_fd(), component.as_ptr(), 0o755) };
            if rc < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(err).context("Cannot create fork checkout parent");
                }
            }
            let fd = unsafe {
                libc::openat(
                    directory.as_raw_fd(),
                    component.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                )
            };
            if fd < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("Fork checkout parent is not a safe directory");
            }
            directory = unsafe { std::fs::File::from_raw_fd(fd) };
        }
        if let Some(nested) = relative.parent() {
            parent.push(nested);
        }
        let leaf = relative.file_name().context("Missing fork checkout name")?;
        let name = std::ffi::CString::new(leaf.as_bytes())?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        let rc = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if rc == 0 {
            bail!(
                "Fork checkout already exists: {}",
                parent.join(leaf).display()
            );
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::NotFound {
            return Err(err).context("Cannot inspect fork checkout path");
        }
        let path = parent.join(leaf);
        let parent_identity = directory.metadata()?;
        let parent_matches = || -> bool {
            std::fs::metadata(&parent)
                .is_ok_and(|m| m.dev() == parent_identity.dev() && m.ino() == parent_identity.ino())
        };
        if !parent_matches() {
            bail!(
                "Fork checkout parent changed before creation: {}",
                parent.display()
            );
        }
        let git_dir = Command::new("git")
            .args(["rev-parse", "--absolute-git-dir"])
            .current_dir(&repo)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_COMMON_DIR")
            .output()
            .context("Cannot find project Git directory")?;
        if !git_dir.status.success() {
            bail!(
                "Cannot find project Git directory: {}",
                String::from_utf8_lossy(&git_dir.stderr).trim()
            );
        }
        let mut git_dir = git_dir.stdout;
        if git_dir.pop() != Some(b'\n') {
            bail!("Invalid project Git directory response");
        }
        let git_dir = std::ffi::OsString::from_vec(git_dir);
        // chdir happens before close-on-exec, anchoring Git's relative output
        // path to the checked parent even if its pathname is replaced.
        let anchored_parent = format!("/dev/fd/{}", directory.as_raw_fd());
        let output = Command::new("git")
            .arg("--git-dir")
            .arg(git_dir)
            .args(["worktree", "add", "-b", branch])
            .arg("--")
            .arg(leaf)
            .arg(commit)
            .current_dir(anchored_parent)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_COMMON_DIR")
            .output()
            .context("Failed to run git worktree add")?;
        if !parent_matches() {
            bail!(
                "Fork checkout parent moved during creation; inspect Git worktrees for branch {branch:?} before retrying (original parent {})",
                parent.display()
            );
        }
        if !output.status.success() {
            let details = String::from_utf8_lossy(&output.stderr);
            if std::fs::symlink_metadata(&path).is_ok() {
                bail!(
                    "Git could not finish the fork checkout: {}. Checkout retained at {}; branch {branch:?} may exist",
                    details.trim(),
                    path.display()
                );
            }
            bail!(
                "Git could not create the fork checkout: {}. Branch {branch:?} may exist; inspect before retrying",
                details.trim()
            );
        }
        Ok(path)
    }
    fn stream_child_output<R: Read + Send + 'static>(reader: Option<R>, tx: mpsc::Sender<String>) {
        if let Some(reader) = reader {
            std::thread::spawn(move || {
                for line in BufReader::new(reader)
                    .lines()
                    .map_while(std::result::Result::ok)
                {
                    let _ = tx.send(line);
                }
            });
        }
    }

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

        let root = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(PathBuf::from(root))
    }

    /// Check if a path is a git worktree (not the main working tree)
    pub fn is_worktree(path: &Path) -> bool {
        let output = Command::new("git")
            .args(["rev-parse", "--git-common-dir"])
            .current_dir(path)
            .output();

        let common = match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => return false,
        };

        let output = Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(path)
            .output();

        let gitdir = match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => return false,
        };

        common != gitdir
    }

    /// Create a new worktree for a branch
    pub fn create(repo: &Path, name: &str, branch: &str) -> Result<PathBuf> {
        let worktree_dir = repo.join(".worktrees");
        std::fs::create_dir_all(&worktree_dir)?;

        let worktree_path = worktree_dir.join(name);

        if worktree_path.exists() {
            bail!("Worktree path already exists: {}", worktree_path.display());
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
                .args(["worktree", "add", &worktree_path.to_string_lossy(), branch])
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
        let src_settings = repo.join(".claude").join("settings.local.json");
        if src_settings.exists() {
            let dest_dir = worktree_path.join(".claude");
            let dest_settings = dest_dir.join("settings.local.json");
            std::fs::create_dir_all(&dest_dir)?;

            let src: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&src_settings)?)?;

            let mut dest: serde_json::Value = if dest_settings.exists() {
                std::fs::read_to_string(&dest_settings)
                    .ok()
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_else(|| serde_json::json!({}))
            } else {
                serde_json::json!({})
            };

            // Merge each top-level key from src into dest
            if let (Some(src_obj), Some(dest_obj)) = (src.as_object(), dest.as_object_mut()) {
                for (key, src_val) in src_obj {
                    let entry = dest_obj
                        .entry(key.clone())
                        .or_insert_with(|| serde_json::json!({}));
                    // Deep-merge objects, overwrite scalars
                    if let (Some(sv), Some(ev)) = (src_val.as_object(), entry.as_object_mut()) {
                        for (k, v) in sv {
                            ev.insert(k.clone(), v.clone());
                        }
                    } else {
                        *entry = src_val.clone();
                    }
                }
            }

            std::fs::write(&dest_settings, serde_json::to_string_pretty(&dest)? + "\n")?;
        }

        Ok(worktree_path)
    }

    /// Create a new worktree branched off a specific base branch/commit.
    /// Runs `git worktree add -b {new_branch} {path} {base}`.
    pub fn create_from(repo: &Path, name: &str, new_branch: &str, base: &str) -> Result<PathBuf> {
        let worktree_dir = repo.join(".worktrees");
        std::fs::create_dir_all(&worktree_dir)?;

        let worktree_path = worktree_dir.join(name);

        if worktree_path.exists() {
            bail!("Worktree path already exists: {}", worktree_path.display());
        }

        let output = Command::new("git")
            .args([
                "worktree",
                "add",
                "-b",
                new_branch,
                &worktree_path.to_string_lossy(),
                base,
            ])
            .current_dir(repo)
            .output()
            .context("Failed to create worktree from base")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("git worktree add failed: {}", stderr.trim());
        }

        // Git has already created these resources. A settings error must not
        // hide their identity or imply that retrying will start from scratch.
        (|| -> Result<()> {
            // Merge .claude/settings.local.json into new worktree
            let src_settings = repo.join(".claude").join("settings.local.json");
            if src_settings.exists() {
                let dest_dir = worktree_path.join(".claude");
                let dest_settings = dest_dir.join("settings.local.json");
                std::fs::create_dir_all(&dest_dir)?;

                let src: serde_json::Value =
                    serde_json::from_str(&std::fs::read_to_string(&src_settings)?)?;

                let mut dest: serde_json::Value = if dest_settings.exists() {
                    std::fs::read_to_string(&dest_settings)
                        .ok()
                        .and_then(|s| serde_json::from_str(&s).ok())
                        .unwrap_or_else(|| serde_json::json!({}))
                } else {
                    serde_json::json!({})
                };

                if let (Some(src_obj), Some(dest_obj)) = (src.as_object(), dest.as_object_mut()) {
                    for (key, src_val) in src_obj {
                        let entry = dest_obj
                            .entry(key.clone())
                            .or_insert_with(|| serde_json::json!({}));
                        if let (Some(sv), Some(ev)) = (src_val.as_object(), entry.as_object_mut()) {
                            for (k, v) in sv {
                                ev.insert(k.clone(), v.clone());
                            }
                        } else {
                            *entry = src_val.clone();
                        }
                    }
                }

                std::fs::write(&dest_settings, serde_json::to_string_pretty(&dest)? + "\n")?;
            }
            Ok(())
        })()
        .with_context(|| {
            format!(
                "Created checkout {} on branch {}, but settings setup failed. The checkout and branch are retained for manual recovery; inspect git worktree list before retrying.",
                worktree_path.display(), new_branch
            )
        })?;

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

    pub fn spawn_remove(repo: &Path, worktree_path: &Path) -> Result<SpawnedWorktreeCommand> {
        let mut child = Command::new("git")
            .args([
                "worktree",
                "remove",
                "--force",
                &worktree_path.to_string_lossy(),
            ])
            .current_dir(repo)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn git worktree remove")?;

        let (tx, rx) = mpsc::channel();
        Self::stream_child_output(child.stdout.take(), tx.clone());
        Self::stream_child_output(child.stderr.take(), tx);

        Ok(SpawnedWorktreeCommand {
            child,
            output_rx: rx,
        })
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

        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch.is_empty() {
            Ok(None)
        } else {
            Ok(Some(branch))
        }
    }

    /// Best-effort: make `workdir`'s repository ignore `probe` by adding
    /// `pattern` to `.git/info/exclude` (per-repo, untracked, shared by every
    /// worktree) rather than to a `.gitignore`, which is either tracked or one
    /// more untracked file for `git add -A` to pick up. Nothing happens when
    /// `probe` is already ignored or `workdir` is not a Git work tree.
    pub fn exclude_locally(workdir: &Path, probe: &str, pattern: &str) {
        let _ = Self::exclude_locally_checked(workdir, probe, pattern);
    }

    /// Like [`Self::exclude_locally`], but reports a failure to read or write
    /// the exclude file of a real work tree. A workdir that is not a Git work
    /// tree, or a probe already ignored, is still `Ok(())`.
    pub(crate) fn exclude_locally_checked(
        workdir: &Path,
        probe: &str,
        pattern: &str,
    ) -> Result<()> {
        let already_ignored = Command::new("git")
            .arg("-C")
            .arg(workdir)
            .args(["check-ignore", "-q", probe])
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if already_ignored {
            return Ok(());
        }

        // A workdir outside Git has no exclude file to maintain; that is not
        // a setup failure. Errors are reserved for a real work tree.
        let inside = Command::new("git")
            .arg("-C")
            .arg(workdir)
            .args(["rev-parse", "--is-inside-work-tree"])
            .stderr(Stdio::null())
            .output()?;
        if !inside.status.success() || inside.stdout.trim_ascii() != b"true" {
            return Ok(());
        }

        let output = Command::new("git")
            .arg("-C")
            .arg(workdir)
            .args(["rev-parse", "--git-path", "info/exclude"])
            .stderr(Stdio::null())
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "Cannot resolve Git exclude file for {}",
            workdir.display()
        );
        let rel = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if rel.is_empty() {
            return Ok(());
        }
        // `--git-path` prints relative to `-C`'s directory; a rare absolute result
        // replaces the join entirely, which is also correct.
        let exclude_path = workdir.join(rel);
        let mut contents = match std::fs::read_to_string(&exclude_path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => return Err(error.into()),
        };
        let wanted = pattern.trim_end_matches('/');
        if contents
            .lines()
            .any(|line| line.trim().trim_end_matches('/') == wanted)
        {
            return Ok(());
        }
        if !contents.is_empty() && !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str(pattern);
        contents.push('\n');
        if let Some(parent) = exclude_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&exclude_path, contents)?;
        Ok(())
    }
}

#[allow(dead_code)] // exercised only by unit tests
fn parse_primary_worktree_root(stdout: &str) -> Result<PathBuf> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("worktree ").map(PathBuf::from))
        .context("No worktree entries found")
}

#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>,
}

// ── WorktreeOps trait implementation ─────────────────────────────────────────

impl WorktreeOps for WorktreeManager {
    fn remove(&self, repo: &Path, worktree_path: &Path) -> Result<()> {
        WorktreeManager::remove(repo, worktree_path)
    }

    fn repo_root(&self, path: &Path) -> Result<PathBuf> {
        WorktreeManager::repo_root(path)
    }

    fn create(&self, repo: &Path, name: &str, branch: &str) -> Result<PathBuf> {
        WorktreeManager::create(repo, name, branch)
    }

    fn create_from(
        &self,
        repo: &Path,
        name: &str,
        new_branch: &str,
        base: &str,
    ) -> Result<PathBuf> {
        WorktreeManager::create_from(repo, name, new_branch, base)
    }

    fn create_fork_base(
        &self,
        repo: &Path,
        name: &str,
        branch: &str,
        commit: &str,
    ) -> Result<PathBuf> {
        WorktreeManager::create_fork_base(repo, name, branch, commit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fork_base_requires_exact_commit_and_creates_an_unconfigured_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let result = Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_AUTHOR_NAME", "Fixture")
                .env("GIT_AUTHOR_EMAIL", "fixture@example.test")
                .env("GIT_COMMITTER_NAME", "Fixture")
                .env("GIT_COMMITTER_EMAIL", "fixture@example.test")
                .output()
                .unwrap();
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            result.stdout
        };
        run(&["init", "-q"]);
        std::fs::write(dir.path().join("file"), b"head").unwrap();
        run(&["add", "file"]);
        run(&["commit", "-qm", "initial"]);
        let head = String::from_utf8(run(&["rev-parse", "HEAD"])).unwrap();
        let ops: &dyn WorktreeOps = &WorktreeManager;
        std::fs::create_dir(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(".claude/settings.local.json"), b"{}\n").unwrap();
        assert!(
            ops.create_fork_base(dir.path(), "bad", "bad", "HEAD")
                .is_err()
        );
        let workdir = ops
            .create_fork_base(dir.path(), "nested/fork", "fork", head.trim())
            .unwrap();
        assert_eq!(std::fs::read(workdir.join("file")).unwrap(), b"head");
        assert!(!workdir.join(".claude/settings.local.json").exists());
        assert!(
            ops.create_fork_base(dir.path(), "nested/fork", "other", head.trim())
                .is_err()
        );
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join(".worktrees/escape")).unwrap();
        assert!(
            ops.create_fork_base(dir.path(), "escape/fork", "escape", head.trim())
                .is_err()
        );
        assert!(!outside.path().join("fork").exists());
    }

    #[test]
    fn parse_primary_worktree_root_returns_first_worktree_entry() {
        let stdout = "\
worktree /repo/main
HEAD abcdef
branch refs/heads/main

worktree /repo/.worktrees/feature
HEAD 123456
branch refs/heads/feature
";

        assert_eq!(
            parse_primary_worktree_root(stdout).unwrap(),
            PathBuf::from("/repo/main")
        );
    }
}
