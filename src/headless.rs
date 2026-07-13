use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::project::AgentKind;

/// Harness-agnostic entry point for one-shot, non-interactive agent work.
///
/// Prompts are always piped over stdin. Besides keeping secrets and large
/// prompts out of process listings, this avoids Linux's 128 KiB per-argument
/// limit for real-world PR diffs.
pub struct HeadlessRunner;

#[derive(Debug, PartialEq, Eq)]
struct HeadlessCommand {
    binary: String,
    args: Vec<&'static str>,
}

impl HeadlessRunner {
    pub fn check_available(harness: &AgentKind) -> Result<()> {
        match harness {
            AgentKind::Claude => crate::claude::ClaudeLauncher::check_available(),
            AgentKind::Codex => crate::codex::CodexLauncher::check_available(),
            AgentKind::Opencode => {
                let output = Command::new("opencode")
                    .arg("--version")
                    .output()
                    .context("opencode CLI not found - is Opencode installed?")?;
                if !output.status.success() {
                    anyhow::bail!("opencode CLI returned an error");
                }
                Ok(())
            }
            AgentKind::Pi => crate::pi::PiLauncher::check_available(),
        }
    }

    pub fn run(harness: &AgentKind, workdir: &Path, prompt: &str) -> Result<String> {
        let spec = command_for(harness);
        run_command(harness, &spec, workdir, prompt)
    }
}

fn run_command(
    harness: &AgentKind,
    spec: &HeadlessCommand,
    workdir: &Path,
    prompt: &str,
) -> Result<String> {
    let mut child = Command::new(&spec.binary)
        .args(&spec.args)
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| {
            format!(
                "Failed to spawn {} in headless mode",
                harness.display_name()
            )
        })?;

    // Drain output concurrently with the write. A large response can fill
    // stdout before the harness consumes all of a large prompt, otherwise
    // deadlocking a write-then-wait implementation.
    let mut stdin = child
        .stdin
        .take()
        .with_context(|| format!("Failed to open stdin for {}", harness.display_name()))?;
    let prompt = prompt.to_string();
    let writer = std::thread::spawn(move || stdin.write_all(prompt.as_bytes()));

    let output = child
        .wait_with_output()
        .with_context(|| format!("Failed to run {} headlessly", harness.display_name()))?;
    let write_result = writer
        .join()
        .map_err(|_| anyhow::anyhow!("{} prompt writer panicked", harness.display_name()))?;

    // Prefer the provider's exit error over a secondary BrokenPipe from the
    // writer: authentication/quota failures often make a CLI exit before it
    // consumes a large prompt, and its stderr is the actionable diagnosis.
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        anyhow::bail!(
            "{} headless command failed{}{}",
            harness.display_name(),
            if detail.is_empty() { "" } else { ": " },
            detail
        );
    }
    write_result.with_context(|| format!("Failed to send prompt to {}", harness.display_name()))?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn command_for(harness: &AgentKind) -> HeadlessCommand {
    match harness {
        AgentKind::Claude => HeadlessCommand {
            binary: crate::claude::ClaudeLauncher::resolve_binary(),
            args: vec!["-p", "--output-format", "text"],
        },
        AgentKind::Codex => HeadlessCommand {
            binary: "codex".into(),
            args: vec!["exec", "--color", "never", "-"],
        },
        AgentKind::Opencode => HeadlessCommand {
            binary: "opencode".into(),
            args: vec!["run"],
        },
        AgentKind::Pi => HeadlessCommand {
            binary: "pi".into(),
            args: vec!["-p"],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_agent_harness_has_a_headless_command() {
        let claude = command_for(&AgentKind::Claude);
        assert!(!claude.binary.is_empty());
        assert_eq!(claude.args, ["-p", "--output-format", "text"]);

        assert_eq!(
            command_for(&AgentKind::Codex),
            HeadlessCommand {
                binary: "codex".into(),
                args: vec!["exec", "--color", "never", "-"]
            }
        );
        assert_eq!(
            command_for(&AgentKind::Opencode),
            HeadlessCommand {
                binary: "opencode".into(),
                args: vec!["run"]
            }
        );
        assert_eq!(
            command_for(&AgentKind::Pi),
            HeadlessCommand {
                binary: "pi".into(),
                args: vec!["-p"]
            }
        );
    }

    #[test]
    fn runner_pipes_the_prompt_over_stdin() {
        let spec = HeadlessCommand {
            binary: "sh".into(),
            args: vec!["-c", "read input; printf 'received:%s' \"$input\""],
        };
        let output = run_command(&AgentKind::Codex, &spec, Path::new("/tmp"), "hello")
            .expect("fake headless command should succeed");
        assert_eq!(output, "received:hello");
    }

    #[test]
    fn runner_failure_names_the_provider_and_preserves_stderr() {
        let spec = HeadlessCommand {
            binary: "sh".into(),
            args: vec!["-c", "printf 'quota exhausted' >&2; exit 9"],
        };
        // Large enough that the early exit also breaks the stdin writer. The
        // provider's stderr must win over that secondary pipe error.
        let prompt = "x".repeat(1_000_000);
        let error = run_command(&AgentKind::Opencode, &spec, Path::new("/tmp"), &prompt)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Opencode headless command failed"));
        assert!(error.contains("quota exhausted"));
    }
}
