use anyhow::Result;
use std::path::{Path, PathBuf};

/// Abstraction over tmux operations, enabling mocking in tests.
///
/// Methods mirror the corresponding `TmuxManager` statics. Using owned
/// `String` / `Vec<String>` for parameters that are lifetimed references
/// in the concrete implementation so that `mockall::automock` can derive
/// mock implementations without lifetime annotation complications.
#[cfg_attr(test, mockall::automock)]
pub trait TmuxOps: Send + Sync {
    fn session_exists(&self, session: &str) -> bool;
    fn list_sessions(&self) -> Result<Vec<String>>;
    fn create_session_with_window(
        &self,
        session: &str,
        first_window: &str,
        workdir: &Path,
    ) -> Result<()>;
    fn set_session_env(&self, session: &str, key: &str, value: &str) -> Result<()>;
    fn create_window(&self, session: &str, window: &str, workdir: &Path) -> Result<()>;
    fn launch_claude(
        &self,
        session: &str,
        window: &str,
        resume_id: Option<String>,
        extra_args: Vec<String>,
    ) -> Result<()>;
    fn launch_opencode(&self, session: &str, window: &str) -> Result<()>;
    fn launch_opencode_with_session(
        &self,
        session: &str,
        window: &str,
        resume_id: Option<String>,
    ) -> Result<()>;
    fn launch_codex(&self, session: &str, window: &str, resume_id: Option<String>) -> Result<()>;
    fn send_keys(&self, session: &str, window: &str, keys: &str) -> Result<()>;
    fn send_literal(&self, session: &str, window: &str, text: &str) -> Result<()>;
    fn paste_text(&self, session: &str, window: &str, text: &str) -> Result<()>;
    fn send_key_name(&self, session: &str, window: &str, key_name: &str) -> Result<()>;
    fn resize_pane(&self, session: &str, window: &str, cols: u16, rows: u16) -> Result<()>;
    fn select_window(&self, session: &str, window: &str) -> Result<()>;
    fn kill_session(&self, session: &str) -> Result<()>;
}

/// Abstraction over git worktree operations, enabling mocking in tests.
#[cfg_attr(test, mockall::automock)]
pub trait WorktreeOps: Send + Sync {
    fn repo_root(&self, path: &Path) -> Result<PathBuf>;
    fn create(&self, repo: &Path, name: &str, branch: &str) -> Result<PathBuf>;
    fn create_from(
        &self,
        repo: &Path,
        name: &str,
        new_branch: &str,
        base: &str,
    ) -> Result<PathBuf> {
        // Default: fall back to create() ignoring base
        let _ = base;
        self.create(repo, name, new_branch)
    }
}
