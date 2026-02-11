use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Command;

pub struct TmuxManager;

impl TmuxManager {
    /// Check if tmux is available
    pub fn check_available() -> Result<()> {
        let output = Command::new("tmux").arg("-V").output()?;
        if !output.status.success() {
            bail!("tmux is not installed or not in PATH");
        }
        Ok(())
    }

    /// Check if a tmux session exists
    pub fn session_exists(session: &str) -> bool {
        Command::new("tmux")
            .args(["has-session", "-t", session])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    /// Create a new tmux session with a Claude Code window and a terminal window
    pub fn create_session(session: &str, workdir: &Path) -> Result<()> {
        if Self::session_exists(session) {
            bail!("tmux session '{}' already exists", session);
        }

        let workdir_str = workdir.to_string_lossy();

        // Create detached session with first window named "claude"
        let status = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                session,
                "-n",
                "claude",
                "-c",
                &workdir_str,
            ])
            .status()
            .context("Failed to create tmux session")?;

        if !status.success() {
            bail!("tmux new-session failed");
        }

        // Create second window named "terminal"
        let status = Command::new("tmux")
            .args([
                "new-window",
                "-t",
                &format!("{}:", session),
                "-n",
                "terminal",
                "-c",
                &workdir_str,
            ])
            .status()
            .context("Failed to create terminal window")?;

        if !status.success() {
            bail!("tmux new-window failed");
        }

        // Select the first window (claude)
        Command::new("tmux")
            .args(["select-window", "-t", &format!("{}:claude", session)])
            .status()?;

        Ok(())
    }

    /// Launch Claude Code in the claude window of a session
    pub fn launch_claude(
        session: &str,
        resume_session_id: Option<&str>,
    ) -> Result<()> {
        let target = format!("{}:claude", session);

        let mut cmd_str = String::from("claude");
        if let Some(sid) = resume_session_id {
            cmd_str.push_str(&format!(" --resume {}", sid));
        }

        Command::new("tmux")
            .args(["send-keys", "-t", &target, &cmd_str, "Enter"])
            .status()
            .context("Failed to send claude command to tmux")?;

        Ok(())
    }

    /// Attach to a session (replaces current terminal)
    pub fn attach_session(session: &str) -> Result<()> {
        if !Self::session_exists(session) {
            bail!("tmux session '{}' does not exist", session);
        }

        let status = Command::new("tmux")
            .args(["switch-client", "-t", session])
            .status();

        // If switch-client fails (not inside tmux), try attach
        match status {
            Ok(s) if s.success() => Ok(()),
            _ => {
                Command::new("tmux")
                    .args(["attach-session", "-t", session])
                    .status()
                    .context("Failed to attach to tmux session")?;
                Ok(())
            }
        }
    }

    /// Kill a tmux session
    pub fn kill_session(session: &str) -> Result<()> {
        if !Self::session_exists(session) {
            return Ok(());
        }

        Command::new("tmux")
            .args(["kill-session", "-t", session])
            .status()
            .context("Failed to kill tmux session")?;

        Ok(())
    }

    /// List all csv-* tmux sessions
    pub fn list_sessions() -> Result<Vec<String>> {
        let output = Command::new("tmux")
            .args(["list-sessions", "-F", "#{session_name}"])
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let sessions: Vec<String> = String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter(|s| s.starts_with("csv-"))
                    .map(String::from)
                    .collect();
                Ok(sessions)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Capture the current pane content of a session's window
    pub fn capture_pane(session: &str, window: &str) -> Result<String> {
        let target = format!("{}:{}", session, window);
        let output = Command::new("tmux")
            .args(["capture-pane", "-t", &target, "-p"])
            .output()
            .context("Failed to capture pane")?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Send keys to a specific window in a session
    pub fn send_keys(session: &str, window: &str, keys: &str) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Command::new("tmux")
            .args(["send-keys", "-t", &target, keys, "Enter"])
            .status()
            .context("Failed to send keys to tmux")?;
        Ok(())
    }
}
