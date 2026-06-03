use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use crate::tmux::TmuxManager;

pub struct ClaudeLauncher;

impl ClaudeLauncher {
    /// Find the newest working Claude Code binary.
    ///
    /// Scans ~/.local/share/claude/versions/ sorted by mtime (newest first)
    /// and returns the path of the first version that exits cleanly on
    /// --version. Falls back to "claude" (PATH lookup) if the versions
    /// directory doesn't exist or all candidates fail.
    pub fn resolve_binary() -> String {
        let Some(home) = dirs::home_dir() else {
            return "claude".to_string();
        };
        let versions_dir = home.join(".local/share/claude/versions");

        let Ok(entries) = std::fs::read_dir(&versions_dir) else {
            return "claude".to_string();
        };

        let mut candidates: Vec<(std::time::SystemTime, std::path::PathBuf)> = entries
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_file())
            .filter_map(|e| {
                let mtime = e.metadata().ok()?.modified().ok()?;
                Some((mtime, e.path()))
            })
            .collect();

        candidates.sort_by(|a, b| b.0.cmp(&a.0));

        for (_, path) in candidates {
            let ok = Command::new(&path)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                return path.to_string_lossy().into_owned();
            }
        }

        "claude".to_string()
    }

    /// Check if a working Claude CLI is available
    pub fn check_available() -> Result<()> {
        let binary = Self::resolve_binary();
        let output = Command::new(&binary)
            .arg("--version")
            .output()
            .context("claude CLI not found - is Claude Code installed?")?;

        if !output.status.success() {
            anyhow::bail!("claude CLI returned an error");
        }
        Ok(())
    }

    /// Launch Claude Code interactively in a tmux session window
    pub fn launch_interactive(session: &str, window: &str, resume_id: Option<&str>) -> Result<()> {
        TmuxManager::launch_claude(session, window, resume_id, &[])
    }

    /// Run a headless Claude command and return the output
    pub fn run_headless(workdir: &Path, prompt: &str) -> Result<String> {
        let binary = Self::resolve_binary();
        let output = Command::new(&binary)
            .args(["-p", prompt, "--output-format", "text"])
            .current_dir(workdir)
            .output()
            .context("Failed to run claude in headless mode")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("claude headless command failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub fn spawn_headless(workdir: &Path, prompt: &str) -> Result<Child> {
        let binary = Self::resolve_binary();
        Command::new(&binary)
            .args(["-p", prompt, "--output-format", "text"])
            .current_dir(workdir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn claude in headless mode")
    }

    /// Run a headless Claude command and return JSON output
    pub fn run_headless_json(workdir: &Path, prompt: &str) -> Result<String> {
        let binary = Self::resolve_binary();
        let output = Command::new(&binary)
            .args(["-p", prompt, "--output-format", "json"])
            .current_dir(workdir)
            .output()
            .context("Failed to run claude in headless mode")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("claude headless command failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
}
