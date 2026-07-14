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

/// Long flags `command_for(Codex)` relies on. Older Codex releases reject
/// some of these with an argument-parse error, so headless availability must
/// probe `codex exec --help` for each — `codex --version` succeeding is not
/// enough.
const CODEX_EXEC_REQUIRED_FLAGS: [&str; 4] = [
    "--sandbox",
    "--ephemeral",
    "--skip-git-repo-check",
    "--color",
];

impl HeadlessRunner {
    pub fn check_available(harness: &AgentKind) -> Result<()> {
        match harness {
            AgentKind::Claude => crate::claude::ClaudeLauncher::check_available(),
            AgentKind::Codex => check_codex_headless_available(),
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

    /// Pick the engine for a plan interview.
    ///
    /// Prefer the feature's harness when `check_available` passes — i.e. its
    /// CLI is installed and responds, and for Codex the flags the headless
    /// command needs are advertised by `codex exec --help` — then fall back
    /// in a stable order. This does not exercise a real headless run, so a
    /// harness that is installed but misconfigured (e.g. not authenticated)
    /// can still be picked ahead of a working fallback. Pi is deliberately
    /// excluded until its headless contract is verified; a Pi feature can
    /// still be implemented by Pi while another harness powers discovery.
    #[allow(dead_code)] // Wired into the interview state machine by the next Epic 3 item.
    pub fn select_for_interview(preferred: &AgentKind) -> Option<AgentKind> {
        select_interview_harness_with(preferred, |harness| Self::check_available(harness).is_ok())
    }
}

fn check_codex_headless_available() -> Result<()> {
    crate::codex::CodexLauncher::check_available()?;
    let output = Command::new("codex")
        .args(["exec", "--help"])
        .output()
        .context("codex CLI not found - is Codex installed?")?;
    if !output.status.success() {
        anyhow::bail!(
            "installed codex CLI does not support `codex exec` - upgrade Codex to run it headlessly"
        );
    }
    let help = String::from_utf8_lossy(&output.stdout);
    let missing = missing_codex_exec_flags(&help);
    if !missing.is_empty() {
        anyhow::bail!(
            "installed codex CLI is too old for headless runs (`codex exec` lacks {}) - upgrade Codex",
            missing.join(", ")
        );
    }
    Ok(())
}

fn missing_codex_exec_flags(exec_help: &str) -> Vec<&'static str> {
    CODEX_EXEC_REQUIRED_FLAGS
        .iter()
        .copied()
        .filter(|flag| !exec_help.contains(flag))
        .collect()
}

/// Exhaustive so introducing a new `AgentKind` forces an explicit decision
/// on whether its headless contract is trusted for interviews.
fn supports_headless_interview(harness: &AgentKind) -> bool {
    match harness {
        AgentKind::Claude | AgentKind::Codex | AgentKind::Opencode => true,
        AgentKind::Pi => false,
    }
}

fn interview_candidates(preferred: &AgentKind) -> Vec<AgentKind> {
    let mut candidates = Vec::with_capacity(3);
    if supports_headless_interview(preferred) {
        candidates.push(preferred.clone());
    }
    for fallback in [AgentKind::Claude, AgentKind::Codex, AgentKind::Opencode] {
        if !candidates.contains(&fallback) {
            candidates.push(fallback);
        }
    }
    candidates
}

fn select_interview_harness_with(
    preferred: &AgentKind,
    mut is_available: impl FnMut(&AgentKind) -> bool,
) -> Option<AgentKind> {
    interview_candidates(preferred)
        .into_iter()
        .find(|harness| is_available(harness))
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
            args: vec![
                "exec",
                "--sandbox",
                "read-only",
                "--ephemeral",
                "--skip-git-repo-check",
                "--color",
                "never",
                "-",
            ],
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
                args: vec![
                    "exec",
                    "--sandbox",
                    "read-only",
                    "--ephemeral",
                    "--skip-git-repo-check",
                    "--color",
                    "never",
                    "-",
                ]
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

    #[test]
    fn codex_flag_probe_reports_only_missing_required_flags() {
        let full_help = "Options:\n  --sandbox <MODE>\n  --ephemeral\n  \
                         --skip-git-repo-check\n  --color <WHEN>";
        assert!(missing_codex_exec_flags(full_help).is_empty());

        let old_help = "Options:\n  --sandbox <MODE>\n  --color <WHEN>";
        assert_eq!(
            missing_codex_exec_flags(old_help),
            ["--ephemeral", "--skip-git-repo-check"]
        );
    }

    #[test]
    fn codex_required_flags_cover_everything_the_headless_command_passes() {
        let args = command_for(&AgentKind::Codex).args;
        for flag in CODEX_EXEC_REQUIRED_FLAGS {
            assert!(
                args.contains(&flag),
                "probe checks {flag} but the headless command no longer passes it"
            );
        }
        for arg in args.iter().filter(|arg| arg.starts_with("--")) {
            assert!(
                CODEX_EXEC_REQUIRED_FLAGS.contains(arg),
                "headless command passes {arg} but the availability probe never checks it"
            );
        }
    }

    #[test]
    fn interview_candidates_prefer_the_feature_harness_then_stable_fallbacks() {
        assert_eq!(
            interview_candidates(&AgentKind::Opencode),
            [AgentKind::Opencode, AgentKind::Claude, AgentKind::Codex]
        );
        assert_eq!(
            interview_candidates(&AgentKind::Codex),
            [AgentKind::Codex, AgentKind::Claude, AgentKind::Opencode]
        );
    }

    #[test]
    fn interview_candidates_skip_unverified_pi_headless_mode() {
        assert_eq!(
            interview_candidates(&AgentKind::Pi),
            [AgentKind::Claude, AgentKind::Codex, AgentKind::Opencode]
        );
    }

    #[test]
    fn interview_harness_selection_falls_back_or_returns_static_only() {
        let selected = select_interview_harness_with(&AgentKind::Codex, |harness| {
            harness == &AgentKind::Opencode
        });
        assert_eq!(selected, Some(AgentKind::Opencode));

        let selected = select_interview_harness_with(&AgentKind::Claude, |_| false);
        assert_eq!(selected, None);
    }
}
