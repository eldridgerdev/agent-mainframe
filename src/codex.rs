use anyhow::{Context, Result};
use std::process::Command;

pub struct CodexLauncher;

impl CodexLauncher {
    /// Check if codex CLI is available
    pub fn check_available() -> Result<()> {
        let output = Command::new("codex")
            .arg("--version")
            .output()
            .context("codex CLI not found - is Codex installed?")?;

        if !output.status.success() {
            anyhow::bail!("codex CLI returned an error");
        }
        Ok(())
    }
}
