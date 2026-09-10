//! Explicit execution boundaries for progress-enabled headless calls.
//!
//! These describe the adapter's command contract. CLI help probes detect old
//! versions, not whether a provider/model is authenticated or whether its
//! sandbox actually works. Real-harness conformance remains a separate check.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadlessExecutionPolicy {
    /// Preserve each harness's existing ordinary headless configuration.
    Ordinary,
    /// Disable tools. In particular, Codex's read-only sandbox is not enough.
    PacketOnly,
    /// Allow only the adapter's repository-inspection tools, with no shell.
    ReadOnlyTools,
    /// Codex filesystem sandbox; commands/configured external tools may exist.
    ReadOnlySandbox,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessToolAccess {
    HarnessConfigured,
    NoTools,
    ReadTools,
    /// No claim that commands or externally configured tools are disabled.
    SandboxedCommands,
}

/// Only capabilities wired through AMF are reported. Optional usage counters
/// do not establish complete billable usage; no hard budgets are implied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadlessCapabilities {
    pub policies: &'static [HeadlessExecutionPolicy],
    pub ordinary_access: HeadlessToolAccess,
}

impl HeadlessCapabilities {
    pub fn access_for(self, policy: HeadlessExecutionPolicy) -> Result<HeadlessToolAccess> {
        anyhow::ensure!(
            self.policies.contains(&policy),
            "unsupported headless execution policy {policy:?}; select an explicitly supported profile"
        );
        Ok(match policy {
            HeadlessExecutionPolicy::Ordinary => self.ordinary_access,
            HeadlessExecutionPolicy::PacketOnly => HeadlessToolAccess::NoTools,
            HeadlessExecutionPolicy::ReadOnlyTools => HeadlessToolAccess::ReadTools,
            HeadlessExecutionPolicy::ReadOnlySandbox => HeadlessToolAccess::SandboxedCommands,
        })
    }
}

pub(super) fn capabilities(harness: &AgentKind) -> HeadlessCapabilities {
    use HeadlessExecutionPolicy::*;
    match harness {
        AgentKind::Codex => HeadlessCapabilities {
            policies: &[Ordinary, ReadOnlySandbox],
            ordinary_access: HeadlessToolAccess::SandboxedCommands,
        },
        AgentKind::Claude | AgentKind::Opencode | AgentKind::Pi => HeadlessCapabilities {
            policies: &[Ordinary, PacketOnly, ReadOnlyTools],
            ordinary_access: HeadlessToolAccess::HarnessConfigured,
        },
    }
}

pub(super) fn command_for_policy(
    harness: &AgentKind,
    policy: HeadlessExecutionPolicy,
) -> Result<HeadlessCommand> {
    command_for_policy_with_binary(harness, policy, None)
}

/// An explicit executable avoids unbounded binary-discovery subprocesses in
/// owned jobs. Existing callers keep the legacy discovery path via `None`.
pub(super) fn command_for_policy_with_binary(
    harness: &AgentKind,
    policy: HeadlessExecutionPolicy,
    binary: Option<&str>,
) -> Result<HeadlessCommand> {
    // Check support before resolving a binary or running any external command.
    let access = HeadlessRunner::capabilities(harness)
        .access_for(policy)
        .with_context(|| format!("{} cannot run {policy:?}", harness.display_name()))?;
    let resolve = || {
        binary
            .map(str::to_string)
            .unwrap_or_else(crate::claude::ClaudeLauncher::resolve_binary)
    };
    let mut spec = match access {
        HeadlessToolAccess::HarnessConfigured | HeadlessToolAccess::SandboxedCommands => {
            command_for_with_binary(harness, false, resolve)
        }
        HeadlessToolAccess::NoTools => command_for_with_binary(harness, true, resolve),
        HeadlessToolAccess::ReadTools => read_only_command_with_binary(harness, resolve)?,
    };
    if let Some(binary) = binary {
        spec.binary = binary.to_string();
    }
    // Release builds must reject an accidentally loosened builder too.
    anyhow::ensure!(
        command_matches_access(harness, &spec, access),
        "{} command does not satisfy {policy:?}",
        harness.display_name()
    );
    Ok(spec)
}

fn command_matches_access(
    harness: &AgentKind,
    spec: &HeadlessCommand,
    access: HeadlessToolAccess,
) -> bool {
    if access == HeadlessToolAccess::HarnessConfigured {
        return true;
    }
    let value = |flag: &str| {
        let mut indices = spec
            .args
            .iter()
            .enumerate()
            .filter(|(_, arg)| **arg == flag);
        let (index, _) = indices.next()?;
        if indices.next().is_some() {
            return None;
        }
        spec.args.get(index + 1).copied()
    };
    // This validator is independent of the builders: comparing a command to
    // another invocation of its builder would bless the same regression twice.
    let has_flags = |flags: &[&str]| flags.iter().all(|flag| spec.args.contains(flag));
    let only_flags = |allowed: &[&str]| {
        spec.args
            .iter()
            .filter(|arg| arg.starts_with('-'))
            .all(|arg| allowed.contains(arg))
    };
    let no_tools = access == HeadlessToolAccess::NoTools;
    match harness {
        AgentKind::Codex => {
            access == HeadlessToolAccess::SandboxedCommands
                && value("--sandbox") == Some("read-only")
                && has_flags(&["--ephemeral"])
                && only_flags(&[
                    "--sandbox",
                    "--ephemeral",
                    "--skip-git-repo-check",
                    "--color",
                ])
                && spec.envs.is_empty()
        }
        AgentKind::Claude => {
            matches!(
                access,
                HeadlessToolAccess::NoTools | HeadlessToolAccess::ReadTools
            ) && has_flags(&["--safe-mode"])
                && value("--tools") == Some(if no_tools { "" } else { "Read,Glob,Grep" })
                && (no_tools || value("--permission-mode") == Some("dontAsk"))
                && only_flags(&[
                    "-p",
                    "--output-format",
                    "--safe-mode",
                    "--tools",
                    "--permission-mode",
                    "--no-session-persistence",
                ])
                && !spec
                    .args
                    .windows(2)
                    .any(|pair| pair[0] == "--permission-mode" && pair[1] != "dontAsk")
                && spec.envs.is_empty()
        }
        AgentKind::Opencode => {
            matches!(
                access,
                HeadlessToolAccess::NoTools | HeadlessToolAccess::ReadTools
            ) && has_flags(&["--pure"])
                && only_flags(&["--pure"])
                && spec.envs
                    == [(
                        "OPENCODE_PERMISSION",
                        if no_tools {
                            OPENCODE_RESTRICTED_PERMISSION
                        } else {
                            OPENCODE_READ_ONLY_PERMISSION
                        },
                    )]
        }
        AgentKind::Pi => {
            matches!(
                access,
                HeadlessToolAccess::NoTools | HeadlessToolAccess::ReadTools
            ) && has_flags(&[
                "--no-extensions",
                "--no-skills",
                "--no-prompt-templates",
                "--no-context-files",
                "--no-approve",
            ]) && if no_tools {
                has_flags(&["--no-tools"]) && !spec.args.contains(&"--tools")
            } else {
                value("--tools") == Some("read,grep,find,ls")
            } && only_flags(&[
                "-p",
                "--no-session",
                "--no-tools",
                "--tools",
                "--no-extensions",
                "--no-skills",
                "--no-prompt-templates",
                "--no-context-files",
                "--no-approve",
            ]) && spec.envs.is_empty()
        }
    }
}

pub(super) fn check_command_available(
    harness: &AgentKind,
    spec: &HeadlessCommand,
    model: Option<&str>,
) -> Result<()> {
    if let Some(model) = model {
        anyhow::ensure!(!model.trim().is_empty(), "headless model must not be empty");
    }
    let help_args: &[&str] = match harness {
        AgentKind::Codex => &["exec", "--help"],
        AgentKind::Opencode => &["run", "--help"],
        AgentKind::Claude | AgentKind::Pi => &["--help"],
    };
    let output = Command::new(&spec.binary)
        .args(help_args)
        // Apply the same safety environment to the help process as to the run.
        .envs(spec.envs.iter().copied())
        .output()
        .with_context(|| format!("{} CLI not found", harness.display_name()))?;
    anyhow::ensure!(
        output.status.success(),
        "{} CLI could not describe its headless mode",
        harness.display_name()
    );
    let help = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let missing = missing_command_flags(harness, spec, model, &help);
    anyhow::ensure!(
        missing.is_empty(),
        "installed {} CLI lacks required headless policy/progress flags: {}; upgrade the harness",
        harness.display_name(),
        missing.join(", ")
    );
    Ok(())
}

pub(super) fn missing_command_flags(
    harness: &AgentKind,
    spec: &HeadlessCommand,
    model: Option<&str>,
    help: &str,
) -> Vec<String> {
    let args = assemble_jsonl_args(harness, spec, model);
    let mut missing = Vec::new();
    // Inspect arguments by position: a model value beginning with '--' must
    // not be mistaken for another option we expect the CLI to advertise.
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        let flag = if arg == "-p" { "--print" } else { arg };
        if flag.starts_with("--") && !help_advertises_flag(help, flag) {
            missing.push(flag.to_string());
        }
        index += if matches!(
            arg,
            "--model"
                | "--output-format"
                | "--tools"
                | "--permission-mode"
                | "--sandbox"
                | "--color"
                | "--format"
                | "--mode"
        ) {
            2
        } else {
            1
        };
    }
    let progress = match harness {
        AgentKind::Claude => &CLAUDE_PROGRESS_REQUIRED_FLAGS[..],
        AgentKind::Opencode => &OPENCODE_PROGRESS_REQUIRED_FLAGS[..],
        AgentKind::Pi => &PI_PROGRESS_REQUIRED_FLAGS[..],
        AgentKind::Codex => &[],
    };
    for flag in missing_progress_flags(help, progress) {
        if !missing.iter().any(|item| item == flag) {
            missing.push(flag.to_string());
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn codex_never_advertises_packet_or_tool_whitelist_access() {
        let report = HeadlessRunner::capabilities(&AgentKind::Codex);
        for policy in [
            HeadlessExecutionPolicy::PacketOnly,
            HeadlessExecutionPolicy::ReadOnlyTools,
        ] {
            assert!(report.access_for(policy).is_err());
            assert!(command_for_policy(&AgentKind::Codex, policy).is_err());
        }
        for policy in report.policies {
            assert_eq!(
                report.access_for(*policy).unwrap(),
                HeadlessToolAccess::SandboxedCommands
            );
        }
    }

    #[test]
    fn ordinary_policy_preserves_every_existing_command() {
        for harness in AgentKind::ALL {
            assert_eq!(
                command_for_policy(&harness, HeadlessExecutionPolicy::Ordinary).unwrap(),
                command_for(&harness, false)
            );
        }
    }

    #[test]
    fn explicit_commands_reject_loosened_permissions_in_release_validation() {
        for harness in [AgentKind::Claude, AgentKind::Opencode, AgentKind::Pi] {
            for policy in [
                HeadlessExecutionPolicy::PacketOnly,
                HeadlessExecutionPolicy::ReadOnlyTools,
            ] {
                let mut spec = command_for_policy(&harness, policy).unwrap();
                let access = capabilities(&harness).access_for(policy).unwrap();
                assert!(command_matches_access(&harness, &spec, access));
                if harness == AgentKind::Opencode {
                    spec.envs.clear();
                } else {
                    spec.args.push("--tools");
                    spec.args.push("Bash,Edit");
                }
                assert!(!command_matches_access(&harness, &spec, access));
            }
        }
    }

    #[test]
    fn removing_discovery_restrictions_invalidates_the_policy() {
        for (harness, flags) in [
            (AgentKind::Claude, vec!["--safe-mode"]),
            (AgentKind::Opencode, vec!["--pure"]),
            (
                AgentKind::Pi,
                vec![
                    "--no-extensions",
                    "--no-skills",
                    "--no-prompt-templates",
                    "--no-context-files",
                    "--no-approve",
                ],
            ),
        ] {
            for policy in [
                HeadlessExecutionPolicy::PacketOnly,
                HeadlessExecutionPolicy::ReadOnlyTools,
            ] {
                for flag in &flags {
                    let mut spec = command_for_policy(&harness, policy).unwrap();
                    spec.args.retain(|arg| arg != flag);
                    assert!(
                        !command_matches_access(
                            &harness,
                            &spec,
                            capabilities(&harness).access_for(policy).unwrap()
                        ),
                        "{harness:?} accepts missing {flag}"
                    );
                }
            }
        }
    }

    #[test]
    fn codex_sandbox_validation_rejects_escalation_or_duplicate_options() {
        for extra in [
            vec!["--sandbox", "workspace-write"],
            vec!["--dangerously-bypass-approvals-and-sandbox"],
        ] {
            let mut spec =
                command_for_policy(&AgentKind::Codex, HeadlessExecutionPolicy::ReadOnlySandbox)
                    .unwrap();
            spec.args.extend(extra);
            assert!(!command_matches_access(
                &AgentKind::Codex,
                &spec,
                HeadlessToolAccess::SandboxedCommands
            ));
        }
        assert!(
            HeadlessRunner::run_with_policy_and_progress(
                &AgentKind::Codex,
                Path::new("/nonexistent"),
                "must not run",
                None,
                HeadlessExecutionPolicy::PacketOnly,
                |_| panic!("unsupported run emitted progress")
            )
            .is_err()
        );
    }

    // An executable stand-in verifies bytes/args/env at the process boundary.
    // It does not claim to prove the real provider's tool restrictions.
    fn fake_cli(dir: &Path, help: &str, events: &str, exit_code: u8) -> String {
        let script = dir.join("harness");
        std::fs::write(&script, format!(
            "#!/bin/sh\nfor arg in \"$@\"; do\n  if [ \"$arg\" = --help ]; then\n    cat <<'AMF_HELP'\n{help}\nAMF_HELP\n    exit 0\n  fi\ndone\nprintf '%s\\n' \"$@\" > args\nprintf '%s' \"${{OPENCODE_PERMISSION-}}\" > permissions\ncat > prompt\ncat <<'AMF_EVENTS'\n{events}\nAMF_EVENTS\nexit {exit_code}\n"
        )).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        script.to_string_lossy().into_owned()
    }

    fn complete_help() -> &'static str {
        "--print --model --safe-mode --tools --permission-mode --no-session-persistence
--output-format text stream-json --verbose
--pure --format json
--no-session --no-tools --no-extensions --no-skills --no-prompt-templates
--no-context-files --no-approve --mode json
--sandbox read-only --ephemeral --skip-git-repo-check --color never --json"
    }

    fn success_event(harness: &AgentKind) -> &'static str {
        match harness {
            AgentKind::Claude => {
                r#"{"type":"result","subtype":"success","is_error":false,"result":"advice","usage":{"input_tokens":4,"output_tokens":2}}"#
            }
            AgentKind::Codex => {
                r#"{"type":"item.completed","item":{"type":"agent_message","text":"advice"}}"#
            }
            AgentKind::Opencode => r#"{"type":"text","part":{"text":"advice"}}"#,
            AgentKind::Pi => {
                r#"{"type":"message_end","message":{"role":"assistant","content":[{"type":"text","text":"advice"}],"usage":{"input":4,"output":2}}}"#
            }
        }
    }

    #[test]
    fn policy_progress_runs_preserve_args_environment_and_stdin() {
        for harness in AgentKind::ALL {
            for policy in capabilities(&harness).policies {
                if *policy == HeadlessExecutionPolicy::Ordinary {
                    continue;
                }
                let dir = tempfile::tempdir().unwrap();
                let mut spec = command_for_policy(&harness, *policy).unwrap();
                spec.binary = fake_cli(dir.path(), complete_help(), success_event(&harness), 0);
                check_command_available(&harness, &spec, Some("expert-model")).unwrap();
                let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
                let progress = events.clone();
                let answer = run_jsonl_command(
                    &harness,
                    &spec,
                    dir.path(),
                    "private\nquestion",
                    Some("expert-model"),
                    move |event| {
                        progress.lock().unwrap().push(event);
                    },
                )
                .unwrap();
                assert_eq!(answer, "advice");
                assert_eq!(
                    std::fs::read_to_string(dir.path().join("prompt")).unwrap(),
                    "private\nquestion"
                );
                let received = std::fs::read_to_string(dir.path().join("args")).unwrap();
                let args: Vec<_> = received.lines().collect();
                assert_eq!(
                    args,
                    assemble_jsonl_args(&harness, &spec, Some("expert-model"))
                );
                let env = std::fs::read_to_string(dir.path().join("permissions")).unwrap();
                if harness == AgentKind::Opencode {
                    assert_eq!(env, spec.envs[0].1);
                }
                if harness == AgentKind::Codex {
                    assert_eq!(args.last(), Some(&"-"));
                }
                assert!(!events.lock().unwrap().is_empty());
                assert!(!format!("{:?}", events.lock().unwrap()).contains("private"));
            }
        }
    }

    #[test]
    fn preflight_requires_safety_and_progress_flags_before_paid_work() {
        let dir = tempfile::tempdir().unwrap();
        let mut spec =
            command_for_policy(&AgentKind::Claude, HeadlessExecutionPolicy::PacketOnly).unwrap();
        spec.binary = fake_cli(
            dir.path(),
            "--print --model --output-format text --verbose --tools",
            "",
            0,
        );
        let error = check_command_available(&AgentKind::Claude, &spec, Some("expert"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("--safe-mode"), "{error}");
        assert!(error.contains("--output-format"), "{error}");
        assert!(!dir.path().join("prompt").exists());
    }

    #[test]
    fn policy_probe_rejects_option_prefixes_and_blank_models() {
        let spec = command_for_policy(&AgentKind::Pi, HeadlessExecutionPolicy::PacketOnly).unwrap();
        let help = complete_help().replace("--model ", "--models ");
        assert_eq!(
            missing_command_flags(&AgentKind::Pi, &spec, Some("expert"), &help),
            ["--model"]
        );
        assert!(check_command_available(&AgentKind::Pi, &spec, Some(" ")).is_err());
    }

    #[test]
    fn every_explicit_command_flag_is_probed() {
        for harness in AgentKind::ALL {
            for policy in capabilities(&harness).policies {
                let spec = command_for_policy(&harness, *policy).unwrap();
                assert!(
                    missing_command_flags(&harness, &spec, Some("expert"), complete_help())
                        .is_empty()
                );
                let missing = missing_command_flags(&harness, &spec, Some("expert"), "");
                for flag in spec.args.iter().filter(|arg| arg.starts_with("--")) {
                    assert!(
                        missing.iter().any(|item| item == flag),
                        "{harness:?} {policy:?} missing probe for {flag}"
                    );
                }
                assert!(missing.iter().any(|flag| flag == "--model"));
            }
        }
    }

    #[test]
    fn process_error_wins_over_unparseable_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fail");
        std::fs::write(&script, "#!/bin/sh\ncat >/dev/null\nprintf 'quota exhausted' >&2\nprintf 'not JSON\\n'\nexit 9\n").unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        let spec = HeadlessCommand {
            binary: script.to_string_lossy().into_owned(),
            args: vec![],
            trailing: vec![],
            envs: vec![],
        };
        let error = run_jsonl_command(
            &AgentKind::Codex,
            &spec,
            dir.path(),
            "question",
            None,
            |_| {},
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("quota exhausted"), "{error}");
    }

    #[test]
    fn streamed_error_cannot_be_hidden_by_a_final_message() {
        for harness in AgentKind::ALL {
            let failure = match harness {
                AgentKind::Claude => r#"{"type":"result","is_error":true}"#,
                AgentKind::Codex => r#"{"type":"turn.failed"}"#,
                AgentKind::Opencode => r#"{"type":"error"}"#,
                AgentKind::Pi => r#"{"type":"auto_retry_end","success":false}"#,
            };
            for events in [
                format!("{}\n{failure}", success_event(&harness)),
                format!("{failure}\n{}", success_event(&harness)),
            ] {
                let dir = tempfile::tempdir().unwrap();
                let spec = HeadlessCommand {
                    binary: fake_cli(dir.path(), "", &events, 0),
                    args: vec![],
                    trailing: vec![],
                    envs: vec![],
                };
                let error =
                    run_jsonl_command(&harness, &spec, dir.path(), "question", None, |_| {})
                        .unwrap_err()
                        .to_string();
                assert!(
                    error.contains("headless command failed"),
                    "{harness:?}: {error}"
                );
            }
        }
    }

    #[test]
    fn empty_or_whitespace_answers_are_incomplete() {
        for message in [None, Some("".into()), Some(" \n\t".into())] {
            assert!(
                JsonlOutput {
                    final_message: message,
                    ..Default::default()
                }
                .finish(&AgentKind::Claude)
                .is_err()
            );
        }
    }

    #[test]
    fn malformed_later_error_preserves_first_failure() {
        let mut output = JsonlOutput::default();
        output.record_error(Some("context exceeded".into()), "fallback");
        output.record_error(None, "later failure");
        assert!(
            output
                .finish(&AgentKind::Codex)
                .unwrap_err()
                .to_string()
                .contains("context exceeded")
        );
    }

    #[test]
    fn claude_error_subtype_cannot_be_reported_as_success() {
        let mut output = JsonlOutput::default();
        apply_claude_json_event(
            &serde_json::json!({"type":"result","subtype":"error_max_turns","is_error":false,"result":"partial answer"}),
            &mut output,
            &|_| {},
        );
        assert!(output.finish(&AgentKind::Claude).is_err());
    }

    #[test]
    fn pi_requires_explicit_success_to_recover_a_failed_attempt() {
        for success in [Some(true), Some(false), None] {
            let mut output = JsonlOutput::default();
            for event in [
                serde_json::json!({"type":"message_end","message":{"role":"assistant","stopReason":"error","errorMessage":"rate limit"}}),
                serde_json::from_str(success_event(&AgentKind::Pi)).unwrap(),
                serde_json::json!({"type":"auto_retry_end","success":success}),
            ] {
                apply_pi_json_event(&event, &mut output, &|_| {});
            }
            assert_eq!(output.finish(&AgentKind::Pi).is_ok(), success == Some(true));
        }
    }

    #[test]
    fn pi_retry_success_without_a_new_answer_cannot_return_failed_text() {
        let mut output = JsonlOutput::default();
        for event in [
            serde_json::json!({"type":"message_end","message":{"role":"assistant","stopReason":"error","content":[{"type":"text","text":"partial"}]}}),
            serde_json::json!({"type":"auto_retry_end","success":true}),
        ] {
            apply_pi_json_event(&event, &mut output, &|_| {});
        }
        assert!(output.finish(&AgentKind::Pi).is_err());
    }

    #[test]
    fn pi_truncated_or_cancelled_responses_cannot_be_recovered_as_success() {
        for reason in ["length", "aborted"] {
            let mut output = JsonlOutput::default();
            for event in [
                serde_json::json!({"type":"message_end","message":{"role":"assistant","stopReason":reason,"content":[{"type":"text","text":"partial"}]}}),
                serde_json::json!({"type":"auto_retry_end","success":true}),
            ] {
                apply_pi_json_event(&event, &mut output, &|_| {});
            }
            assert!(output.finish(&AgentKind::Pi).is_err());
        }
    }
}
