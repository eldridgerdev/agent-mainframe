//! Settings-only connection to an existing Codex daemon. Never starts a turn,
//! loads a conversation, changes defaults, or sends terminal input.
use super::discovery;
use crate::{
    model_options::{EligibleOptions, LaunchPath, ModelChoice},
    project::AgentKind,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(super) struct Request {
    pub thread_id: String,
    pub workdir: PathBuf,
    pub choice: ModelChoice,
}

pub(super) trait Prepared: Send {
    fn commit(self: Box<Self>, cancelled: &AtomicBool) -> Result<()>;
}

pub(super) fn prepare(request: &Request, cancelled: &AtomicBool) -> Result<Box<dyn Prepared>> {
    ensure!(!cancelled.load(Ordering::Relaxed), "application cancelled");
    let child = Command::new("codex")
        .args(["app-server", "proxy"])
        .current_dir(&request.workdir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .context("Could not connect to the running Codex daemon")?;
    prepare_with(Rpc::connect(child, cancelled)?, request.clone(), cancelled)
}

trait Protocol: Send {
    fn call(&mut self, method: &str, params: Value, cancelled: &AtomicBool) -> Result<Value>;
    fn initialized(&mut self) -> Result<()>;
}

struct Settings<P> {
    rpc: P,
    request: Request,
    before: Value,
}

fn prepare_with<P: Protocol + 'static>(
    mut rpc: P,
    request: Request,
    cancelled: &AtomicBool,
) -> Result<Box<dyn Prepared>> {
    rpc.call("initialize", json!({"clientInfo":{"name":"amf-model-settings","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}), cancelled)?;
    rpc.initialized()?;
    validate_eligibility(&mut rpc, &request, cancelled)?;
    let before = read_thread(&mut rpc, &request, cancelled)?;
    Ok(Box::new(Settings {
        rpc,
        request,
        before,
    }))
}

fn validate_eligibility(
    rpc: &mut impl Protocol,
    request: &Request,
    cancelled: &AtomicBool,
) -> Result<()> {
    ensure!(
        *request.choice.harness() == AgentKind::Codex && request.choice.reasoning().is_some(),
        "No verified live control for this setting"
    );
    let account = rpc.call("account/read", json!({"refreshToken":false}), cancelled)?;
    ensure!(
        !account["account"].is_null() && account["requiresOpenaiAuth"] == true,
        "Running daemon does not have verified OpenAI authentication"
    );
    let requirements = rpc.call("configRequirements/read", json!({}), cancelled)?;
    ensure!(
        requirements.get("requirements") == Some(&Value::Null),
        "Managed requirements cannot be verified for live application"
    );
    let config = rpc.call(
        "config/read",
        json!({"includeLayers":false,"cwd":request.workdir}),
        cancelled,
    )?;
    ensure!(
        discovery::is_openai_config(&config) && std::env::var_os("OPENAI_BASE_URL").is_none(),
        "Running daemon uses unverified provider configuration"
    );
    let models = rpc.call(
        "model/list",
        json!({"limit":100,"includeHidden":false}),
        cancelled,
    )?;
    ensure!(
        models.get("nextCursor") == Some(&Value::Null),
        "Live catalog pagination is unsupported"
    );
    let caps = discovery::capability_from_models(&models)?;
    EligibleOptions::new(&[AgentKind::Codex], &[caps], LaunchPath::ExistingSession)
        .revalidate(&request.choice)?;
    Ok(())
}

fn read_thread(
    rpc: &mut impl Protocol,
    request: &Request,
    cancelled: &AtomicBool,
) -> Result<Value> {
    let result = rpc.call(
        "thread/read",
        json!({"threadId":request.thread_id,"includeTurns":false}),
        cancelled,
    )?;
    let thread = &result["thread"];
    ensure!(
        thread["id"] == request.thread_id && thread["modelProvider"] == "openai",
        "Live conversation identity or provider changed"
    );
    let cwd = thread["cwd"]
        .as_str()
        .context("Missing live conversation workdir")?;
    ensure!(
        std::fs::canonicalize(cwd)? == std::fs::canonicalize(&request.workdir)?,
        "Live conversation workdir changed"
    );
    ensure!(
        matches!(thread["status"]["type"].as_str(), Some("idle" | "active")),
        "Conversation is not loaded in this daemon; use its own model picker"
    );
    ensure!(
        thread["model"].is_string() && thread["reasoningEffort"].is_string(),
        "Daemon cannot report effective model and effort"
    );
    Ok(thread.clone())
}

impl<P: Protocol> Prepared for Settings<P> {
    fn commit(mut self: Box<Self>, cancelled: &AtomicBool) -> Result<()> {
        // Repeat policy/catalog checks on the same connection, then reject a
        // native-picker change that happened while AMF was validating its target.
        validate_eligibility(&mut self.rpc, &self.request, cancelled)?;
        let current = read_thread(&mut self.rpc, &self.request, cancelled)?;
        ensure!(
            current["model"] == self.before["model"]
                && current["reasoningEffort"] == self.before["reasoningEffort"],
            "Session settings changed; retry analysis before applying"
        );
        ensure!(!cancelled.load(Ordering::Relaxed), "application cancelled");
        // Exactly these fields change. No service tier, permission, collaboration,
        // working-directory or user-default writes are included.
        self.rpc.call("thread/settings/update", json!({"threadId":self.request.thread_id,"model":self.request.choice.model(),"effort":self.request.choice.reasoning()}), cancelled)
            .context("Update could not be confirmed; inspect the harness settings before retrying")?;
        let after = read_thread(&mut self.rpc, &self.request, cancelled)
            .context("Update was sent, but its result could not be verified; inspect the harness settings before retrying")?;
        ensure!(
            after["model"] == self.request.choice.model()
                && after["reasoningEffort"] == self.request.choice.reasoning().unwrap(),
            "Update was sent, but effective settings differ; inspect the harness settings before retrying"
        );
        Ok(())
    }
}

// The CLI proxy is a byte tunnel, not JSONL. Its reader must consume raw
// bytes so WebSocket framing and the HTTP Upgrade handshake remain intact.
struct ProxyIo {
    stdin: ChildStdin,
    rx: mpsc::Receiver<std::io::Result<Vec<u8>>>,
    pending: VecDeque<u8>,
}
impl Read for ProxyIo {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        if self.pending.is_empty() {
            match self.rx.try_recv() {
                Ok(bytes) => self.pending.extend(bytes?),
                Err(mpsc::TryRecvError::Empty) => return Err(std::io::ErrorKind::WouldBlock.into()),
                Err(mpsc::TryRecvError::Disconnected) => return Ok(0),
            }
        }
        let count = buffer.len().min(self.pending.len());
        for byte in buffer.iter_mut().take(count) {
            *byte = self.pending.pop_front().unwrap();
        }
        Ok(count)
    }
}
impl Write for ProxyIo {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.stdin.write(buffer)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.stdin.flush()
    }
}
struct Proxy(Child);
impl Drop for Proxy {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            // SAFETY: this unreaped proxy child owns its dedicated process group.
            unsafe {
                libc::kill(-(self.0.id() as libc::pid_t), libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }
}
struct Rpc<S: Read + Write> {
    socket: tungstenite::WebSocket<S>,
    next: u64,
    deadline: Instant,
    _proxy: Option<Proxy>,
}
impl Rpc<ProxyIo> {
    fn connect(child: Child, cancelled: &AtomicBool) -> Result<Self> {
        let mut proxy = Proxy(child);
        let stdin = proxy.0.stdin.take().context("Missing proxy stdin")?;
        let mut stdout = proxy.0.stdout.take().context("Missing proxy stdout")?;
        // A stalled proxy must not block the worker while writing a frame.
        // SAFETY: stdin owns this open descriptor and fcntl does not retain it.
        let flags = unsafe { libc::fcntl(stdin.as_raw_fd(), libc::F_GETFL) };
        ensure!(flags >= 0, "Could not inspect proxy stdin");
        // SAFETY: same owned descriptor; preserve its existing status flags.
        ensure!(
            unsafe { libc::fcntl(stdin.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
            "Could not make proxy stdin nonblocking"
        );
        let (tx, rx) = mpsc::sync_channel(32);
        std::thread::spawn(move || {
            let mut buffer = [0; 8192];
            loop {
                let read = stdout.read(&mut buffer);
                let finished = !matches!(read, Ok(n) if n > 0);
                if finished {
                    break;
                }
                if tx.send(read.map(|n| buffer[..n].to_vec())).is_err() {
                    break;
                }
            }
        });
        let io = ProxyIo {
            stdin,
            rx,
            pending: VecDeque::new(),
        };
        let mut rpc = Self::connect_stream(io, cancelled)?;
        rpc._proxy = Some(proxy);
        Ok(rpc)
    }
}
fn check_wait(deadline: Instant, cancelled: &AtomicBool) -> Result<()> {
    ensure!(!cancelled.load(Ordering::Relaxed), "application cancelled");
    ensure!(
        Instant::now() < deadline,
        "Codex settings connection timed out"
    );
    Ok(())
}
fn would_block(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(e) if e.kind() == std::io::ErrorKind::WouldBlock)
}
impl<S: Read + Write> Rpc<S> {
    fn connect_stream(io: S, cancelled: &AtomicBool) -> Result<Self> {
        let deadline = Instant::now() + Duration::from_secs(30);
        check_wait(deadline, cancelled)?;
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(4 * 1024 * 1024))
            .max_frame_size(Some(4 * 1024 * 1024));
        let mut handshake =
            tungstenite::client::client_with_config("ws://localhost/", io, Some(config));
        let socket = loop {
            check_wait(deadline, cancelled)?;
            match handshake {
                Ok((socket, _)) => break socket,
                Err(tungstenite::HandshakeError::Interrupted(pending)) => {
                    std::thread::sleep(Duration::from_millis(10));
                    handshake = pending.handshake();
                }
                Err(tungstenite::HandshakeError::Failure(e)) => {
                    return Err(anyhow::anyhow!(e)
                        .context("Could not connect to the running Codex control socket"));
                }
            }
        };
        Ok(Self {
            socket,
            next: 0,
            deadline,
            _proxy: None,
        })
    }
    fn send(&mut self, value: Value, cancelled: &AtomicBool) -> Result<()> {
        check_wait(self.deadline, cancelled)?;
        // write queues a frame even if its first flush would block. Never
        // enqueue that frame again; retry only flush, to avoid duplicate RPCs.
        if let Err(e) = self
            .socket
            .write(tungstenite::Message::text(value.to_string()))
        {
            ensure!(would_block(&e), "Codex control write failed: {e}");
        }
        loop {
            check_wait(self.deadline, cancelled)?;
            match self.socket.flush() {
                Ok(()) => return Ok(()),
                Err(e) if would_block(&e) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(e.into()),
            }
        }
    }
}
impl<S: Read + Write + Send> Protocol for Rpc<S> {
    fn call(&mut self, method: &str, params: Value, cancelled: &AtomicBool) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        self.send(json!({"id":id,"method":method,"params":params}), cancelled)?;
        loop {
            check_wait(self.deadline, cancelled)?;
            match self.socket.read() {
                Ok(tungstenite::Message::Text(text)) => {
                    let value: Value = serde_json::from_str(&text)?;
                    if value["id"] != id {
                        continue;
                    }
                    ensure!(
                        value.get("error").is_none(),
                        "Codex rejected the settings request ({method})"
                    );
                    return value
                        .get("result")
                        .cloned()
                        .context("Missing settings response");
                }
                Ok(tungstenite::Message::Ping(_) | tungstenite::Message::Pong(_)) => {}
                Ok(_) => anyhow::bail!("Unexpected Codex control frame"),
                Err(e) if would_block(&e) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(e.into()),
            }
        }
    }
    fn initialized(&mut self) -> Result<()> {
        self.send(
            json!({"method":"initialized","params":{}}),
            &AtomicBool::new(false),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Trace {
        calls: Vec<(String, Value)>,
        reads: usize,
        catalogs: usize,
        applied: bool,
    }
    struct Fake {
        trace: Arc<Mutex<Trace>>,
        workdir: PathBuf,
        fault: &'static str,
    }
    impl Protocol for Fake {
        fn initialized(&mut self) -> Result<()> {
            Ok(())
        }
        fn call(&mut self, method: &str, params: Value, cancelled: &AtomicBool) -> Result<Value> {
            ensure!(!cancelled.load(Ordering::Relaxed), "application cancelled");
            let mut trace = self.trace.lock().unwrap();
            trace.calls.push((method.into(), params));
            match method {
                "initialize" => Ok(json!({})),
                "account/read" => Ok(if self.fault == "auth" {
                    json!({"account":null,"requiresOpenaiAuth":true})
                } else {
                    json!({"account":{"type":"chatgpt"},"requiresOpenaiAuth":true})
                }),
                "configRequirements/read" => Ok(
                    json!({"requirements":if self.fault == "managed" { json!({"model":"restricted"}) } else { Value::Null }}),
                ),
                "config/read" => Ok(
                    json!({"config":{"model_provider":if self.fault == "provider" { "custom" } else { "openai" }}}),
                ),
                "model/list" => {
                    trace.catalogs += 1;
                    let levels = if self.fault == "effort_removed" && trace.catalogs > 1 {
                        vec![]
                    } else {
                        vec![json!({"reasoningEffort":"low"})]
                    };
                    Ok(
                        json!({"nextCursor":null,"data":[{"hidden":false,"model":"test-model","supportedReasoningEfforts":levels}]}),
                    )
                }
                "thread/read" => {
                    trace.reads += 1;
                    if trace.applied && self.fault == "disconnect" {
                        anyhow::bail!("connection closed");
                    }
                    let model = if self.fault == "drift" && trace.reads > 1 {
                        "other-model"
                    } else {
                        "test-model"
                    };
                    let effort = if trace.applied && self.fault != "clamped" {
                        "low"
                    } else {
                        "high"
                    };
                    Ok(
                        json!({"thread":{"id":if self.fault == "identity" { "other-thread" } else { "thread-1" },"cwd":if self.fault == "cwd" { self.workdir.join("missing") } else { self.workdir.clone() },"modelProvider":"openai","model":model,"reasoningEffort":effort,"status":{"type":if self.fault == "unloaded" { "notLoaded" } else { "active" }}}}),
                    )
                }
                "thread/settings/update" => {
                    if self.fault == "rejected" {
                        anyhow::bail!("unsupported method");
                    }
                    trace.applied = true;
                    Ok(json!({}))
                }
                _ => panic!("Unexpected control method: {method}"),
            }
        }
    }

    fn fixture(fault: &'static str) -> (Fake, Request, Arc<Mutex<Trace>>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let caps = discovery::capability_from_models(&json!({"data":[{"hidden":false,"model":"test-model","supportedReasoningEfforts":[{"reasoningEffort":"low"}]}]})).unwrap();
        let options =
            EligibleOptions::new(&[AgentKind::Codex], &[caps], LaunchPath::ExistingSession);
        let choice = options
            .choices()
            .iter()
            .find(|c| c.reasoning() == Some("low"))
            .unwrap()
            .clone();
        let request = Request {
            thread_id: "thread-1".into(),
            workdir: dir.path().into(),
            choice,
        };
        let trace = Arc::new(Mutex::new(Trace::default()));
        (
            Fake {
                trace: trace.clone(),
                workdir: dir.path().into(),
                fault,
            },
            request,
            trace,
            dir,
        )
    }

    #[test]
    fn live_update_changes_only_model_and_effort_and_verifies_effective_result() {
        let (rpc, request, trace, _dir) = fixture("");
        let cancel = AtomicBool::new(false);
        let prepared = prepare_with(rpc, request, &cancel).unwrap();
        assert!(!trace.lock().unwrap().applied);
        prepared.commit(&cancel).unwrap();
        let trace = trace.lock().unwrap();
        let updates: Vec<_> = trace
            .calls
            .iter()
            .filter(|(m, _)| m == "thread/settings/update")
            .collect();
        assert_eq!(updates.len(), 1);
        assert_eq!(
            updates[0].1,
            json!({"threadId":"thread-1","model":"test-model","effort":"low"})
        );
        assert_eq!(trace.reads, 3);
        assert_eq!(trace.catalogs, 2);
        assert!(!trace.calls.iter().any(|(m, _)| m == "thread/start"
            || m == "thread/resume"
            || m.starts_with("turn/")
            || m.contains("write")));
    }

    #[test]
    fn unknown_auth_policy_provider_and_live_identity_never_send_an_update() {
        for fault in ["auth", "managed", "provider", "identity", "cwd", "unloaded"] {
            let (rpc, request, trace, _dir) = fixture(fault);
            assert!(
                prepare_with(rpc, request, &AtomicBool::new(false)).is_err(),
                "{fault}"
            );
            assert!(
                !trace
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .any(|(m, _)| m == "thread/settings/update")
            );
        }
    }

    #[test]
    fn changed_native_settings_or_removed_effort_fail_before_mutation() {
        for fault in ["drift", "effort_removed"] {
            let (rpc, request, trace, _dir) = fixture(fault);
            let cancel = AtomicBool::new(false);
            let prepared = prepare_with(rpc, request, &cancel).unwrap();
            assert!(prepared.commit(&cancel).is_err());
            assert!(
                !trace
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .any(|(m, _)| m == "thread/settings/update")
            );
        }
    }

    #[test]
    fn cancellation_after_preparation_and_dropping_prepared_control_do_not_apply() {
        let (rpc, request, trace, _dir) = fixture("");
        let cancel = AtomicBool::new(false);
        let prepared = prepare_with(rpc, request, &cancel).unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(prepared.commit(&cancel).is_err());
        assert!(!trace.lock().unwrap().applied);
        let (rpc, request, trace, _dir) = fixture("");
        drop(prepare_with(rpc, request, &AtomicBool::new(false)).unwrap());
        assert!(!trace.lock().unwrap().applied);
    }

    #[test]
    fn rejected_clamped_or_disconnected_updates_do_not_claim_success_or_retry() {
        for fault in ["rejected", "clamped", "disconnect"] {
            let (rpc, request, trace, _dir) = fixture(fault);
            let cancel = AtomicBool::new(false);
            let error = prepare_with(rpc, request, &cancel)
                .unwrap()
                .commit(&cancel)
                .unwrap_err();
            assert!(format!("{error:#}").contains("inspect the harness settings"));
            assert_eq!(
                trace
                    .lock()
                    .unwrap()
                    .calls
                    .iter()
                    .filter(|(m, _)| m == "thread/settings/update")
                    .count(),
                1
            );
        }
    }

    fn websocket_pair(
        replies: Vec<&str>,
    ) -> (
        Rpc<std::os::unix::net::UnixStream>,
        std::thread::JoinHandle<()>,
    ) {
        use std::os::unix::net::UnixStream;
        let (client, server) = UnixStream::pair().unwrap();
        let replies: Vec<String> = replies.into_iter().map(str::to_string).collect();
        let server = std::thread::spawn(move || {
            let mut server = tungstenite::accept(server).unwrap();
            if server.read().is_ok() {
                for reply in replies {
                    server.send(tungstenite::Message::text(reply)).unwrap();
                }
                let _ = server.read();
            }
        });
        client.set_nonblocking(true).unwrap();
        (
            Rpc::connect_stream(client, &AtomicBool::new(false)).unwrap(),
            server,
        )
    }

    #[test]
    fn websocket_rpc_binds_replies_to_ids_ignoring_notifications_and_other_replies() {
        let (mut rpc, server) = websocket_pair(vec![
            r#"{"method":"thread/settings/updated","params":{}}"#,
            r#"{"id":999,"result":{}}"#,
            r#"{"id":1,"result":{"verified":true}}"#,
        ]);
        assert_eq!(
            rpc.call("thread/read", json!({}), &AtomicBool::new(false))
                .unwrap(),
            json!({"verified":true})
        );
        drop(rpc);
        server.join().unwrap();
    }

    #[test]
    fn websocket_wait_is_bounded_and_cancellable_and_rejects_malformed_or_error_replies() {
        let (mut rpc, server) = websocket_pair(vec![]);
        rpc.deadline = Instant::now();
        assert!(
            rpc.call("thread/read", json!({}), &AtomicBool::new(false))
                .is_err()
        );
        drop(rpc);
        server.join().unwrap();
        let (mut rpc, server) = websocket_pair(vec![]);
        assert!(
            rpc.call("thread/read", json!({}), &AtomicBool::new(true))
                .is_err()
        );
        drop(rpc);
        server.join().unwrap();
        for reply in ["not-json", r#"{"id":1,"error":{"code":-32601}}"#] {
            let (mut rpc, server) = websocket_pair(vec![reply]);
            assert!(
                rpc.call("thread/read", json!({}), &AtomicBool::new(false))
                    .is_err()
            );
            drop(rpc);
            server.join().unwrap();
        }
    }

    #[test]
    fn raw_proxy_reads_fragmented_bytes_without_waiting_for_a_newline() {
        let child = Command::new("sh")
            .args(["-c", "sleep 10"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut proxy = Proxy(child);
        let stdin = proxy.0.stdin.take().unwrap();
        let (tx, rx) = mpsc::sync_channel(2);
        let mut io = ProxyIo {
            stdin,
            rx,
            pending: VecDeque::new(),
        };
        let mut buffer = [0; 2];
        assert_eq!(
            io.read(&mut buffer).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
        tx.send(Ok(vec![0x81, 0x02, b'o', b'k'])).unwrap();
        assert_eq!(io.read(&mut buffer).unwrap(), 2);
        assert_eq!(buffer, [0x81, 0x02]);
        assert_eq!(io.read(&mut buffer).unwrap(), 2);
        assert_eq!(buffer, *b"ok");
        drop(tx);
        assert_eq!(io.read(&mut buffer).unwrap(), 0);
    }
}
