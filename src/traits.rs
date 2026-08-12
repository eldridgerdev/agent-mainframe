use anyhow::Result;
use std::path::{Path, PathBuf};

use crate::project::AgentKind;

/// Abstraction over tmux operations, enabling mocking in tests.
///
/// Methods mirror the corresponding `TmuxManager` statics. Using owned
/// `String` / `Vec<String>` for parameters that are lifetimed references
/// in the concrete implementation so that `mockall::automock` can derive
/// mock implementations without lifetime annotation complications.
#[cfg_attr(test, mockall::automock)]
pub trait TmuxOps: Send + Sync {
    fn check_harness_available(&self, kind: &AgentKind) -> Result<()>;
    fn session_exists(&self, session: &str) -> bool;
    fn window_exists(&self, session: &str, window: &str) -> bool;
    fn list_sessions(&self) -> Result<Vec<String>>;
    /// `(session, window, last-activity unix seconds)` for every window tmux
    /// knows about, in one call. Empty when tmux cannot be reached.
    fn window_activity(&self) -> Vec<(String, String, i64)>;
    /// `(session, window, pane pid)` for every pane on the server, in one
    /// call. The pid is the pane's own shell, not what it is running.
    fn list_panes(&self) -> Vec<(String, String, i64)>;
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
        feature_session_id: &str,
        resume_id: Option<String>,
        extra_args: Vec<String>,
    ) -> Result<()>;
    fn launch_opencode(&self, session: &str, window: &str, feature_session_id: &str) -> Result<()>;
    fn launch_opencode_with_session(
        &self,
        session: &str,
        window: &str,
        feature_session_id: &str,
        resume_id: Option<String>,
    ) -> Result<()>;
    fn launch_codex(
        &self,
        session: &str,
        window: &str,
        feature_session_id: &str,
        resume_id: Option<String>,
        extra_args: Vec<String>,
    ) -> Result<()>;
    fn launch_pi(&self, session: &str, window: &str, feature_session_id: &str) -> Result<()>;
    fn run_shell_command(&self, session: &str, window: &str, command: &str) -> Result<()>;
    fn send_keys(&self, session: &str, window: &str, keys: &str) -> Result<()>;
    fn send_literal(&self, session: &str, window: &str, text: &str) -> Result<()>;
    fn paste_text(&self, session: &str, window: &str, text: &str) -> Result<()>;
    fn send_key_name(&self, session: &str, window: &str, key_name: &str) -> Result<()>;
    fn resize_pane(&self, session: &str, window: &str, cols: u16, rows: u16) -> Result<()>;
    fn select_window(&self, session: &str, window: &str) -> Result<()>;
    fn kill_window(&self, session: &str, window: &str) -> Result<()>;
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
