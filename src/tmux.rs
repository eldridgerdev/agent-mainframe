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
    pub fn create_session(
        session: &str,
        workdir: &Path,
    ) -> Result<()> {
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
            .args([
                "select-window",
                "-t",
                &format!("{}:claude", session),
            ])
            .status()?;

        // Set status bar hint for navigating back
        Command::new("tmux")
            .args([
                "set-option",
                "-t",
                session,
                "status-right",
                " #[fg=cyan]prefix+s#[default]: sessions ",
            ])
            .status()?;

        Ok(())
    }

    /// Create a new tmux session with a single named first
    /// window.
    pub fn create_session_with_window(
        session: &str,
        first_window: &str,
        workdir: &Path,
    ) -> Result<()> {
        if Self::session_exists(session) {
            bail!("tmux session '{}' already exists", session);
        }

        let workdir_str = workdir.to_string_lossy();

        let status = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                session,
                "-n",
                first_window,
                "-c",
                &workdir_str,
            ])
            .status()
            .context("Failed to create tmux session")?;

        if !status.success() {
            bail!("tmux new-session failed");
        }

        // Set status bar hint
        Command::new("tmux")
            .args([
                "set-option",
                "-t",
                session,
                "status-right",
                " #[fg=cyan]prefix+s#[default]: sessions ",
            ])
            .status()?;

        Ok(())
    }

    /// Add a new window to an existing tmux session.
    pub fn create_window(
        session: &str,
        window_name: &str,
        workdir: &Path,
    ) -> Result<()> {
        let workdir_str = workdir.to_string_lossy();

        let status = Command::new("tmux")
            .args([
                "new-window",
                "-t",
                &format!("{}:", session),
                "-n",
                window_name,
                "-c",
                &workdir_str,
            ])
            .status()
            .context("Failed to create tmux window")?;

        if !status.success() {
            bail!("tmux new-window failed");
        }

        Ok(())
    }

    /// Select a specific window in a tmux session.
    pub fn select_window(
        session: &str,
        window: &str,
    ) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Command::new("tmux")
            .args(["select-window", "-t", &target])
            .status()
            .context("Failed to select tmux window")?;
        Ok(())
    }

    /// Kill a single window in a tmux session.
    pub fn kill_window(
        session: &str,
        window: &str,
    ) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Command::new("tmux")
            .args(["kill-window", "-t", &target])
            .status()
            .context("Failed to kill tmux window")?;
        Ok(())
    }

    /// List window names for a tmux session.
    pub fn list_windows(session: &str) -> Result<Vec<String>> {
        let output = Command::new("tmux")
            .args([
                "list-windows",
                "-t",
                session,
                "-F",
                "#{window_name}",
            ])
            .output()
            .context("Failed to list tmux windows")?;

        if !output.status.success() {
            return Ok(Vec::new());
        }

        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(String::from)
            .collect())
    }

    /// Launch Claude Code in a specific window of a session
    pub fn launch_claude(
        session: &str,
        window: &str,
        resume_session_id: Option<&str>,
        extra_args: &[&str],
    ) -> Result<()> {
        let target = format!("{}:{}", session, window);

        let mut cmd_str = String::from("claude");
        if let Some(sid) = resume_session_id {
            cmd_str.push_str(&format!(" --resume {}", sid));
        }
        for arg in extra_args {
            cmd_str.push(' ');
            cmd_str.push_str(arg);
        }

        Command::new("tmux")
            .args([
                "send-keys", "-t", &target, &cmd_str, "Enter",
            ])
            .status()
            .context("Failed to send claude command to tmux")?;

        Ok(())
    }

    /// Check if we're currently running inside a tmux session
    pub fn is_inside_tmux() -> bool {
        std::env::var("TMUX").is_ok()
    }

    /// Get the name of the current tmux session (only works inside tmux)
    pub fn current_session() -> Option<String> {
        let output = Command::new("tmux")
            .args(["display-message", "-p", "#{session_name}"])
            .output()
            .ok()?;
        if output.status.success() {
            let name = String::from_utf8_lossy(&output.stdout)
                .trim()
                .to_string();
            if name.is_empty() {
                None
            } else {
                Some(name)
            }
        } else {
            None
        }
    }

    /// Switch the tmux client to a different session (only works inside tmux)
    pub fn switch_client(session: &str) -> Result<()> {
        let status = Command::new("tmux")
            .args(["switch-client", "-t", session])
            .status()
            .context("Failed to switch tmux client")?;
        if !status.success() {
            bail!("tmux switch-client failed");
        }
        Ok(())
    }

    /// Attach to a session (replaces current terminal)
    pub fn attach_session(session: &str) -> Result<()> {
        if !Self::session_exists(session) {
            bail!(
                "tmux session '{}' does not exist",
                session
            );
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
                    .context(
                        "Failed to attach to tmux session",
                    )?;
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

    /// List all amf-* tmux sessions
    pub fn list_sessions() -> Result<Vec<String>> {
        let output = Command::new("tmux")
            .args(["list-sessions", "-F", "#{session_name}"])
            .output();

        match output {
            Ok(o) if o.status.success() => {
                let sessions: Vec<String> =
                    String::from_utf8_lossy(&o.stdout)
                        .lines()
                        .filter(|s| s.starts_with("amf-"))
                        .map(String::from)
                        .collect();
                Ok(sessions)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Capture the current pane content of a session's window
    pub fn capture_pane(
        session: &str,
        window: &str,
    ) -> Result<String> {
        let target = format!("{}:{}", session, window);
        let output = Command::new("tmux")
            .args(["capture-pane", "-t", &target, "-p"])
            .output()
            .context("Failed to capture pane")?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Capture pane content with ANSI escape sequences preserved
    pub fn capture_pane_ansi(
        session: &str,
        window: &str,
    ) -> Result<String> {
        let target = format!("{}:{}", session, window);
        let output = Command::new("tmux")
            .args([
                "capture-pane",
                "-t",
                &target,
                "-e",
                "-p",
            ])
            .output()
            .context("Failed to capture pane with ANSI")?;

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Get the cursor position (col, row) for a pane.
    pub fn cursor_position(
        session: &str,
        window: &str,
    ) -> Option<(u16, u16)> {
        let target = format!("{}:{}", session, window);
        let output = Command::new("tmux")
            .args([
                "display-message",
                "-t",
                &target,
                "-p",
                "#{cursor_x} #{cursor_y}",
            ])
            .output()
            .ok()?;
        let text =
            String::from_utf8_lossy(&output.stdout);
        let mut parts = text.trim().split(' ');
        let col: u16 = parts.next()?.parse().ok()?;
        let row: u16 = parts.next()?.parse().ok()?;
        Some((col, row))
    }

    /// Resize a tmux pane to match the TUI rendering area
    pub fn resize_pane(
        session: &str,
        window: &str,
        cols: u16,
        rows: u16,
    ) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Command::new("tmux")
            .args([
                "resize-window",
                "-t",
                &target,
                "-x",
                &cols.to_string(),
                "-y",
                &rows.to_string(),
            ])
            .status()
            .context("Failed to resize tmux pane")?;
        Ok(())
    }

    /// Send literal text to a tmux pane (no key name interpretation)
    pub fn send_literal(
        session: &str,
        window: &str,
        text: &str,
    ) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Command::new("tmux")
            .args(["send-keys", "-t", &target, "-l", text])
            .status()
            .context("Failed to send literal text to tmux")?;
        Ok(())
    }

    /// Send a named key (e.g. Enter, Up, BSpace) to a tmux pane
    pub fn send_key_name(
        session: &str,
        window: &str,
        key_name: &str,
    ) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Command::new("tmux")
            .args(["send-keys", "-t", &target, key_name])
            .status()
            .context("Failed to send key to tmux")?;
        Ok(())
    }

    /// Send keys to a specific window in a session
    pub fn send_keys(
        session: &str,
        window: &str,
        keys: &str,
    ) -> Result<()> {
        let target = format!("{}:{}", session, window);
        Command::new("tmux")
            .args(["send-keys", "-t", &target, keys, "Enter"])
            .status()
            .context("Failed to send keys to tmux")?;
        Ok(())
    }
}
