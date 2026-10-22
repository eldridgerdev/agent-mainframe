use anyhow::{Context, Result};
use std::process::Command;

pub struct PiLauncher;

impl PiLauncher {
    /// Check if pi CLI is available
    pub fn check_available() -> Result<()> {
        let output = Command::new("pi")
            .arg("--version")
            .output()
            .context("pi CLI not found - is Pi installed?")?;

        if !output.status.success() {
            return Err(crate::headless::version_check_error("pi", "pi", &output));
        }
        Ok(())
    }
}
