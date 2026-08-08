use anyhow::{Context, Result};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};

use crate::project::AgentKind;
use crate::resources::limits::HeadlessLease;

/// A headless harness process whose lifetime outlives the call that started
/// it — the poll-driven runs (Final Review walkthroughs and co-reviews, the
/// changeset overview, the diff-review explanation) that park a `Child` in app
/// state and check on it each tick.
///
/// The concurrency lease rides along with the child instead of being scoped to
/// a function, so a run that is abandoned mid-flight releases its slot when the
/// state holding it is dropped.
#[derive(Debug)]
pub struct LeasedChild {
    child: Child,
    _lease: HeadlessLease,
}

impl LeasedChild {
    pub fn new(child: Child) -> Self {
        Self {
            child,
            _lease: HeadlessLease::acquire(),
        }
    }

    /// Non-blocking status check; `Ok(None)` while the run is still going.
    pub fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Collect the finished run's output, releasing the lease afterwards.
    pub fn wait_with_output(self) -> std::io::Result<Output> {
        let Self { child, _lease } = self;
        child.wait_with_output()
    }
}

/// Harness-agnostic entry point for one-shot, non-interactive agent work.
///
/// Prompts are always piped over stdin. Besides keeping secrets and large
/// prompts out of process listings, this avoids Linux's 128 KiB per-argument
/// limit for real-world PR diffs.
pub struct HeadlessRunner;

/// Sanitized progress emitted by a headless harness while it works. The
/// provider's raw event payload is deliberately not exposed: it may contain
/// the full prompt, reasoning text, or shell commands. Callers get enough
/// structure to prove the run is alive without leaking review input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeadlessProgress {
    Activity(String),
    Usage {
        input_tokens: u64,
        output_tokens: u64,
    },
}

#[derive(Debug, PartialEq, Eq)]
struct HeadlessCommand {
    binary: String,
    args: Vec<&'static str>,
    /// Args that must stay last (e.g. Codex's trailing `-` stdin marker). An
    /// optional `--model <name>` is inserted between `args` and `trailing`.
    trailing: Vec<&'static str>,
    /// Extra env vars for the spawned process (e.g. Opencode's
    /// `OPENCODE_PERMISSION` deny-all override for restricted runs).
    envs: Vec<(&'static str, &'static str)>,
}

/// Deny-all permission override for Opencode's restricted headless runs.
/// Traced the installed binary's config loader: `OPENCODE_PERMISSION` is
/// merged into the resolved permission set *after* project/global
/// `opencode.json`, so unlike a config-defined agent this can't be
/// re-loosened by repo-controlled config.
const OPENCODE_RESTRICTED_PERMISSION: &str = r#"{"*":"deny"}"#;

/// Read-only tool policy for user-directed plan revisions. The wildcard keeps
/// every unlisted tool (including edit, bash, web, and subagents) denied while
/// the four repository-inspection tools are explicitly enabled.
const OPENCODE_READ_ONLY_PERMISSION: &str =
    r#"{"*":"deny","read":"allow","glob":"allow","grep":"allow","list":"allow"}"#;

/// Long flags `command_for(Codex)` relies on. Older Codex releases reject
/// some of these with an argument-parse error, so headless availability must
/// probe `codex exec --help` for each — `codex --version` succeeding is not
/// enough.
const CODEX_EXEC_REQUIRED_FLAGS: [&str; 5] = [
    "--sandbox",
    "--ephemeral",
    "--skip-git-repo-check",
    "--color",
    "--json",
];

/// Long flags the Pi plan-interview commands rely on. Pi's headless mode was
/// originally left out of interview selection because its non-interactive and
/// permission contracts had not been verified. Requiring every flag used by
/// the restricted and read-only commands keeps older releases on the existing
/// fallback path instead of launching them with a weaker safety boundary.
const PI_HEADLESS_REQUIRED_FLAGS: [&str; 10] = [
    "--print",
    "--no-session",
    "--no-tools",
    "--tools",
    "--no-extensions",
    "--no-skills",
    "--no-prompt-templates",
    "--no-context-files",
    "--no-approve",
    "--model",
];

/// A flag a headless CLI must advertise in `--help` for structured progress
/// to be trusted. `value`, when set, must appear within
/// [`PROGRESS_FLAG_VALUE_WINDOW`] characters of `flag`'s occurrence — a
/// generic token like `json` or `stream-json` appearing anywhere else in the
/// help text (an unrelated flag, an example, a URL) must not count. The
/// window (rather than requiring the exact same line) tolerates clap-style
/// help that wraps a flag's accepted values onto their own line.
struct ProgressFlag {
    flag: &'static str,
    value: Option<&'static str>,
}

const PROGRESS_FLAG_VALUE_WINDOW: usize = 240;

const CLAUDE_PROGRESS_REQUIRED_FLAGS: [ProgressFlag; 2] = [
    ProgressFlag {
        flag: "--output-format",
        value: Some("stream-json"),
    },
    ProgressFlag {
        flag: "--verbose",
        value: None,
    },
];
const OPENCODE_PROGRESS_REQUIRED_FLAGS: [ProgressFlag; 1] = [ProgressFlag {
    flag: "--format",
    value: Some("json"),
}];
const PI_PROGRESS_REQUIRED_FLAGS: [ProgressFlag; 1] = [ProgressFlag {
    flag: "--mode",
    value: Some("json"),
}];

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

    /// AI Review depends on the harness's structured event mode in addition
    /// to its ordinary headless launcher. Probe that mode before any diff is
    /// fetched or paid work begins so older CLIs fail with an upgrade hint.
    pub fn check_progress_available(harness: &AgentKind) -> Result<()> {
        Self::check_available(harness)?;
        match harness {
            AgentKind::Claude => check_progress_flags(
                &crate::claude::ClaudeLauncher::resolve_binary(),
                &["--help"],
                &CLAUDE_PROGRESS_REQUIRED_FLAGS,
                "Claude",
            ),
            AgentKind::Codex => Ok(()),
            AgentKind::Opencode => check_progress_flags(
                "opencode",
                &["run", "--help"],
                &OPENCODE_PROGRESS_REQUIRED_FLAGS,
                "Opencode",
            ),
            AgentKind::Pi => {
                check_progress_flags("pi", &["--help"], &PI_PROGRESS_REQUIRED_FLAGS, "Pi")
            }
        }
    }

    /// `model`, when set, is passed as an explicit `--model <name>` (the
    /// flag name/format every harness shares) so a caller — e.g. PR
    /// Triage's AI review — can pick a model independent of whatever the
    /// feature's interactive session runs.
    ///
    /// `restricted`, when true, forces Claude, Opencode, and Pi into a no-tools
    /// invocation that repo-controlled config (settings files, hooks,
    /// plugins, MCP servers) cannot loosen — for callers whose prompt
    /// carries all the context it needs and expects a plain text answer, not
    /// a repo-exploring agent run. Codex is always sandboxed read-only
    /// already.
    pub fn run(
        harness: &AgentKind,
        workdir: &Path,
        prompt: &str,
        model: Option<&str>,
        restricted: bool,
    ) -> Result<String> {
        let spec = command_for(harness, restricted);
        run_command(harness, &spec, workdir, prompt, model)
    }

    /// Run a repository-aware pass with tools constrained to read-only access.
    /// This is intentionally separate from `run(..., restricted: true)`: most
    /// interview work receives all context in its prompt and needs no tools,
    /// while directed plan feedback may explicitly ask the agent to locate or
    /// verify code before revising the draft.
    pub fn run_read_only(
        harness: &AgentKind,
        workdir: &Path,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<String> {
        let spec = read_only_command_for(harness)?;
        run_command(harness, &spec, workdir, prompt, model)
    }

    /// Run a headless pass while reporting sanitized provider activity.
    ///
    /// Every built-in harness has a structured event mode. Provider payloads
    /// are reduced to the same sanitized activity/usage contract here; raw
    /// reasoning, commands, tool arguments, and prompt content never leave
    /// this module.
    pub fn run_with_progress(
        harness: &AgentKind,
        workdir: &Path,
        prompt: &str,
        model: Option<&str>,
        on_progress: impl Fn(HeadlessProgress) + Send + 'static,
    ) -> Result<String> {
        let spec = command_for(harness, false);
        run_jsonl_command(harness, &spec, workdir, prompt, model, on_progress)
    }

    /// Pick the engine for a plan interview.
    ///
    /// Prefer the feature's harness when the interview-specific availability
    /// check passes — i.e. its CLI is installed and responds, with required
    /// command and safety flags verified for Codex and Pi — then fall back in
    /// a stable order. This does not exercise a real headless run, so a
    /// harness that is installed but misconfigured (e.g. not authenticated)
    /// can still be picked ahead of a working fallback. Pi is selected only
    /// when its CLI advertises the complete restricted/read-only flag set;
    /// older Pi releases retain the stable fallback behavior.
    pub fn select_for_interview(preferred: &AgentKind) -> Option<AgentKind> {
        select_interview_harness_with(preferred, |harness| {
            check_interview_available(harness).is_ok()
        })
    }
}

fn check_progress_flags(
    binary: &str,
    help_args: &[&str],
    required: &[ProgressFlag],
    display_name: &str,
) -> Result<()> {
    let output = Command::new(binary)
        .args(help_args)
        .output()
        .with_context(|| format!("{display_name} CLI not found"))?;
    if !output.status.success() {
        anyhow::bail!("{display_name} CLI could not describe its headless mode");
    }
    let mut help = String::from_utf8_lossy(&output.stdout).into_owned();
    help.push_str(&String::from_utf8_lossy(&output.stderr));
    let missing = missing_progress_flags(&help, required);
    if !missing.is_empty() {
        anyhow::bail!(
            "installed {display_name} CLI is too old for live headless progress (lacks {}) - upgrade {display_name}",
            missing.join(", ")
        );
    }
    Ok(())
}

fn missing_progress_flags(help: &str, required: &[ProgressFlag]) -> Vec<&'static str> {
    required
        .iter()
        .filter(|req| !flag_advertised(help, req))
        .map(|req| req.flag)
        .collect()
}

/// Whether `help` advertises `req.flag` with `req.value` (if any) nearby.
/// Unlike a plain substring search over the whole text, this ties a generic
/// value token to the specific flag it must belong to.
fn flag_advertised(help: &str, req: &ProgressFlag) -> bool {
    let Some(value) = req.value else {
        return help.contains(req.flag);
    };
    help.match_indices(req.flag).any(|(start, _)| {
        let end = (start + req.flag.len() + PROGRESS_FLAG_VALUE_WINDOW).min(help.len());
        let mut end = end;
        while !help.is_char_boundary(end) {
            end -= 1;
        }
        help[start..end].contains(value)
    })
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

fn check_pi_headless_available() -> Result<()> {
    crate::pi::PiLauncher::check_available()?;
    let output = Command::new("pi")
        .arg("--help")
        .output()
        .context("pi CLI not found - is Pi installed?")?;
    if !output.status.success() {
        anyhow::bail!("installed Pi CLI could not describe its headless mode");
    }
    let mut help = String::from_utf8_lossy(&output.stdout).into_owned();
    help.push_str(&String::from_utf8_lossy(&output.stderr));
    let missing = missing_pi_headless_flags(&help);
    if !missing.is_empty() {
        anyhow::bail!(
            "installed Pi CLI is too old for safe plan interviews (lacks {}) - upgrade Pi",
            missing.join(", ")
        );
    }
    Ok(())
}

fn check_interview_available(harness: &AgentKind) -> Result<()> {
    match harness {
        AgentKind::Pi => check_pi_headless_available(),
        _ => HeadlessRunner::check_available(harness),
    }
}

fn missing_pi_headless_flags(help: &str) -> Vec<&'static str> {
    PI_HEADLESS_REQUIRED_FLAGS
        .iter()
        .copied()
        .filter(|flag| !help_advertises_flag(help, flag))
        .collect()
}

/// Match a complete long-option name rather than a substring: `--models`
/// must not satisfy the `--model` requirement on an older Pi release.
fn help_advertises_flag(help: &str, flag: &str) -> bool {
    help.match_indices(flag).any(|(start, _)| {
        help[start + flag.len()..]
            .chars()
            .next()
            .is_none_or(|next| !matches!(next, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_'))
    })
}

/// Exhaustive so introducing a new `AgentKind` forces an explicit decision
/// on whether its headless contract is trusted for interviews.
fn supports_headless_interview(harness: &AgentKind) -> bool {
    match harness {
        AgentKind::Claude | AgentKind::Codex | AgentKind::Opencode | AgentKind::Pi => true,
    }
}

fn interview_candidates(preferred: &AgentKind) -> Vec<AgentKind> {
    let mut candidates = Vec::with_capacity(4);
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

/// Harnesses whose headless CLI accepts `--model <name>`.
fn supports_model_flag(harness: &AgentKind) -> bool {
    match harness {
        AgentKind::Claude | AgentKind::Codex | AgentKind::Opencode | AgentKind::Pi => true,
    }
}

/// `spec.args`, then an optional `--model <name>` (only for harnesses where
/// [`supports_model_flag`] holds), then `spec.trailing` — e.g. Codex's `-`
/// stdin marker must stay last.
fn assemble_args(harness: &AgentKind, spec: &HeadlessCommand, model: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = spec.args.iter().map(|arg| arg.to_string()).collect();
    if let Some(model) = model
        && supports_model_flag(harness)
    {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    args.extend(spec.trailing.iter().map(|arg| arg.to_string()));
    args
}

fn run_command(
    harness: &AgentKind,
    spec: &HeadlessCommand,
    workdir: &Path,
    prompt: &str,
    model: Option<&str>,
) -> Result<String> {
    // Held for the whole run so the concurrency gate sees headless work.
    // Taken here rather than at each call site: every path out of this
    // function — spawn failure, `?`, cancellation, panic — releases it.
    let _lease = crate::resources::limits::HeadlessLease::acquire();
    let args = assemble_args(harness, spec, model);

    let mut child = Command::new(&spec.binary)
        .args(&args)
        .envs(spec.envs.iter().copied())
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

#[derive(Default)]
struct JsonlOutput {
    final_message: Option<String>,
    event_error: Option<String>,
    input_tokens: u64,
    output_tokens: u64,
}

/// Drain a harness's structured event stream while the child is alive so the
/// UI sees progress immediately. Stderr is drained separately to avoid pipe
/// deadlocks and preserve actionable provider errors on non-zero exit.
fn run_jsonl_command(
    harness: &AgentKind,
    spec: &HeadlessCommand,
    workdir: &Path,
    prompt: &str,
    model: Option<&str>,
    on_progress: impl Fn(HeadlessProgress) + Send + 'static,
) -> Result<String> {
    // See `run_command`: one lease per in-flight headless run, released on
    // every exit path including cancellation.
    let _lease = crate::resources::limits::HeadlessLease::acquire();
    let args = assemble_jsonl_args(harness, spec, model);

    let mut child = Command::new(&spec.binary)
        .args(&args)
        .envs(spec.envs.iter().copied())
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

    let mut stdin = child
        .stdin
        .take()
        .with_context(|| format!("Failed to open stdin for {}", harness.display_name()))?;
    let prompt = prompt.to_string();
    let writer = std::thread::spawn(move || stdin.write_all(prompt.as_bytes()));

    let stdout = child
        .stdout
        .take()
        .with_context(|| format!("Failed to capture stdout for {}", harness.display_name()))?;
    let harness_for_reader = harness.clone();
    let stdout_reader = std::thread::spawn(move || -> Result<JsonlOutput> {
        let mut output = JsonlOutput::default();
        for line in BufReader::new(stdout).lines() {
            let line = line.with_context(|| {
                format!(
                    "Failed to read {} JSONL output",
                    harness_for_reader.display_name()
                )
            })?;
            if line.trim().is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(&line).with_context(|| {
                format!(
                    "{} emitted invalid JSONL output",
                    harness_for_reader.display_name()
                )
            })?;
            apply_jsonl_event(&harness_for_reader, &value, &mut output, &on_progress);
        }
        Ok(output)
    });

    let mut stderr = child
        .stderr
        .take()
        .with_context(|| format!("Failed to capture stderr for {}", harness.display_name()))?;
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).map(|_| bytes)
    });

    let status = child
        .wait()
        .with_context(|| format!("Failed to run {} headlessly", harness.display_name()))?;
    let write_result = writer
        .join()
        .map_err(|_| anyhow::anyhow!("{} prompt writer panicked", harness.display_name()))?;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow::anyhow!("{} stderr reader panicked", harness.display_name()))?
        .with_context(|| format!("Failed to read stderr from {}", harness.display_name()))?;
    let json_output = stdout_reader
        .join()
        .map_err(|_| anyhow::anyhow!("{} JSONL reader panicked", harness.display_name()))??;

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr);
        let detail = stderr.trim();
        anyhow::bail!(
            "{} headless command failed{}{}",
            harness.display_name(),
            if detail.is_empty() { "" } else { ": " },
            detail
        );
    }
    write_result.with_context(|| format!("Failed to send prompt to {}", harness.display_name()))?;
    if let Some(message) = json_output.final_message {
        return Ok(message);
    }
    if let Some(error) = json_output.event_error {
        anyhow::bail!(
            "{} headless command failed: {error}",
            harness.display_name()
        );
    }
    anyhow::bail!(
        "{} headless command completed without a final agent message",
        harness.display_name()
    )
}

fn assemble_jsonl_args(
    harness: &AgentKind,
    spec: &HeadlessCommand,
    model: Option<&str>,
) -> Vec<String> {
    let mut args = assemble_args(harness, spec, model);
    match harness {
        AgentKind::Claude => {
            if let Some(format) = args
                .windows(2)
                .position(|pair| pair[0] == "--output-format")
            {
                args[format + 1] = "stream-json".to_string();
            }
            args.push("--verbose".to_string());
        }
        AgentKind::Codex => {
            let trailing_len = spec.trailing.len();
            args.insert(
                args.len().saturating_sub(trailing_len),
                "--json".to_string(),
            );
        }
        AgentKind::Opencode => args.extend(["--format".to_string(), "json".to_string()]),
        AgentKind::Pi => args.extend(["--mode".to_string(), "json".to_string()]),
    }
    args
}

fn apply_jsonl_event(
    harness: &AgentKind,
    event: &serde_json::Value,
    output: &mut JsonlOutput,
    on_progress: &impl Fn(HeadlessProgress),
) {
    match harness {
        AgentKind::Claude => apply_claude_json_event(event, output, on_progress),
        AgentKind::Codex => apply_codex_json_event(event, output, on_progress),
        AgentKind::Opencode => apply_opencode_json_event(event, output, on_progress),
        AgentKind::Pi => apply_pi_json_event(event, output, on_progress),
    }
}

fn apply_codex_json_event(
    event: &serde_json::Value,
    output: &mut JsonlOutput,
    on_progress: &impl Fn(HeadlessProgress),
) {
    match event.get("type").and_then(serde_json::Value::as_str) {
        Some("thread.started") => on_progress(HeadlessProgress::Activity(
            "Codex session started".to_string(),
        )),
        Some("turn.started") => on_progress(HeadlessProgress::Activity(
            "Analyzing the PR diff".to_string(),
        )),
        Some("item.started") | Some("item.completed") => {
            let event_type = event.get("type").and_then(serde_json::Value::as_str);
            let item = event.get("item").unwrap_or(&serde_json::Value::Null);
            let item_type = item.get("type").and_then(serde_json::Value::as_str);
            if item_type == Some("agent_message")
                && event_type == Some("item.completed")
                && let Some(text) = item.get("text").and_then(serde_json::Value::as_str)
            {
                output.final_message = Some(text.to_string());
            }
            let completed = event_type == Some("item.completed");
            let activity = match (item_type, completed) {
                (Some("reasoning"), false) => Some("Reasoning about possible findings"),
                (Some("reasoning"), true) => Some("Completed a reasoning step"),
                (Some("command_execution"), false) => Some("Inspecting the repository"),
                (Some("command_execution"), true) => Some("Completed a repository check"),
                (Some("mcp_tool_call"), false) => Some("Consulting an external tool"),
                (Some("mcp_tool_call"), true) => Some("Completed an external tool call"),
                (Some("web_search"), false) => Some("Searching for supporting context"),
                (Some("web_search"), true) => Some("Completed a context search"),
                (Some("plan"), _) => Some("Updating the review plan"),
                (Some("agent_message"), true) => Some("Drafted the review response"),
                _ => None,
            };
            if let Some(activity) = activity {
                on_progress(HeadlessProgress::Activity(activity.to_string()));
            }
        }
        Some("turn.completed") => {
            let usage = event.get("usage").unwrap_or(&serde_json::Value::Null);
            let input_tokens = usage
                .get("input_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            let output_tokens = usage
                .get("output_tokens")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            on_progress(HeadlessProgress::Usage {
                input_tokens,
                output_tokens,
            });
        }
        Some("turn.failed") | Some("error") => {
            output.event_error = event
                .get("message")
                .or_else(|| event.get("error").and_then(|error| error.get("message")))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
        }
        _ => {}
    }
}

fn apply_claude_json_event(
    event: &serde_json::Value,
    output: &mut JsonlOutput,
    on_progress: &impl Fn(HeadlessProgress),
) {
    match event.get("type").and_then(serde_json::Value::as_str) {
        Some("system") => match event.get("subtype").and_then(serde_json::Value::as_str) {
            Some("init") => on_progress(HeadlessProgress::Activity(
                "Claude session started".to_string(),
            )),
            Some("api_retry") => on_progress(HeadlessProgress::Activity(
                "Retrying the Claude request".to_string(),
            )),
            _ => {}
        },
        Some("assistant") => {
            let content = event
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(serde_json::Value::as_array);
            let activity = content.map(|blocks| {
                if blocks.iter().any(|block| {
                    matches!(
                        block.get("type").and_then(serde_json::Value::as_str),
                        Some("tool_use")
                    )
                }) {
                    "Inspecting the repository"
                } else if blocks.iter().any(|block| {
                    matches!(
                        block.get("type").and_then(serde_json::Value::as_str),
                        Some("thinking") | Some("redacted_thinking")
                    )
                }) {
                    "Reasoning about possible findings"
                } else {
                    "Drafting the review response"
                }
            });
            if let Some(activity) = activity {
                on_progress(HeadlessProgress::Activity(activity.to_string()));
            }
        }
        Some("user") => on_progress(HeadlessProgress::Activity(
            "Completed a repository check".to_string(),
        )),
        Some("result") => {
            if event
                .get("is_error")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
            {
                output.event_error = event
                    .get("result")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
            } else {
                output.final_message = event
                    .get("result")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
            }
            emit_usage_from(event.get("usage"), false, output, on_progress);
            on_progress(HeadlessProgress::Activity(
                "Completed the Claude review".to_string(),
            ));
        }
        _ => {}
    }
}

fn apply_opencode_json_event(
    event: &serde_json::Value,
    output: &mut JsonlOutput,
    on_progress: &impl Fn(HeadlessProgress),
) {
    match event.get("type").and_then(serde_json::Value::as_str) {
        Some("step_start") => on_progress(HeadlessProgress::Activity(
            "Opencode started a review step".to_string(),
        )),
        Some("tool_use") => on_progress(HeadlessProgress::Activity(
            "Completed a repository check".to_string(),
        )),
        Some("reasoning") => on_progress(HeadlessProgress::Activity(
            "Completed a reasoning step".to_string(),
        )),
        Some("text") => {
            // Opencode's `run --format json` emits `text` as incremental
            // chunks of the response rather than one final full-text part,
            // so each chunk is appended, not treated as a full replacement.
            if let Some(text) = event
                .get("part")
                .and_then(|part| part.get("text"))
                .and_then(serde_json::Value::as_str)
            {
                match &mut output.final_message {
                    Some(message) => message.push_str(text),
                    None => output.final_message = Some(text.to_string()),
                }
            }
            on_progress(HeadlessProgress::Activity(
                "Drafted the review response".to_string(),
            ));
        }
        Some("step_finish") => {
            let usage = event.get("part").and_then(|part| part.get("tokens"));
            emit_usage_from(usage, true, output, on_progress);
            on_progress(HeadlessProgress::Activity(
                "Completed an Opencode review step".to_string(),
            ));
        }
        Some("error") => {
            output.event_error = json_error_message(event.get("error"));
        }
        _ => {}
    }
}

fn apply_pi_json_event(
    event: &serde_json::Value,
    output: &mut JsonlOutput,
    on_progress: &impl Fn(HeadlessProgress),
) {
    match event.get("type").and_then(serde_json::Value::as_str) {
        Some("session") => {
            on_progress(HeadlessProgress::Activity("Pi session started".to_string()))
        }
        Some("turn_start") => on_progress(HeadlessProgress::Activity(
            "Analyzing the PR diff".to_string(),
        )),
        Some("tool_execution_start") => on_progress(HeadlessProgress::Activity(
            "Inspecting the repository".to_string(),
        )),
        Some("tool_execution_end") => on_progress(HeadlessProgress::Activity(
            "Completed a repository check".to_string(),
        )),
        Some("message_update") => {
            let update_type = event
                .get("assistantMessageEvent")
                .and_then(|update| update.get("type"))
                .and_then(serde_json::Value::as_str);
            if matches!(update_type, Some("thinking_start") | Some("thinking_delta")) {
                on_progress(HeadlessProgress::Activity(
                    "Reasoning about possible findings".to_string(),
                ));
            }
        }
        Some("message_end") => {
            let Some(message) = event.get("message") else {
                return;
            };
            if message.get("role").and_then(serde_json::Value::as_str) == Some("assistant") {
                if message
                    .get("stopReason")
                    .and_then(serde_json::Value::as_str)
                    == Some("error")
                {
                    output.event_error = message
                        .get("errorMessage")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string);
                }
                if let Some(text) = assistant_message_text(message) {
                    output.final_message = Some(text);
                }
                emit_usage_from(message.get("usage"), true, output, on_progress);
                on_progress(HeadlessProgress::Activity(
                    "Drafted the review response".to_string(),
                ));
            }
        }
        Some("auto_retry_start") => on_progress(HeadlessProgress::Activity(
            "Retrying the Pi request".to_string(),
        )),
        Some("auto_retry_end") => {
            if !event
                .get("success")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(true)
            {
                output.event_error = event
                    .get("finalError")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string);
            }
        }
        Some("compaction_start") => on_progress(HeadlessProgress::Activity(
            "Compacting the Pi review context".to_string(),
        )),
        Some("agent_end") => on_progress(HeadlessProgress::Activity(
            "Completed the Pi review".to_string(),
        )),
        _ => {}
    }
}

fn assistant_message_text(message: &serde_json::Value) -> Option<String> {
    let text = message
        .get("content")?
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(serde_json::Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    (!text.is_empty()).then_some(text)
}

fn emit_usage_from(
    usage: Option<&serde_json::Value>,
    accumulate: bool,
    output: &mut JsonlOutput,
    on_progress: &impl Fn(HeadlessProgress),
) {
    let Some(usage) = usage else {
        return;
    };
    let input = usage
        .get("input_tokens")
        .or_else(|| usage.get("input"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("output"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if accumulate {
        output.input_tokens = output.input_tokens.saturating_add(input);
        output.output_tokens = output.output_tokens.saturating_add(output_tokens);
    } else {
        output.input_tokens = input;
        output.output_tokens = output_tokens;
    }
    on_progress(HeadlessProgress::Usage {
        input_tokens: output.input_tokens,
        output_tokens: output.output_tokens,
    });
}

fn json_error_message(error: Option<&serde_json::Value>) -> Option<String> {
    let error = error?;
    error
        .get("data")
        .and_then(|data| data.get("message"))
        .or_else(|| error.get("message"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| error.as_str().map(str::to_string))
}

fn command_for(harness: &AgentKind, restricted: bool) -> HeadlessCommand {
    match harness {
        AgentKind::Claude => {
            let mut args = vec!["-p", "--output-format", "text"];
            if restricted {
                // Verified against the installed `claude --help`: --safe-mode
                // drops repo-configured hooks/MCP servers/plugins without
                // touching auth (unlike --bare, which also disables OAuth),
                // and --tools "" empties the tool whitelist outright so no
                // settings-file allow rule can grant Bash/Edit/Read.
                args.extend(["--safe-mode", "--tools", ""]);
            }
            HeadlessCommand {
                binary: crate::claude::ClaudeLauncher::resolve_binary(),
                args,
                trailing: vec![],
                envs: vec![],
            }
        }
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
            ],
            // Codex reads the prompt from stdin only when `-` is the final
            // positional arg; an inserted `--model` must land before it.
            trailing: vec!["-"],
            envs: vec![],
        },
        AgentKind::Opencode => {
            let mut args = vec!["run"];
            let mut envs = vec![];
            if restricted {
                // --pure drops external plugins; OPENCODE_PERMISSION is a
                // deny-all override applied after project/global
                // opencode.json (see OPENCODE_RESTRICTED_PERMISSION's doc).
                args.push("--pure");
                envs.push(("OPENCODE_PERMISSION", OPENCODE_RESTRICTED_PERMISSION));
            }
            HeadlessCommand {
                binary: "opencode".into(),
                args,
                trailing: vec![],
                envs,
            }
        }
        AgentKind::Pi => {
            let mut args = vec!["-p", "--no-session"];
            if restricted {
                // --no-tools covers built-in, extension, and custom tools on
                // current Pi releases. Disable all discovered prompt/code
                // resources as a second boundary, and ignore project-local
                // configuration so a repository cannot weaken the run.
                args.extend([
                    "--no-tools",
                    "--no-extensions",
                    "--no-skills",
                    "--no-prompt-templates",
                    "--no-context-files",
                    "--no-approve",
                ]);
            }
            HeadlessCommand {
                binary: "pi".into(),
                args,
                trailing: vec![],
                envs: vec![],
            }
        }
    }
}

/// Commands used when the model must investigate a repository without being
/// able to alter it.
fn read_only_command_for(harness: &AgentKind) -> Result<HeadlessCommand> {
    match harness {
        AgentKind::Claude => Ok(HeadlessCommand {
            binary: crate::claude::ClaudeLauncher::resolve_binary(),
            args: vec![
                "-p",
                "--output-format",
                "text",
                "--safe-mode",
                "--tools",
                "Read,Glob,Grep",
                "--permission-mode",
                "dontAsk",
                "--no-session-persistence",
            ],
            trailing: vec![],
            envs: vec![],
        }),
        // Codex's ordinary headless command is already an ephemeral read-only
        // sandbox, so the same command is the investigation contract.
        AgentKind::Codex => Ok(command_for(harness, false)),
        AgentKind::Opencode => Ok(HeadlessCommand {
            binary: "opencode".into(),
            args: vec!["run", "--pure"],
            trailing: vec![],
            envs: vec![("OPENCODE_PERMISSION", OPENCODE_READ_ONLY_PERMISSION)],
        }),
        AgentKind::Pi => Ok(HeadlessCommand {
            binary: "pi".into(),
            args: vec![
                "-p",
                "--no-session",
                "--tools",
                "read,grep,find,ls",
                "--no-extensions",
                "--no-skills",
                "--no-prompt-templates",
                "--no-context-files",
                "--no-approve",
            ],
            trailing: vec![],
            envs: vec![],
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::limits::{in_flight_headless_runs, lock_lease_tests, wait_for_in_flight};

    #[test]
    fn a_failed_headless_spawn_releases_its_lease() {
        let _guard = lock_lease_tests();
        let base = in_flight_headless_runs();
        let spec = HeadlessCommand {
            binary: "amf-no-such-headless-binary".into(),
            args: vec!["-p"],
            trailing: vec![],
            envs: vec![],
        };
        let result = run_command(
            &AgentKind::Claude,
            &spec,
            std::path::Path::new("/tmp"),
            "prompt",
            None,
        );
        assert!(result.is_err(), "spawn of a missing binary must fail");
        assert_eq!(wait_for_in_flight(base), base);
    }

    #[test]
    fn dropping_an_abandoned_leased_child_releases_its_lease() {
        let _guard = lock_lease_tests();
        // Stands in for a poll-driven run (walkthrough, co-review) that the
        // reviewer walks away from before it finishes.
        let base = in_flight_headless_runs();
        let child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sleep should be available");
        let mut leased = LeasedChild::new(child);
        assert!(in_flight_headless_runs() > base);
        assert!(leased.try_wait().expect("try_wait").is_none());

        drop(leased);
        assert_eq!(wait_for_in_flight(base), base);
    }

    #[test]
    fn collecting_a_leased_child_releases_its_lease() {
        let _guard = lock_lease_tests();
        let base = in_flight_headless_runs();
        let child = Command::new("true")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("true should be available");
        let leased = LeasedChild::new(child);
        assert!(in_flight_headless_runs() > base);

        let output = leased.wait_with_output().expect("wait_with_output");
        assert!(output.status.success());
        assert_eq!(wait_for_in_flight(base), base);
    }

    #[test]
    fn every_agent_harness_has_a_headless_command() {
        let claude = command_for(&AgentKind::Claude, false);
        assert!(!claude.binary.is_empty());
        assert_eq!(claude.args, ["-p", "--output-format", "text"]);

        assert_eq!(
            command_for(&AgentKind::Codex, false),
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
                ],
                trailing: vec!["-"],
                envs: vec![],
            }
        );
        assert_eq!(
            command_for(&AgentKind::Opencode, false),
            HeadlessCommand {
                binary: "opencode".into(),
                args: vec!["run"],
                trailing: vec![],
                envs: vec![],
            }
        );
        assert_eq!(
            command_for(&AgentKind::Pi, false),
            HeadlessCommand {
                binary: "pi".into(),
                args: vec!["-p", "--no-session"],
                trailing: vec![],
                envs: vec![],
            }
        );
    }

    #[test]
    fn restricted_harnesses_deny_repo_configurable_tool_access() {
        let claude = command_for(&AgentKind::Claude, true);
        assert!(claude.args.contains(&"--safe-mode"));
        let tools_idx = claude
            .args
            .iter()
            .position(|arg| *arg == "--tools")
            .expect("--tools flag present");
        assert_eq!(claude.args[tools_idx + 1], "");

        let opencode = command_for(&AgentKind::Opencode, true);
        assert!(opencode.args.contains(&"--pure"));
        assert_eq!(
            opencode.envs,
            [("OPENCODE_PERMISSION", OPENCODE_RESTRICTED_PERMISSION)]
        );

        // Codex is already sandboxed unconditionally; restricted is a no-op.
        assert_eq!(
            command_for(&AgentKind::Codex, true),
            command_for(&AgentKind::Codex, false)
        );

        let pi = command_for(&AgentKind::Pi, true);
        for flag in [
            "--no-tools",
            "--no-extensions",
            "--no-skills",
            "--no-prompt-templates",
            "--no-context-files",
            "--no-approve",
        ] {
            assert!(pi.args.contains(&flag), "restricted Pi lacks {flag}");
        }
    }

    #[test]
    fn read_only_commands_allow_inspection_without_edit_or_shell_tools() {
        let claude = read_only_command_for(&AgentKind::Claude).unwrap();
        assert!(claude.args.contains(&"--safe-mode"));
        let tools_idx = claude
            .args
            .iter()
            .position(|arg| *arg == "--tools")
            .unwrap();
        assert_eq!(claude.args[tools_idx + 1], "Read,Glob,Grep");
        assert!(claude.args.contains(&"dontAsk"));

        let codex = read_only_command_for(&AgentKind::Codex).unwrap();
        let sandbox_idx = codex
            .args
            .iter()
            .position(|arg| *arg == "--sandbox")
            .unwrap();
        assert_eq!(codex.args[sandbox_idx + 1], "read-only");

        let opencode = read_only_command_for(&AgentKind::Opencode).unwrap();
        assert!(opencode.args.contains(&"--pure"));
        assert_eq!(
            opencode.envs,
            [("OPENCODE_PERMISSION", OPENCODE_READ_ONLY_PERMISSION)]
        );

        let pi = read_only_command_for(&AgentKind::Pi).unwrap();
        let tools_idx = pi.args.iter().position(|arg| *arg == "--tools").unwrap();
        assert_eq!(pi.args[tools_idx + 1], "read,grep,find,ls");
        assert!(
            !pi.args
                .iter()
                .any(|arg| matches!(*arg, "bash" | "edit" | "write"))
        );
        assert!(pi.args.contains(&"--no-extensions"));
        assert!(pi.args.contains(&"--no-approve"));
    }

    #[test]
    fn runner_pipes_the_prompt_over_stdin() {
        let spec = HeadlessCommand {
            binary: "sh".into(),
            args: vec!["-c", "read input; printf 'received:%s' \"$input\""],
            trailing: vec![],
            envs: vec![],
        };
        let output = run_command(&AgentKind::Codex, &spec, Path::new("/tmp"), "hello", None)
            .expect("fake headless command should succeed");
        assert_eq!(output, "received:hello");
    }

    #[test]
    fn runner_failure_names_the_provider_and_preserves_stderr() {
        let spec = HeadlessCommand {
            binary: "sh".into(),
            args: vec!["-c", "printf 'quota exhausted' >&2; exit 9"],
            trailing: vec![],
            envs: vec![],
        };
        // Large enough that the early exit also breaks the stdin writer. The
        // provider's stderr must win over that secondary pipe error.
        let prompt = "x".repeat(1_000_000);
        let error = run_command(
            &AgentKind::Opencode,
            &spec,
            Path::new("/tmp"),
            &prompt,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("Opencode headless command failed"));
        assert!(error.contains("quota exhausted"));
    }

    #[test]
    fn model_flag_is_appended_for_supported_harnesses_only() {
        assert!(supports_model_flag(&AgentKind::Claude));
        assert!(supports_model_flag(&AgentKind::Codex));
        assert!(supports_model_flag(&AgentKind::Opencode));
        assert!(supports_model_flag(&AgentKind::Pi));
    }

    #[test]
    fn assemble_args_inserts_model_before_trailing_stdin_marker() {
        let spec = command_for(&AgentKind::Codex, false);
        assert_eq!(
            assemble_args(&AgentKind::Codex, &spec, Some("gpt-5.5")),
            [
                "exec",
                "--sandbox",
                "read-only",
                "--ephemeral",
                "--skip-git-repo-check",
                "--color",
                "never",
                "--model",
                "gpt-5.5",
                "-",
            ]
        );
    }

    #[test]
    fn codex_json_args_keep_json_and_model_before_stdin_marker() {
        let spec = command_for(&AgentKind::Codex, false);
        assert_eq!(
            assemble_jsonl_args(&AgentKind::Codex, &spec, Some("gpt-5.5")),
            [
                "exec",
                "--sandbox",
                "read-only",
                "--ephemeral",
                "--skip-git-repo-check",
                "--color",
                "never",
                "--model",
                "gpt-5.5",
                "--json",
                "-",
            ]
        );
    }

    #[test]
    fn structured_args_enable_each_harness_event_stream() {
        assert_eq!(
            assemble_jsonl_args(
                &AgentKind::Claude,
                &command_for(&AgentKind::Claude, false),
                Some("sonnet")
            ),
            [
                "-p",
                "--output-format",
                "stream-json",
                "--model",
                "sonnet",
                "--verbose",
            ]
        );
        assert_eq!(
            assemble_jsonl_args(
                &AgentKind::Opencode,
                &command_for(&AgentKind::Opencode, false),
                None
            ),
            ["run", "--format", "json"]
        );
        assert_eq!(
            assemble_jsonl_args(&AgentKind::Pi, &command_for(&AgentKind::Pi, false), None),
            ["-p", "--no-session", "--mode", "json"]
        );
    }

    #[test]
    fn assemble_args_applies_model_flag_for_pi() {
        let spec = command_for(&AgentKind::Pi, false);
        assert_eq!(
            assemble_args(&AgentKind::Pi, &spec, Some("some-model")),
            ["-p", "--no-session", "--model", "some-model"]
        );
    }

    #[test]
    fn assemble_args_is_unchanged_when_no_model_requested() {
        let spec = command_for(&AgentKind::Claude, false);
        assert_eq!(
            assemble_args(&AgentKind::Claude, &spec, None),
            ["-p", "--output-format", "text"]
        );
    }

    #[test]
    fn codex_flag_probe_reports_only_missing_required_flags() {
        let full_help = "Options:\n  --sandbox <MODE>\n  --ephemeral\n  \
                         --skip-git-repo-check\n  --color <WHEN>\n  --json";
        assert!(missing_codex_exec_flags(full_help).is_empty());

        let old_help = "Options:\n  --sandbox <MODE>\n  --color <WHEN>";
        assert_eq!(
            missing_codex_exec_flags(old_help),
            ["--ephemeral", "--skip-git-repo-check", "--json"]
        );
    }

    #[test]
    fn pi_flag_probe_reports_only_missing_required_flags() {
        let full_help = PI_HEADLESS_REQUIRED_FLAGS.join("\n");
        assert!(missing_pi_headless_flags(&full_help).is_empty());

        let old_help = "Options:\n  --print\n  --no-session\n  --model";
        assert_eq!(
            missing_pi_headless_flags(old_help),
            [
                "--no-tools",
                "--tools",
                "--no-extensions",
                "--no-skills",
                "--no-prompt-templates",
                "--no-context-files",
                "--no-approve",
            ]
        );

        let ambiguous_help = "Options:\n  --models <patterns>\n  --no-tools";
        assert!(!help_advertises_flag(ambiguous_help, "--model"));
        assert!(help_advertises_flag(ambiguous_help, "--no-tools"));
    }

    #[test]
    fn pi_required_flags_cover_every_interview_command_flag() {
        for spec in [
            command_for(&AgentKind::Pi, true),
            read_only_command_for(&AgentKind::Pi).unwrap(),
        ] {
            for arg in spec.args.iter().filter(|arg| arg.starts_with("--")) {
                assert!(
                    PI_HEADLESS_REQUIRED_FLAGS.contains(arg),
                    "Pi interview command passes {arg} but availability never checks it"
                );
            }
        }
    }

    #[test]
    fn progress_flag_probe_requires_value_near_its_flag_not_anywhere_in_help() {
        // An older CLI whose --format only supports plain text, but which
        // happens to mention "json" elsewhere, far from --format's own
        // definition (a config file example, an unrelated flag), must not
        // be mistaken for one that supports `--format json`.
        let filler = "x".repeat(300);
        let stale_help = format!(
            "Options:\n  --format <FORMAT>  text or markdown\n{filler}\n  \
             --config <PATH>  e.g. settings.json"
        );
        assert_eq!(
            missing_progress_flags(&stale_help, &OPENCODE_PROGRESS_REQUIRED_FLAGS),
            ["--format"]
        );

        let current_help = "Options:\n  --format <FORMAT>  [possible values: text, json]";
        assert!(missing_progress_flags(current_help, &OPENCODE_PROGRESS_REQUIRED_FLAGS).is_empty());
    }

    #[test]
    fn progress_flag_probe_tolerates_value_wrapped_onto_its_own_line() {
        let wrapped_help = "Options:\n  --output-format <FORMAT>\n          \
                             Output format\n          \
                             [possible values: text, json, stream-json]\n  --verbose";
        assert!(missing_progress_flags(wrapped_help, &CLAUDE_PROGRESS_REQUIRED_FLAGS).is_empty());
    }

    #[test]
    fn codex_required_flags_cover_everything_the_headless_command_passes() {
        let spec = command_for(&AgentKind::Codex, false);
        let args = assemble_jsonl_args(&AgentKind::Codex, &spec, None);
        for flag in CODEX_EXEC_REQUIRED_FLAGS {
            assert!(
                args.iter().any(|arg| arg == flag),
                "probe checks {flag} but the headless command no longer passes it"
            );
        }
        for arg in spec.args.iter().filter(|arg| arg.starts_with("--")) {
            assert!(
                CODEX_EXEC_REQUIRED_FLAGS.contains(arg),
                "headless command passes {arg} but the availability probe never checks it"
            );
        }
    }

    #[test]
    fn codex_json_events_preserve_final_message_and_sanitize_progress() {
        use std::cell::RefCell;

        let progress = RefCell::new(Vec::new());
        let mut output = JsonlOutput::default();
        for event in [
            serde_json::json!({"type": "thread.started", "thread_id": "secret"}),
            serde_json::json!({"type": "turn.started"}),
            serde_json::json!({
                "type": "item.started",
                "item": {
                    "type": "command_execution",
                    "command": "cat super-secret-review-input"
                }
            }),
            serde_json::json!({
                "type": "item.completed",
                "item": {
                    "type": "agent_message",
                    "text": "### src/main.rs:42\nFinding body"
                }
            }),
            serde_json::json!({
                "type": "turn.completed",
                "usage": {"input_tokens": 1200, "output_tokens": 34}
            }),
        ] {
            apply_codex_json_event(&event, &mut output, &|event| {
                progress.borrow_mut().push(event)
            });
        }

        assert_eq!(
            output.final_message.as_deref(),
            Some("### src/main.rs:42\nFinding body")
        );
        assert!(progress.borrow().contains(&HeadlessProgress::Activity(
            "Inspecting the repository".to_string()
        )));
        assert!(progress.borrow().contains(&HeadlessProgress::Usage {
            input_tokens: 1200,
            output_tokens: 34,
        }));
        let rendered = format!("{:?}", progress.borrow());
        assert!(!rendered.contains("super-secret-review-input"));
        assert!(!rendered.contains("Finding body"));
    }

    #[test]
    fn codex_json_error_event_keeps_actionable_message() {
        let mut output = JsonlOutput::default();
        apply_codex_json_event(
            &serde_json::json!({
                "type": "turn.failed",
                "error": {"message": "context window exceeded"}
            }),
            &mut output,
            &|_| {},
        );
        assert_eq!(
            output.event_error.as_deref(),
            Some("context window exceeded")
        );
    }

    #[test]
    fn claude_stream_events_preserve_result_usage_and_redact_payloads() {
        use std::cell::RefCell;

        let progress = RefCell::new(Vec::new());
        let mut output = JsonlOutput::default();
        for event in [
            serde_json::json!({"type": "system", "subtype": "init"}),
            serde_json::json!({
                "type": "assistant",
                "message": {"content": [{
                    "type": "tool_use",
                    "name": "Bash",
                    "input": {"command": "cat private-diff"}
                }]}
            }),
            serde_json::json!({
                "type": "result",
                "subtype": "success",
                "is_error": false,
                "result": "### src/lib.rs:7\nFinding body",
                "usage": {"input_tokens": 450, "output_tokens": 21}
            }),
        ] {
            apply_claude_json_event(&event, &mut output, &|event| {
                progress.borrow_mut().push(event)
            });
        }

        assert_eq!(
            output.final_message.as_deref(),
            Some("### src/lib.rs:7\nFinding body")
        );
        assert!(progress.borrow().contains(&HeadlessProgress::Usage {
            input_tokens: 450,
            output_tokens: 21,
        }));
        let rendered = format!("{:?}", progress.borrow());
        assert!(!rendered.contains("private-diff"));
        assert!(!rendered.contains("Finding body"));
    }

    #[test]
    fn opencode_events_capture_final_text_and_accumulate_step_usage() {
        let progress = std::cell::RefCell::new(Vec::new());
        let mut output = JsonlOutput::default();
        for event in [
            serde_json::json!({
                "type": "tool_use",
                "part": {"tool": "bash", "state": {"output": "private output"}}
            }),
            serde_json::json!({
                "type": "step_finish",
                "part": {"tokens": {"input": 100, "output": 10}}
            }),
            serde_json::json!({
                "type": "step_finish",
                "part": {"tokens": {"input": 50, "output": 5}}
            }),
            serde_json::json!({
                "type": "text",
                "part": {"text": "### src/main.rs:9\n"}
            }),
            serde_json::json!({
                "type": "text",
                "part": {"text": "Finding"}
            }),
        ] {
            apply_opencode_json_event(&event, &mut output, &|event| {
                progress.borrow_mut().push(event)
            });
        }

        assert_eq!(
            output.final_message.as_deref(),
            Some("### src/main.rs:9\nFinding")
        );
        assert!(progress.borrow().contains(&HeadlessProgress::Usage {
            input_tokens: 150,
            output_tokens: 15,
        }));
        assert!(!format!("{:?}", progress.borrow()).contains("private output"));
    }

    #[test]
    fn pi_events_capture_assistant_text_usage_and_redact_tool_details() {
        let progress = std::cell::RefCell::new(Vec::new());
        let mut output = JsonlOutput::default();
        for event in [
            serde_json::json!({"type": "session", "cwd": "/secret/path"}),
            serde_json::json!({
                "type": "tool_execution_start",
                "toolName": "bash",
                "args": {"command": "cat private-diff"}
            }),
            serde_json::json!({
                "type": "message_end",
                "message": {
                    "role": "assistant",
                    "content": [
                        {"type": "thinking", "thinking": "private reasoning"},
                        {"type": "text", "text": "### src/app.rs:4\nFinding"}
                    ],
                    "usage": {"input": 88, "output": 12}
                }
            }),
        ] {
            apply_pi_json_event(&event, &mut output, &|event| {
                progress.borrow_mut().push(event)
            });
        }

        assert_eq!(
            output.final_message.as_deref(),
            Some("### src/app.rs:4\nFinding")
        );
        assert!(progress.borrow().contains(&HeadlessProgress::Usage {
            input_tokens: 88,
            output_tokens: 12,
        }));
        let rendered = format!("{:?}", progress.borrow());
        assert!(!rendered.contains("private-diff"));
        assert!(!rendered.contains("private reasoning"));
    }

    #[test]
    fn pi_terminal_retry_error_is_actionable() {
        let mut output = JsonlOutput::default();
        apply_pi_json_event(
            &serde_json::json!({
                "type": "auto_retry_end",
                "success": false,
                "attempt": 3,
                "finalError": "rate limit did not recover"
            }),
            &mut output,
            &|_| {},
        );
        assert_eq!(
            output.event_error.as_deref(),
            Some("rate limit did not recover")
        );
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
    fn interview_candidates_prefer_verified_pi_headless_mode() {
        assert_eq!(
            interview_candidates(&AgentKind::Pi),
            [
                AgentKind::Pi,
                AgentKind::Claude,
                AgentKind::Codex,
                AgentKind::Opencode
            ]
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
