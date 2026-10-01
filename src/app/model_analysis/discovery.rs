//! Bounded, cancellable account-specific discovery; never starts a turn.
mod claude;
use crate::{
    model_options::{Availability, HarnessCapability, ModelCapability},
    project::AgentKind,
};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::os::unix::process::CommandExt;
use std::{
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

struct Probe(Child, bool);
impl Probe {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        let status = self.0.try_wait()?;
        self.1 |= status.is_some();
        Ok(status)
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        if self.1 {
            return;
        }
        // SAFETY: this unreaped child owns its dedicated process group.
        unsafe {
            libc::kill(-(self.0.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.0.wait();
    }
}

pub(super) fn discover(
    workdir: &Path,
    allowed: &[AgentKind],
    cancelled: &AtomicBool,
) -> Result<Vec<HarnessCapability>> {
    discover_configured(allowed, cancelled, |harness| match harness {
        AgentKind::Codex => discover_codex(workdir, cancelled),
        AgentKind::Claude => claude::discover(workdir, cancelled),
        _ => Ok(vec![]),
    })
}

fn discover_configured(
    allowed: &[AgentKind],
    cancelled: &AtomicBool,
    mut probe: impl FnMut(&AgentKind) -> Result<Vec<HarnessCapability>>,
) -> Result<Vec<HarnessCapability>> {
    let mut capabilities = vec![];
    let mut error = None;
    for harness in allowed {
        ensure!(!cancelled.load(Ordering::Relaxed), "analysis cancelled");
        if !matches!(harness, AgentKind::Codex | AgentKind::Claude) {
            continue;
        }
        match probe(harness) {
            Ok(found) => capabilities.extend(found),
            Err(e) => error = Some(e),
        }
    }
    ensure!(!cancelled.load(Ordering::Relaxed), "analysis cancelled");
    if !capabilities.iter().any(|c| {
        c.availability == Availability::Available
            && c.models
                .iter()
                .any(|m| m.availability == Availability::Available)
    }) && let Some(error) = error
    {
        return Err(error);
    }
    Ok(capabilities)
}

fn discover_codex(workdir: &Path, cancelled: &AtomicBool) -> Result<Vec<HarnessCapability>> {
    ensure!(!cancelled.load(Ordering::Relaxed), "analysis cancelled");
    let child = Command::new("codex")
        .arg("app-server")
        .current_dir(workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("Codex capability discovery failed")?;
    let mut probe = Probe(child, false);
    let mut stdin = probe
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
    let deadline = Instant::now() + Duration::from_secs(30);
    {
        let mut request = |id: u64, method: &str, params: Value| -> Result<Value> {
            writeln!(
                stdin,
                "{}",
                json!({"id":id,"method":method,"params":params})
            )?;
            stdin.flush()?;
            loop {
                ensure!(!cancelled.load(Ordering::Relaxed), "analysis cancelled");
                ensure!(
                    Instant::now() < deadline,
                    "Codex discovery timed out; retry when ready"
                );
                match rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(line) => {
                        let value: Value = serde_json::from_str(&line?)?;
                        if value.get("id") == Some(&json!(id)) {
                            ensure!(
                                value.get("error").is_none(),
                                "Codex discovery request failed"
                            );
                            return value
                                .get("result")
                                .cloned()
                                .context("missing discovery result");
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => bail!("Codex discovery stopped"),
                }
            }
        };
        request(
            1,
            "initialize",
            json!({"clientInfo":{"name":"amf","version":env!("CARGO_PKG_VERSION")}}),
        )?;
        // The notification must precede the subsequent API calls. Write through
        // the same stdin owner after the handshake.
    }
    writeln!(stdin, "{}", json!({"method":"initialized","params":{}}))?;
    stdin.flush()?;
    // All further messages can be sent together; replies remain ID-bound.
    for (id, method, params) in [
        (2, "account/read", json!({"refreshToken":false})),
        (3, "model/list", json!({"limit":100,"includeHidden":false})),
        (4, "configRequirements/read", json!({})),
        (5, "config/read", json!({"includeLayers":false})),
    ] {
        writeln!(
            stdin,
            "{}",
            json!({"id":id,"method":method,"params":params})
        )?;
    }
    stdin.flush()?;
    let mut account = None;
    let mut models = None;
    let mut requirements = None;
    let mut config = None;
    while account.is_none() || models.is_none() || requirements.is_none() || config.is_none() {
        ensure!(!cancelled.load(Ordering::Relaxed), "analysis cancelled");
        ensure!(
            Instant::now() < deadline,
            "Codex discovery timed out; retry when ready"
        );
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(line) => {
                let v: Value = serde_json::from_str(&line?)?;
                if matches!(v.get("id").and_then(Value::as_u64), Some(2..=5)) {
                    ensure!(
                        v.get("error").is_none(),
                        "Codex account/model discovery failed"
                    );
                    if v["id"] == 2 {
                        account = v.get("result").cloned();
                    } else if v["id"] == 3 {
                        models = v.get("result").cloned();
                    } else if v["id"] == 4 {
                        requirements = v.get("result").cloned();
                    } else {
                        config = v.get("result").cloned();
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => bail!("Codex discovery stopped"),
        }
    }
    let account = account.unwrap();
    // Research applies only to authenticated OpenAI Codex. Custom providers
    // and unknown authentication cannot borrow OpenAI evidence or access.
    if account["account"].is_null() || account["requiresOpenaiAuth"] != true {
        return Ok(vec![]);
    }
    // V1 cannot prove effective settings under managed requirements. Exclude
    // that environment rather than guessing which settings may be clamped.
    let requirements = requirements.unwrap();
    ensure!(
        requirements.get("requirements").is_some(),
        "missing managed requirements result"
    );
    if !requirements["requirements"].is_null()
        || !is_openai_config(&config.unwrap())
        || std::env::var_os("OPENAI_BASE_URL").is_some()
    {
        return Ok(vec![]);
    }
    let models = models.unwrap();
    ensure!(
        models["nextCursor"].is_null(),
        "Codex catalog pagination requires a newer discovery adapter"
    );
    Ok(vec![capability_from_models(&models)?])
}

fn is_openai_config(value: &Value) -> bool {
    let config = &value["config"];
    // Null is the typed API's default OpenAI provider, not a missing response.
    (matches!(config.get("model_provider"), Some(Value::Null))
        || config["model_provider"] == "openai")
        && (config["model_providers"]["openai"].is_null()
            || config["model_providers"]["openai"]
                .as_object()
                .is_some_and(|m| m.is_empty()))
}

fn capability_from_models(value: &Value) -> Result<HarnessCapability> {
    let rows = value["data"].as_array().context("missing Codex models")?;
    let models = rows
        .iter()
        .filter(|v| v["hidden"] == false)
        .map(|v| {
            Ok(ModelCapability {
                model: v["model"]
                    .as_str()
                    .context("missing model identifier")?
                    .into(),
                availability: Availability::Available,
                reasoning_levels: v["supportedReasoningEfforts"].as_array().map(|levels| {
                    levels
                        .iter()
                        .filter_map(|l| l["reasoningEffort"].as_str().map(str::to_string))
                        .collect()
                }),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(HarnessCapability {
        harness: AgentKind::Codex,
        availability: Availability::Available,
        model_flag: true,
        reasoning_flag: true,
        models,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model_options::{EligibleOptions, LaunchPath};
    #[test]
    fn a_failed_harness_does_not_hide_verified_choices_from_another() {
        let cancelled = AtomicBool::new(false);
        let mut probed = vec![];
        let caps = discover_configured(
            &[AgentKind::Codex, AgentKind::Claude, AgentKind::Pi],
            &cancelled,
            |harness| {
                probed.push(harness.clone());
                if *harness == AgentKind::Codex {
                    bail!("Codex protocol failed");
                }
                Ok(vec![HarnessCapability {
                    harness: AgentKind::Claude,
                    availability: Availability::Available,
                    model_flag: true,
                    reasoning_flag: true,
                    models: vec![ModelCapability {
                        model: "claude-sonnet-5-5".into(),
                        availability: Availability::Available,
                        reasoning_levels: Some(vec!["medium".into()]),
                    }],
                }])
            },
        )
        .unwrap();
        assert_eq!(probed, vec![AgentKind::Codex, AgentKind::Claude]);
        assert_eq!(caps.len(), 1);
        assert!(
            discover_configured(&[AgentKind::Claude], &cancelled, |_| bail!(
                "protocol failed"
            ))
            .is_err()
        );
        assert!(
            discover_configured(&[], &cancelled, |_| panic!(
                "empty configuration must not probe"
            ))
            .unwrap()
            .is_empty()
        );
        cancelled.store(true, Ordering::Relaxed);
        assert!(
            discover_configured(&[AgentKind::Claude], &cancelled, |_| panic!("cancelled")).is_err()
        );
    }
    #[test]
    fn unknown_and_custom_provider_settings_cannot_borrow_openai_evidence() {
        assert!(is_openai_config(&json!({"config":{"model_provider":null}})));
        assert!(is_openai_config(
            &json!({"config":{"model_provider":"openai"}})
        ));
        assert!(!is_openai_config(&json!({})));
        assert!(!is_openai_config(
            &json!({"config":{"model_provider":"custom"}})
        ));
        assert!(!is_openai_config(
            &json!({"config":{"model_provider":null,"model_providers":{"openai":{"base_url":"https://example.test"}}}})
        ));
    }
    #[test]
    fn account_catalog_preserves_per_model_controls_and_hidden_models() {
        let cap=capability_from_models(&json!({"data":[{"model":"gpt-6.1-sol","hidden":false,"supportedReasoningEfforts":[{"reasoningEffort":"low"}]},{"model":"hidden","hidden":true},{"model":"unknown","hidden":false}]})).unwrap();
        let options = EligibleOptions::new(&[AgentKind::Codex], &[cap], LaunchPath::Interactive);
        assert!(
            options
                .choices()
                .iter()
                .any(|c| c.model() == "gpt-6.1-sol" && c.reasoning() == Some("low"))
        );
        assert!(
            !options
                .choices()
                .iter()
                .any(|c| c.model() == "hidden" || c.reasoning() == Some("high"))
        );
        assert!(
            options
                .choices()
                .iter()
                .filter(|c| c.model() == "unknown")
                .all(|c| c.reasoning().is_none())
        );
    }
}
