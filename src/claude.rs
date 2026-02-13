use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

use crate::tmux::TmuxManager;

pub struct ClaudeLauncher;

impl ClaudeLauncher {
    /// Check if claude CLI is available
    pub fn check_available() -> Result<()> {
        let output = Command::new("claude")
            .arg("--version")
            .output()
            .context(
                "claude CLI not found - is Claude Code installed?",
            )?;

        if !output.status.success() {
            anyhow::bail!("claude CLI returned an error");
        }
        Ok(())
    }

    /// Launch Claude Code interactively in a tmux session window
    pub fn launch_interactive(
        session: &str,
        window: &str,
        resume_id: Option<&str>,
    ) -> Result<()> {
        TmuxManager::launch_claude(session, window, resume_id, &[])
    }

    /// Run a headless Claude command and return the output
    pub fn run_headless(
        workdir: &Path,
        prompt: &str,
    ) -> Result<String> {
        let output = Command::new("claude")
            .args(["-p", prompt, "--output-format", "text"])
            .current_dir(workdir)
            .output()
            .context(
                "Failed to run claude in headless mode",
            )?;

        if !output.status.success() {
            let stderr =
                String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "claude headless command failed: {}",
                stderr
            );
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Run a headless Claude command and return JSON output
    pub fn run_headless_json(
        workdir: &Path,
        prompt: &str,
    ) -> Result<String> {
        let output = Command::new("claude")
            .args(["-p", prompt, "--output-format", "json"])
            .current_dir(workdir)
            .output()
            .context(
                "Failed to run claude in headless mode",
            )?;

        if !output.status.success() {
            let stderr =
                String::from_utf8_lossy(&output.stderr);
            anyhow::bail!(
                "claude headless command failed: {}",
                stderr
            );
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}
