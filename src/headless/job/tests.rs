use super::*;
use std::os::unix::fs::PermissionsExt;

const HELP: &str = "--print --model --safe-mode --tools --permission-mode --no-session-persistence
--output-format text stream-json --verbose
--pure --format json
--no-session --no-tools --no-extensions --no-skills --no-prompt-templates
--no-context-files --no-approve --mode json
--sandbox read-only --ephemeral --skip-git-repo-check --color never --json";
const ANSWER: &str = r#"{"type":"result","subtype":"success","is_error":false,"result":"advice","usage":{"input_tokens":12,"output_tokens":3}}"#;

fn request(dir: &Path, body: &str) -> HeadlessJobRequest {
    let script = dir.join("fake-harness");
    std::fs::write(&script, format!("#!/bin/sh\nfor arg in \"$@\"; do\nif [ \"$arg\" = --help ]; then\ncat <<'HELP'\n{HELP}\nHELP\nexit 0\nfi\ndone\n{body}\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    HeadlessJobRequest {
        harness: AgentKind::Claude,
        binary: script.to_string_lossy().into_owned(),
        policy: HeadlessExecutionPolicy::PacketOnly,
        model: "explicit-expert".into(),
        workdir: dir.to_path_buf(),
        prompt: "private\nquestion".into(),
        limits: HeadlessJobLimits {
            elapsed: Duration::from_secs(5),
            ..Default::default()
        },
    }
}

fn answer_body() -> String {
    format!("cat > received-prompt\ncat <<'ANSWER'\n{ANSWER}\nANSWER")
}

fn wait_result(handle: &HeadlessJobHandle) -> HeadlessJobResult {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(result) = handle.try_result().unwrap() {
            return result;
        }
        assert!(Instant::now() < deadline, "owned job did not terminate");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_file(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(text) = std::fs::read_to_string(path)
            && !text.trim().is_empty()
        {
            return text;
        }
        assert!(Instant::now() < deadline, "missing {}", path.display());
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn running(pid: i32) -> bool {
    // Zombies have no executing work and cannot hold pipe descriptors. Only
    // the direct child can be reaped by this test; init adopts grandchildren.
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "stat="])
        .output()
        .unwrap();
    let state = String::from_utf8_lossy(&output.stdout);
    !state.trim().is_empty() && !state.trim().starts_with('Z')
}

#[test]
fn owned_success_preserves_prompt_usage_and_terminal_result() {
    let dir = tempfile::tempdir().unwrap();
    let handle = HeadlessRunner::start_job(request(dir.path(), &answer_body())).unwrap();
    let result = wait_result(&handle);
    assert_eq!(
        result.status,
        HeadlessJobStatus::Completed,
        "{:?}",
        result.error
    );
    assert_eq!(result.response.as_deref(), Some("advice"));
    assert_eq!(result.usage.input_tokens, Some(12));
    assert!(!result.usage_complete);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("received-prompt")).unwrap(),
        "private\nquestion"
    );
    assert!(!handle.cancel(), "completed job cannot be cancelled");
}

#[test]
fn cancel_wins_and_cleans_descendants_that_ignore_term() {
    let _guard = crate::resources::limits::lock_lease_tests();
    let baseline = crate::resources::limits::wait_for_in_flight(0);
    let dir = tempfile::tempdir().unwrap();
    let handle = HeadlessRunner::start_job(request(
        dir.path(),
        "trap '' TERM\n/bin/sleep 30 &\necho $! > descendant\nwait",
    ))
    .unwrap();
    let pid: i32 = wait_file(&dir.path().join("descendant"))
        .trim()
        .parse()
        .unwrap();
    let started = Instant::now();
    assert!(handle.cancel());
    assert!(
        started.elapsed() < Duration::from_millis(100),
        "cancel blocked the caller"
    );
    let result = wait_result(&handle);
    assert_eq!(result.status, HeadlessJobStatus::Cancelled);
    assert!(result.response.is_none());
    assert!(!running(pid), "descendant survived cancellation");
    assert_eq!(
        crate::resources::limits::wait_for_in_flight(baseline),
        baseline
    );
}

#[test]
fn dropping_the_handle_cancels_without_joining_on_the_caller() {
    let _guard = crate::resources::limits::lock_lease_tests();
    let baseline = crate::resources::limits::wait_for_in_flight(0);
    let dir = tempfile::tempdir().unwrap();
    let handle =
        HeadlessRunner::start_job(request(dir.path(), "echo $$ > root\n/bin/sleep 30")).unwrap();
    let pid: i32 = wait_file(&dir.path().join("root")).trim().parse().unwrap();
    let started = Instant::now();
    drop(handle);
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(
        crate::resources::limits::wait_for_in_flight(baseline),
        baseline
    );
    assert!(!running(pid));
}

#[test]
fn watchdog_handles_parent_lifeline_loss_without_rust_cleanup() {
    let dir = tempfile::tempdir().unwrap();
    let args = vec![
        "-c".into(),
        "trap '' TERM; /bin/sleep 30 & echo $! > descendant; wait".into(),
    ];
    let mut child = GuardedChild::spawn("/bin/sh", &args, &[], dir.path()).unwrap();
    let pid: i32 = wait_file(&dir.path().join("descendant"))
        .trim()
        .parse()
        .unwrap();
    // Closing the writer models the kernel closing AMF's descriptors on exit.
    // Neither cancel nor GuardedChild::drop sends any signal in this test.
    child.lifeline.shutdown(Shutdown::Write).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    // The wrapper can exit on TERM before the resistant descendant receives
    // KILL. Observe both independently through the watchdog's grace period.
    while child.child.try_wait().unwrap().is_none() || running(pid) {
        assert!(
            Instant::now() < deadline,
            "watchdog failed to clean its group"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!running(pid), "watchdog failed to kill descendant");
}

#[test]
fn success_before_the_full_prompt_is_written_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = request(dir.path(), &format!("cat <<'ANSWER'\n{ANSWER}\nANSWER"));
    request.prompt = "x".repeat(200_000);
    request.limits.prompt_bytes = request.prompt.len();
    let result = wait_result(&HeadlessRunner::start_job(request).unwrap());
    assert_eq!(result.status, HeadlessJobStatus::Failed);
    assert!(result.response.is_none());
    assert!(
        result
            .error
            .unwrap()
            .contains("could not send headless prompt")
    );
}

#[test]
fn deadline_covers_a_child_that_never_consumes_stdin() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = request(dir.path(), "/bin/sleep 30");
    request.prompt = "x".repeat(200_000);
    request.limits.prompt_bytes = 200_000;
    request.limits.elapsed = Duration::from_millis(800);
    let result = wait_result(&HeadlessRunner::start_job(request).unwrap());
    assert_eq!(
        result.status,
        HeadlessJobStatus::TimedOut,
        "{:?}",
        result.error
    );
    assert!(result.elapsed < Duration::from_secs(3));
}

#[test]
fn preflight_is_owned_and_obeys_the_same_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = request(dir.path(), &answer_body());
    std::fs::write(&request.binary, "#!/bin/sh\n/bin/sleep 30\n").unwrap();
    request.limits.elapsed = Duration::from_millis(200);
    let result = wait_result(&HeadlessRunner::start_job(request).unwrap());
    assert_eq!(result.status, HeadlessJobStatus::TimedOut);
    assert!(!dir.path().join("received-prompt").exists());
}

#[test]
fn unsupported_profile_and_oversized_prompt_spawn_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let mut request = request(dir.path(), "touch must-not-run");
    request.harness = AgentKind::Codex;
    assert!(HeadlessRunner::start_job(request.clone()).is_err());
    request.harness = AgentKind::Claude;
    request.limits.prompt_bytes = 1;
    assert!(HeadlessRunner::start_job(request).is_err());
    assert!(!dir.path().join("must-not-run").exists());
}

#[test]
fn bounded_output_rejects_oversized_lines_stderr_and_response() {
    for kind in ["line", "stderr", "response", "stream"] {
        let dir = tempfile::tempdir().unwrap();
        let body = match kind {
            "stderr" => "cat >/dev/null; printf '%0300d' 0 >&2".into(),
            "line" => "cat >/dev/null; printf '%0300d' 0".into(),
            "stream" => "cat >/dev/null; while :; do printf '\\n'; done".into(),
            _ => answer_body(),
        };
        let mut request = request(dir.path(), &body);
        match kind {
            "stderr" => request.limits.stderr_bytes = 128,
            "line" => request.limits.line_bytes = 128,
            "stream" => request.limits.stdout_bytes = 1024,
            _ => request.limits.response_bytes = 2,
        }
        let result = wait_result(&HeadlessRunner::start_job(request).unwrap());
        assert_eq!(
            result.status,
            HeadlessJobStatus::Incomplete,
            "{kind}: {:?}",
            result.error
        );
        assert!(result.response.is_none());
    }
}

#[test]
fn invalid_json_or_missing_terminal_signal_cannot_become_ready() {
    for event in ["garbage", r#"{"type":"result","result":"partial"}"#] {
        let dir = tempfile::tempdir().unwrap();
        let request = request(
            dir.path(),
            &format!("cat >/dev/null\ncat <<'EVENT'\n{event}\nEVENT"),
        );
        let result = wait_result(&HeadlessRunner::start_job(request).unwrap());
        assert_eq!(
            result.status,
            HeadlessJobStatus::Incomplete,
            "{:?}",
            result.error
        );
    }
}

#[test]
fn terminal_provider_failure_keeps_usage_and_never_returns_text() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!(
        "cat >/dev/null\ncat <<'EVENTS'\n{ANSWER}\n{{\"type\":\"result\",\"is_error\":true,\"result\":\"quota exhausted\"}}\nEVENTS"
    );
    let result = wait_result(&HeadlessRunner::start_job(request(dir.path(), &body)).unwrap());
    assert_eq!(result.status, HeadlessJobStatus::Failed);
    assert!(result.response.is_none());
    assert_eq!(result.usage.input_tokens, Some(12));
}

#[test]
fn terminal_signal_contract_is_harness_specific() {
    for (harness, events) in [
        (
            AgentKind::Codex,
            vec![
                serde_json::json!({"type":"turn.completed"}),
                serde_json::json!({"type":"turn.started"}),
            ],
        ),
        (
            AgentKind::Opencode,
            vec![
                serde_json::json!({"type":"step_finish","part":{"reason":"stop"}}),
                serde_json::json!({"type":"step_finish","part":{"reason":"tool-calls"}}),
            ],
        ),
        (
            AgentKind::Pi,
            vec![
                serde_json::json!({"type":"agent_end"}),
                serde_json::json!({"type":"turn_start"}),
            ],
        ),
    ] {
        let mut output = JsonlOutput::default();
        super::super::apply_jsonl_event(&harness, &events[0], &mut output, &|_| {});
        assert!(output.terminal_complete);
        super::super::apply_jsonl_event(&harness, &events[1], &mut output, &|_| {});
        assert!(!output.terminal_complete);
    }
}
