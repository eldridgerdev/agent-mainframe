use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

pub struct ClaudeLauncher;

/// Minimum Claude Code version that supports Remote Control
/// (research preview landed in v2.1.51).
pub const MIN_REMOTE_CONTROL_VERSION: (u32, u32, u32) = (2, 1, 51);

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

        candidates.sort_by_key(|candidate| std::cmp::Reverse(candidate.0));

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

    /// Parse a `claude --version` string into a `(major, minor, patch)`
    /// triple. Tolerates trailing text (`"2.1.172 (Claude Code)"`) and
    /// pre-release suffixes (`"2.1.51-beta"`), returning `None` if no
    /// semver-looking token is present.
    pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
        s.split_whitespace().find_map(Self::parse_semver_token)
    }

    fn parse_semver_token(token: &str) -> Option<(u32, u32, u32)> {
        let mut parts = token.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        // The patch component may carry a pre-release suffix (e.g. "51-beta");
        // keep the leading digits only.
        let patch_digits: String = parts
            .next()?
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let patch = patch_digits.parse().ok()?;
        Some((major, minor, patch))
    }

    /// Read the installed Claude Code version, or `None` if it can't be
    /// determined (binary missing, error, or unparseable output).
    pub fn version() -> Option<(u32, u32, u32)> {
        let binary = Self::resolve_binary();
        let output = Command::new(&binary).arg("--version").output().ok()?;
        if !output.status.success() {
            return None;
        }
        Self::parse_version(&String::from_utf8_lossy(&output.stdout))
    }

    fn env_truthy(var: &str) -> bool {
        std::env::var(var)
            .map(|v| {
                let v = v.trim();
                !v.is_empty() && v != "0" && !v.eq_ignore_ascii_case("false")
            })
            .unwrap_or(false)
    }

    /// Detect a third-party inference provider forced via environment
    /// variables. Remote Control requires Anthropic's first-party
    /// claude.ai backend, so these unambiguously block it. Returns a
    /// short human-readable provider label when one is active.
    ///
    /// Note: an `ANTHROPIC_API_KEY` in the environment is intentionally
    /// *not* treated as a block here — Claude Code prefers claude.ai OAuth
    /// when the user has run `/login`, so the key's mere presence is not a
    /// reliable signal. If the session truly resolves to an API key, Claude
    /// surfaces its own "requires a claude.ai subscription" error.
    pub fn remote_control_provider_block() -> Option<&'static str> {
        const PROVIDERS: &[(&str, &str)] = &[
            ("CLAUDE_CODE_USE_BEDROCK", "AWS Bedrock"),
            ("CLAUDE_CODE_USE_VERTEX", "Google Vertex AI"),
            ("CLAUDE_CODE_USE_FOUNDRY", "Azure AI Foundry"),
        ];
        PROVIDERS
            .iter()
            .find(|(var, _)| Self::env_truthy(var))
            .map(|(_, label)| *label)
    }

    /// Resolve why Remote Control is unavailable, or `None` if it can be
    /// offered. `zai_configured` is supplied by the caller because the
    /// z.ai endpoint lives in AMF's own config rather than the environment.
    ///
    /// Checks, in order: z.ai / third-party provider config, then the
    /// installed Claude Code version. An undetectable version does not
    /// block (we prefer letting Claude report a precise error over a false
    /// negative that hides the feature).
    pub fn remote_control_block_reason(zai_configured: bool) -> Option<String> {
        if zai_configured {
            return Some("Unavailable with z.ai provider".to_string());
        }
        if let Some(provider) = Self::remote_control_provider_block() {
            return Some(format!("Unavailable with {provider}"));
        }
        match Self::version() {
            Some(v) if v >= MIN_REMOTE_CONTROL_VERSION => None,
            Some((a, b, c)) => Some(format!(
                "Requires Claude Code v{}.{}.{}+ (have {a}.{b}.{c})",
                MIN_REMOTE_CONTROL_VERSION.0,
                MIN_REMOTE_CONTROL_VERSION.1,
                MIN_REMOTE_CONTROL_VERSION.2,
            )),
            None => None,
        }
    }

    /// Run a headless Claude command and return the output.
    ///
    /// The prompt is piped over stdin rather than passed as a `-p <prompt>`
    /// argument: Linux caps a single `argv` element at `MAX_ARG_STRLEN`
    /// (128 KiB), well under what a real PR diff or file review routinely
    /// runs to, and exceeding it fails the whole spawn with `E2BIG` before
    /// `claude` ever sees the request. Piping has no comparable ceiling.
    pub fn run_headless(workdir: &Path, prompt: &str) -> Result<String> {
        // A headless summary run is a full Claude process like any other, so
        // it counts toward the agent concurrency limit while it lasts.
        let _lease = crate::resources::limits::HeadlessLease::acquire();
        let binary = Self::resolve_binary();
        let mut child = Command::new(&binary)
            .args(["-p", "--output-format", "text"])
            .current_dir(workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn claude in headless mode")?;

        // Write on a separate thread and let `wait_with_output` drain
        // stdout/stderr concurrently: writing the whole prompt first and only
        // then reading output risks a classic pipe deadlock if a large
        // response fills the stdout pipe buffer before `claude` has consumed
        // all of stdin.
        let mut stdin = child
            .stdin
            .take()
            .context("Failed to open stdin for claude")?;
        let prompt = prompt.to_string();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(prompt.as_bytes());
            // `stdin` drops here, closing the pipe so `claude` sees EOF.
        });

        let output = child
            .wait_with_output()
            .context("Failed to run claude in headless mode")?;
        let _ = writer.join();

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("claude headless command failed: {}", stderr);
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Spawn a non-blocking headless Claude command; callers poll the
    /// returned `Child`'s piped stdout/stderr for the result. See
    /// [`Self::run_headless`] for why the prompt is piped over stdin instead
    /// of passed as an argument. The write happens on a detached thread so
    /// this stays non-blocking — the child's stdout isn't read until the
    /// caller starts polling, so a synchronous write-then-return here would
    /// risk the same pipe deadlock `run_headless` avoids.
    ///
    /// `model`, when set, is passed as `--model <name>` so a bounded,
    /// single-purpose pass (a per-file walkthrough, an AI co-review) can run
    /// on a cheaper model than whatever the interactive session uses,
    /// independent of the feature's own harness/model.
    pub fn spawn_headless(
        workdir: &Path,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<crate::headless::LeasedChild> {
        let binary = Self::resolve_binary();
        let mut args = vec![
            "-p".to_string(),
            "--output-format".to_string(),
            "text".to_string(),
        ];
        if let Some(model) = model {
            args.push("--model".to_string());
            args.push(model.to_string());
        }
        let mut child = Command::new(&binary)
            .args(&args)
            .current_dir(workdir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Failed to spawn claude in headless mode")?;

        if let Some(mut stdin) = child.stdin.take() {
            let prompt = prompt.to_string();
            std::thread::spawn(move || {
                let _ = stdin.write_all(prompt.as_bytes());
            });
        }

        // Wrapped so the run counts toward the agent concurrency limit for as
        // long as the caller holds the child, however that ends.
        Ok(crate::headless::LeasedChild::new(child))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_version_handles_trailing_text() {
        assert_eq!(
            ClaudeLauncher::parse_version("2.1.172 (Claude Code)"),
            Some((2, 1, 172))
        );
    }

    #[test]
    fn parse_version_handles_bare_semver() {
        assert_eq!(ClaudeLauncher::parse_version("2.1.51"), Some((2, 1, 51)));
    }

    #[test]
    fn parse_version_strips_prerelease_suffix() {
        assert_eq!(
            ClaudeLauncher::parse_version("2.1.51-beta.3"),
            Some((2, 1, 51))
        );
    }

    #[test]
    fn parse_version_returns_none_for_garbage() {
        assert_eq!(ClaudeLauncher::parse_version("not a version"), None);
        assert_eq!(ClaudeLauncher::parse_version("2.1"), None);
        assert_eq!(ClaudeLauncher::parse_version(""), None);
    }

    #[test]
    fn min_version_boundary_is_inclusive() {
        // The minimum supported version itself must satisfy the gate.
        assert!(MIN_REMOTE_CONTROL_VERSION >= MIN_REMOTE_CONTROL_VERSION);
        assert!((2, 1, 51) >= MIN_REMOTE_CONTROL_VERSION);
        assert!((2, 0, 99) < MIN_REMOTE_CONTROL_VERSION);
        assert!((2, 1, 50) < MIN_REMOTE_CONTROL_VERSION);
        assert!((2, 1, 172) >= MIN_REMOTE_CONTROL_VERSION);
        assert!((3, 0, 0) >= MIN_REMOTE_CONTROL_VERSION);
    }

    #[test]
    fn zai_blocks_remote_control() {
        assert_eq!(
            ClaudeLauncher::remote_control_block_reason(true).as_deref(),
            Some("Unavailable with z.ai provider")
        );
    }

    #[test]
    #[ignore = "manual verification only: shells out to a live `claude` binary"]
    fn run_headless_handles_a_prompt_over_the_argv_limit() {
        let big = "x ".repeat(100_000); // ~200KB, well past Linux's 128KB MAX_ARG_STRLEN
        let prompt = format!("Reply with exactly the word DONE and nothing else. Padding: {big}");
        let out = ClaudeLauncher::run_headless(std::path::Path::new("/tmp"), &prompt)
            .expect("run_headless should not fail on a large prompt");
        assert!(out.to_uppercase().contains("DONE"), "got: {out}");
    }
}
