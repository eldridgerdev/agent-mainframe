//! Read-only SDK controls: initialize, verify a model, inspect applied effort.
//! No user messages, tool execution, settings writes or persisted session.
use super::*;
use crate::claude::ClaudeLauncher;
use std::process::ChildStdin;

const OVERRIDES: &[&str] = &[
    "ANTHROPIC_BASE_URL",
    "CLAUDE_CODE_USE_BEDROCK",
    "CLAUDE_CODE_USE_VERTEX",
    "CLAUDE_CODE_USE_FOUNDRY",
    "CLAUDE_CODE_USE_MANTLE",
    "CLAUDE_CODE_PROVIDER_MANAGED_BY_HOST",
    "CLAUDE_CODE_EFFORT_LEVEL",
    "CLAUDE_CODE_EXTRA_BODY",
];

pub(super) fn discover(workdir: &Path, cancelled: &AtomicBool) -> Result<Vec<HarnessCapability>> {
    if OVERRIDES
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        return Ok(vec![]);
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    let binary =
        ClaudeLauncher::resolve_binary_with(|path| version_available(path, cancelled, deadline));
    discover_with_deadline(&binary, workdir, cancelled, deadline)
}

fn version_available(path: &Path, cancelled: &AtomicBool, deadline: Instant) -> bool {
    if cancelled.load(Ordering::Relaxed) || Instant::now() >= deadline {
        return false;
    }
    let Ok(child) = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
    else {
        return false;
    };
    let mut probe = Probe(child, false);
    let deadline = deadline.min(Instant::now() + Duration::from_secs(2));
    while !cancelled.load(Ordering::Relaxed) && Instant::now() < deadline {
        match probe.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return false,
        }
    }
    false
}

struct Controls<'a> {
    _probe: Probe,
    stdin: ChildStdin,
    rx: mpsc::Receiver<std::io::Result<String>>,
    deadline: Instant,
    cancelled: &'a AtomicBool,
}

impl Controls<'_> {
    fn request(&mut self, id: &str, request: Value) -> Result<Option<Value>> {
        ensure!(
            !self.cancelled.load(Ordering::Relaxed),
            "analysis cancelled"
        );
        writeln!(
            self.stdin,
            "{}",
            json!({"type":"control_request","request_id":id,"request":request})
        )?;
        self.stdin.flush()?;
        loop {
            ensure!(
                !self.cancelled.load(Ordering::Relaxed),
                "analysis cancelled"
            );
            ensure!(
                Instant::now() < self.deadline,
                "Claude discovery timed out; retry when ready"
            );
            match self.rx.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    let value: Value = serde_json::from_str(&line?)?;
                    ensure!(
                        value["type"] != "control_request",
                        "Claude discovery requires an unsupported dialog"
                    );
                    if value["type"] == "control_response" && value["response"]["request_id"] == id
                    {
                        let response = &value["response"];
                        return match response["subtype"].as_str() {
                            Some("error") => Ok(None),
                            Some("success") => Ok(Some(
                                response.get("response").cloned().unwrap_or(Value::Null),
                            )),
                            _ => bail!("unrecognized Claude discovery response"),
                        };
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => bail!("Claude capability discovery stopped; check your CLI version"),
            }
        }
    }
}

#[cfg(test)]
fn discover_with_binary(
    binary: &str,
    workdir: &Path,
    cancelled: &AtomicBool,
) -> Result<Vec<HarnessCapability>> {
    discover_with_deadline(
        binary,
        workdir,
        cancelled,
        Instant::now() + Duration::from_secs(30),
    )
}

fn discover_with_deadline(
    binary: &str,
    workdir: &Path,
    cancelled: &AtomicBool,
    deadline: Instant,
) -> Result<Vec<HarnessCapability>> {
    ensure!(!cancelled.load(Ordering::Relaxed), "analysis cancelled");
    ensure!(
        Instant::now() < deadline,
        "Claude discovery timed out; retry when ready"
    );
    let child = Command::new(binary)
        .args([
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--no-chrome",
            "--disable-slash-commands",
            "--permission-mode",
            "dontAsk",
            "--tools",
            "",
            "--strict-mcp-config",
            "--mcp-config",
            "{\"mcpServers\":{}}",
            "--no-session-persistence",
            "--settings",
            "{\"disableAllHooks\":true}",
            "--effort",
            "high",
        ])
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("Claude capability discovery failed")?;
    let mut probe = Probe(child, false);
    let stdin = probe
        .0
        .stdin
        .take()
        .context("discovery stdin unavailable")?;
    let stdout = probe
        .0
        .stdout
        .take()
        .context("discovery stdout unavailable")?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut controls = Controls {
        _probe: probe,
        stdin,
        rx,
        deadline,
        cancelled,
    };
    let initialized = controls
        .request("initialize", json!({"subtype":"initialize"}))?
        .context("Claude initialization failed; check your CLI version")?;
    if !authenticated_first_party(&initialized) {
        return Ok(vec![]);
    }
    let settings = controls
        .request("settings", json!({"subtype":"get_settings"}))?
        .context("Claude cannot report effective settings; update the CLI")?;
    ensure!(
        settings["effective"].is_object() && settings["sources"].is_array(),
        "missing Claude effective settings"
    );
    ensure!(
        settings["effective"]["disableAllHooks"] == true,
        "Claude discovery cannot disable hooks"
    );
    ensure!(
        settings["errors"].is_null() || settings["errors"] == json!([]),
        "Claude settings contain errors"
    );
    if !first_party_settings(&settings) {
        return Ok(vec![]);
    }
    let mut models = candidate_models(&initialized)?;
    for (index, model) in models.iter_mut().enumerate() {
        // Picker membership is insufficient. The SDK checks explicit access;
        // substitutions are rejected by comparing the applied wire identity.
        if controls
            .request(
                &format!("switch-{index}"),
                json!({"subtype":"set_model","model":model.model}),
            )?
            .is_none()
        {
            model.availability = Availability::Unavailable;
            continue;
        }
        let applied = controls
            .request(
                &format!("effort-{index}"),
                json!({"subtype":"get_settings"}),
            )?
            .context("Claude cannot report applied effort")?;
        restrict_to_applied(model, &applied);
    }
    Ok(vec![HarnessCapability {
        harness: AgentKind::Claude,
        availability: Availability::Available,
        model_flag: true,
        reasoning_flag: true,
        models,
    }])
}

fn authenticated_first_party(value: &Value) -> bool {
    let account = &value["account"];
    account["apiProvider"] == "firstParty"
        && ["subscriptionType", "tokenSource", "apiKeySource"]
            .iter()
            .any(|key| {
                account[key]
                    .as_str()
                    .is_some_and(|v| !v.is_empty() && v != "none")
            })
}

fn first_party_settings(value: &Value) -> bool {
    let settings = &value["effective"];
    (settings["modelOverrides"].is_null() || settings["modelOverrides"] == json!({}))
        && OVERRIDES
            .iter()
            .all(|key| settings["env"].get(key).is_none())
}

fn candidate_models(value: &Value) -> Result<Vec<ModelCapability>> {
    let rows = value["models"]
        .as_array()
        .context("missing Claude model catalog")?;
    ensure!(rows.len() <= 50, "Claude catalog exceeds discovery limit");
    let mut models = vec![];
    for row in rows {
        if row["value"] == "default" || row["disabled"] == true {
            continue;
        }
        let Some(model) = row["resolvedModel"].as_str() else {
            continue;
        };
        if !model.starts_with("claude-") {
            continue;
        }
        let reasoning_levels = if row["supportsEffort"] == true {
            row["supportedEffortLevels"].as_array().map(|levels| {
                levels
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
        } else {
            None
        };
        let model = ModelCapability {
            model: model.into(),
            availability: Availability::Available,
            reasoning_levels,
        };
        if !models.contains(&model) {
            models.push(model);
        }
    }
    Ok(models)
}

fn restrict_to_applied(model: &mut ModelCapability, settings: &Value) {
    let applied = &settings["applied"];
    if applied["model"] != model.model
        || !first_party_settings(settings)
        || !(settings["errors"].is_null() || settings["errors"] == json!([]))
    {
        model.availability = Availability::Unavailable;
        return;
    }
    // Probe requests high. Applied low/medium proves a cap; never infer
    // xhigh/max behavior from the conventional effort ordering.
    let cap = applied["effort"].as_str().and_then(effort_rank);
    if let Some(levels) = &mut model.reasoning_levels {
        levels.retain(|level| {
            effort_rank(level)
                .zip(cap)
                .is_some_and(|(rank, cap)| rank <= cap)
        });
    }
}

fn effort_rank(level: &str) -> Option<u8> {
    match level {
        "low" => Some(0),
        "medium" => Some(1),
        "high" => Some(2),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_options::{EligibleOptions, LaunchPath};
    use std::os::unix::fs::PermissionsExt;

    fn initialized() -> Value {
        json!({"account":{"apiProvider":"firstParty","subscriptionType":"Claude Pro"},"models":[
            {"value":"default","resolvedModel":"claude-sonnet-5-5","supportsEffort":true,"supportedEffortLevels":["low","medium","high"]},
            {"value":"sonnet","resolvedModel":"claude-sonnet-5-5","supportsEffort":true,"supportedEffortLevels":["low","medium","high","xhigh","max"]},
            {"value":"haiku","resolvedModel":"claude-haiku-4-5-20251001"},
            {"value":"opus","resolvedModel":"claude-opus-5-5","disabled":true},
            {"value":"unresolved","supportsEffort":true,"supportedEffortLevels":["high"]}
        ]})
    }

    #[test]
    fn canonical_picker_metadata_never_infers_an_alias_or_effort_support() {
        let models = candidate_models(&initialized()).unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model, "claude-sonnet-5-5");
        assert!(models[1].reasoning_levels.is_none());
        assert!(
            !models.iter().any(|m| m.model == "default"
                || m.model == "sonnet"
                || m.model == "claude-opus-5-5")
        );
        let mut old = initialized();
        old["models"][1]
            .as_object_mut()
            .unwrap()
            .remove("resolvedModel");
        assert_eq!(candidate_models(&old).unwrap().len(), 1);
        assert!(candidate_models(&json!({})).is_err());
    }

    #[test]
    fn applied_caps_substitution_and_unknown_controls_fail_closed() {
        let model = candidate_models(&initialized()).unwrap().remove(0);
        let mut capped = model.clone();
        restrict_to_applied(
            &mut capped,
            &json!({"effective":{},"applied":{"model":model.model,"effort":"medium"}}),
        );
        assert_eq!(
            capped.reasoning_levels,
            Some(vec!["low".into(), "medium".into()])
        );
        for applied in [
            json!({"model":"claude-opus-5-5","effort":"high"}),
            json!({"model":model.model,"effort":null}),
        ] {
            let mut changed = model.clone();
            restrict_to_applied(&mut changed, &json!({"effective":{},"applied":applied}));
            assert!(
                changed.availability != Availability::Available
                    || changed.reasoning_levels == Some(vec![])
            );
        }
        assert!(!authenticated_first_party(
            &json!({"account":{"apiProvider":"firstParty"}})
        ));
        assert!(!authenticated_first_party(
            &json!({"account":{"apiProvider":"bedrock","subscriptionType":"Claude Pro"}})
        ));
        assert!(authenticated_first_party(&initialized()));
        assert!(!first_party_settings(
            &json!({"effective":{"modelOverrides":{"claude-sonnet-5-5":"custom"}}})
        ));
        assert!(!first_party_settings(
            &json!({"effective":{"env":{"CLAUDE_CODE_EFFORT_LEVEL":"high"}}})
        ));
    }

    fn fake_cli(dir: &Path, wait: bool) -> std::path::PathBuf {
        let binary = dir.join("claude-mock");
        let init = initialized();
        let settings = json!({"effective":{"disableAllHooks":true},"sources":[],"applied":{"model":"claude-sonnet-5-5","effort":"medium"}});
        let reply = |id, response| {
            json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":response}}).to_string()
        };
        let script = format!(
            r#"#!/bin/sh
for arg do printf '%s\n' "$arg" >> '{}/args'; done
while IFS= read -r line; do
case "$line" in
*'"request_id":"initialize"'*) printf '%s\n' '{}' ;;
*'"request_id":"settings"'*|*'"request_id":"effort-0"'*)
case "$line" in *'"request_id":"settings"'*) printf '%s\n' '{}' ;; *) printf '%s\n' '{}' ;; esac ;;
*'"request_id":"switch-0"'*) printf '%s\n' '{}' ;;
*'"request_id":"switch-1"'*) printf '%s\n' '{{"type":"control_response","response":{{"subtype":"error","request_id":"switch-1","error":"restricted"}}}}' ;;
*) exit 9 ;;
esac
done
"#,
            dir.display(),
            reply("initialize", init),
            reply("settings", settings.clone()),
            reply("effort-0", settings),
            reply("switch-0", Value::Null)
        );
        let script = if wait {
            "#!/bin/sh\nwhile IFS= read -r line; do sleep 30; done\n".to_string()
        } else {
            script
        };
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        binary
    }

    #[test]
    fn no_turn_protocol_checks_access_and_applied_cap_before_admitting_choices() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_cli(dir.path(), false);
        let caps = discover_with_binary(
            binary.to_str().unwrap(),
            dir.path(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let choices = EligibleOptions::new(&[AgentKind::Claude], &caps, LaunchPath::Interactive);
        assert_eq!(choices.choices().len(), 3); // unspecified, low, medium
        assert!(
            choices
                .choices()
                .iter()
                .all(|c| c.model() == "claude-sonnet-5-5")
        );
        assert!(
            !choices
                .choices()
                .iter()
                .any(|c| c.reasoning() == Some("high"))
        );
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        for flag in [
            "--no-session-persistence",
            "--strict-mcp-config",
            "--tools",
            "{\"disableAllHooks\":true}",
            "--effort\nhigh",
        ] {
            assert!(args.contains(flag));
        }
    }

    #[test]
    fn cancelling_a_waiting_probe_stops_without_waiting_for_the_child() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_cli(dir.path(), true);
        let cancelled = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                cancelled.store(true, Ordering::Relaxed);
            });
            let start = Instant::now();
            assert!(
                discover_with_binary(binary.to_str().unwrap(), dir.path(), &cancelled).is_err()
            );
            assert!(start.elapsed() < Duration::from_secs(2));
        });
    }
    #[test]
    fn readiness_checks_also_obey_cancellation_and_the_deadline() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("hung-version");
        std::fs::write(&binary, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        let cancelled = AtomicBool::new(false);
        let start = Instant::now();
        assert!(!version_available(
            &binary,
            &cancelled,
            start + Duration::from_millis(50)
        ));
        assert!(start.elapsed() < Duration::from_secs(2));
        cancelled.store(true, Ordering::Relaxed);
        assert!(!version_available(
            &binary,
            &cancelled,
            Instant::now() + Duration::from_secs(30)
        ));
    }
}
