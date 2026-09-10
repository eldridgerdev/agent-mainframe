//! Owned, bounded headless attempts for Expert Assist. The UI consumer lands
//! in the next prototype steps; this module is tested without paid providers.
#![allow(dead_code)]

use std::io::{self, Read, Write};
use std::net::Shutdown;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use super::{
    HeadlessExecutionPolicy, HeadlessProgress, HeadlessRunner, HeadlessUsage, JsonlOutput,
};
use crate::project::AgentKind;
use crate::resources::limits::HeadlessLease;

const RUNNING: u8 = 0;
const CANCEL_REQUESTED: u8 = 1;
const FINISHED: u8 = 2;
const POLL_INTERVAL: Duration = Duration::from_millis(10);

// Spawn the watchdog BEFORE the harness. FD 3 is a parent-held lifeline; the actual
// harness cannot inherit it. The watchdog ignores TERM so it can escalate to
// KILL even when the harness ignores TERM. All signals target its own process
// group (0), never a PID recovered from disk. EOF also occurs on parent crash.
// Arguments are positional; no prompt, path, or model is interpolated as code.
const WATCHDOG: &str = r#"
(
    trap '' TERM
    IFS= read -r unused <&3
    kill -TERM 0
    /bin/sleep 0.2
    kill -KILL 0
) </dev/null &
exec 4<&0
"$@" <&4 3<&- 4<&- &
harness=$!
wait "$harness"
exit $?
"#;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HeadlessJobLimits {
    pub prompt_bytes: usize,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub line_bytes: usize,
    pub response_bytes: usize,
    /// Includes the bounded help probe, execution, and pipe draining. Process
    /// termination has a separate 200 ms grace period.
    pub elapsed: Duration,
}

impl Default for HeadlessJobLimits {
    fn default() -> Self {
        Self {
            prompt_bytes: 48 * 1024,
            stdout_bytes: 4 * 1024 * 1024,
            stderr_bytes: 64 * 1024,
            line_bytes: 256 * 1024,
            response_bytes: 32 * 1024,
            elapsed: Duration::from_secs(180),
        }
    }
}

impl HeadlessJobLimits {
    pub(crate) fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.prompt_bytes > 0
                && self.stdout_bytes > 0
                && self.stderr_bytes > 0
                && self.line_bytes > 0
                && self.response_bytes > 0
                && !self.elapsed.is_zero(),
            "headless job limits must be positive"
        );
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct HeadlessJobRequest {
    pub harness: AgentKind,
    /// Explicit executable/path from the profile; no automatic binary/model
    /// fallback or subprocess discovery outside the attempt deadline.
    pub binary: String,
    pub policy: HeadlessExecutionPolicy,
    pub model: String,
    pub workdir: PathBuf,
    pub prompt: String,
    pub limits: HeadlessJobLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadlessJobStatus {
    Completed,
    Failed,
    Incomplete,
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone)]
pub struct HeadlessJobResult {
    pub status: HeadlessJobStatus,
    /// Present only after clean exit, terminal signal, and nonempty response.
    pub response: Option<String>,
    pub error: Option<String>,
    pub usage: HeadlessUsage,
    /// Existing counters do not establish complete billable attribution.
    pub usage_complete: bool,
    pub elapsed: Duration,
}

#[derive(Debug, Clone, Default)]
pub struct HeadlessJobProgress {
    pub activity: Option<String>,
    pub usage: HeadlessUsage,
}

pub struct HeadlessJobHandle {
    state: Arc<AtomicU8>,
    progress: Arc<Mutex<HeadlessJobProgress>>,
    result: mpsc::Receiver<HeadlessJobResult>,
}

impl HeadlessJobHandle {
    /// True only if cancellation won the race with terminal completion.
    pub fn cancel(&self) -> bool {
        self.state
            .compare_exchange(
                RUNNING,
                CANCEL_REQUESTED,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
    }

    /// Coalesced state, not an unbounded queue of provider events.
    pub fn progress(&self) -> HeadlessJobProgress {
        self.progress
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn try_result(&self) -> Result<Option<HeadlessJobResult>> {
        match self.result.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                anyhow::bail!("headless job result already collected or worker disconnected")
            }
        }
    }
}

impl Drop for HeadlessJobHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl HeadlessRunner {
    /// Starts no paid work until the owned help probe verifies the profile.
    /// Caller handles feature/session admission and durable attempt ownership.
    pub fn start_job(request: HeadlessJobRequest) -> Result<HeadlessJobHandle> {
        anyhow::ensure!(
            request.policy != HeadlessExecutionPolicy::Ordinary,
            "owned consultations require an explicit execution policy"
        );
        Self::capabilities(&request.harness).access_for(request.policy)?;
        anyhow::ensure!(
            !request.binary.trim().is_empty() && !request.model.trim().is_empty(),
            "an explicit executable and model are required"
        );
        request.limits.validate()?;
        anyhow::ensure!(
            request.prompt.len() <= request.limits.prompt_bytes,
            "rendered prompt exceeds the byte limit"
        );
        let state = Arc::new(AtomicU8::new(RUNNING));
        let progress = Arc::new(Mutex::new(HeadlessJobProgress::default()));
        let (tx, result) = mpsc::sync_channel(1);
        let worker_state = state.clone();
        let worker_progress = progress.clone();
        std::thread::Builder::new()
            .name("amf-headless-job".into())
            .spawn(move || {
                let started = Instant::now();
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_attempt(&request, &worker_state, &worker_progress, started)
                }));
                let mut result = match outcome {
                    Ok(result) => result,
                    Err(_) => HeadlessJobResult {
                        status: HeadlessJobStatus::Failed,
                        response: None,
                        error: Some(
                            "headless worker panicked; owned process cleanup requested".into(),
                        ),
                        usage: worker_progress
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .usage
                            .clone(),
                        usage_complete: false,
                        elapsed: started.elapsed(),
                    },
                };
                if worker_state
                    .compare_exchange(RUNNING, FINISHED, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    result.status = HeadlessJobStatus::Cancelled;
                    result.response = None;
                    result.error = Some("consultation cancelled".into());
                    worker_state.store(FINISHED, Ordering::SeqCst);
                }
                let _ = tx.send(result);
            })
            .context("could not start headless worker")?;
        Ok(HeadlessJobHandle {
            state,
            progress,
            result,
        })
    }
}

struct GuardedChild {
    child: Child,
    lifeline: UnixStream,
    // Includes preflight and cleanup, not just the synchronous launch call.
    _lease: HeadlessLease,
}

impl GuardedChild {
    fn spawn(binary: &str, args: &[String], envs: &[(&str, &str)], workdir: &Path) -> Result<Self> {
        let (parent, guard) = UnixStream::pair()?;
        let guard_fd = guard.as_raw_fd();
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", WATCHDOG, "amf-headless-watchdog"])
            .arg(binary)
            .args(args)
            .envs(envs.iter().copied())
            .env_remove("ENV")
            .env_remove("BASH_ENV")
            .current_dir(workdir)
            .process_group(0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // SAFETY: only async-signal-safe descriptor syscalls run after fork.
        // The original sockets are CLOEXEC. Only the read endpoint becomes
        // FD 3; the watchdog never inherits the parent's write endpoint.
        unsafe {
            command.pre_exec(move || {
                if libc::dup2(guard_fd, 3) < 0 || libc::fcntl(3, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let lease = HeadlessLease::acquire();
        let child = command
            .spawn()
            .context("could not start owned headless process")?;
        drop(guard);
        Ok(Self {
            child,
            lifeline: parent,
            _lease: lease,
        })
    }

    fn request_cleanup(&self) {
        let _ = self.lifeline.shutdown(Shutdown::Write);
    }
}

impl Drop for GuardedChild {
    fn drop(&mut self) {
        self.request_cleanup();
        // This is always on the worker, never the UI. EOF acknowledges that
        // the watchdog has closed FD 3 (normally via its final group KILL).
        let _ = self.lifeline.set_read_timeout(Some(Duration::from_secs(3)));
        let mut byte = [0];
        if self.lifeline.read(&mut byte).is_err() && matches!(self.child.try_wait(), Ok(None)) {
            // The child is still owned and unreaped, so its process-group ID
            // cannot have been recycled. Do not signal a recovered/stale PID.
            unsafe {
                libc::kill(-(self.child.id() as i32), libc::SIGKILL);
            }
        }
        let _ = self.child.wait();
    }
}

fn nonblocking(fd: &impl AsRawFd) -> Result<()> {
    // SAFETY: fcntl changes only this owned pipe's descriptor flags.
    let current = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) };
    anyhow::ensure!(current >= 0, "could not read pipe flags");
    let result = unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, current | libc::O_NONBLOCK) };
    anyhow::ensure!(result >= 0, "could not make headless pipe nonblocking");
    Ok(())
}

#[derive(Debug)]
struct AttemptFailure(HeadlessJobStatus, String);

impl From<anyhow::Error> for AttemptFailure {
    fn from(error: anyhow::Error) -> Self {
        Self(HeadlessJobStatus::Failed, format!("{error:#}"))
    }
}

type AttemptResult<T> = std::result::Result<T, AttemptFailure>;

fn stop_reason(
    state: &AtomicU8,
    started: Instant,
    limits: &HeadlessJobLimits,
) -> AttemptResult<()> {
    if state.load(Ordering::SeqCst) == CANCEL_REQUESTED {
        return Err(AttemptFailure(
            HeadlessJobStatus::Cancelled,
            "consultation cancelled".into(),
        ));
    }
    if started.elapsed() >= limits.elapsed {
        return Err(AttemptFailure(
            HeadlessJobStatus::TimedOut,
            "consultation deadline exceeded".into(),
        ));
    }
    Ok(())
}

/// One bounded read per tick keeps stderr/stdin/cancellation from being starved
/// by a provider continuously flooding stdout. Returns true on EOF.
fn read_chunk(
    reader: &mut impl Read,
    total: &mut usize,
    limit: usize,
    consume: &mut impl FnMut(&[u8]) -> AttemptResult<()>,
) -> AttemptResult<bool> {
    let mut bytes = [0; 8192];
    match reader.read(&mut bytes) {
        Ok(0) => Ok(true),
        Ok(count) => {
            if count > limit.saturating_sub(*total) {
                return Err(AttemptFailure(
                    HeadlessJobStatus::Incomplete,
                    "headless output exceeded a byte limit".into(),
                ));
            }
            *total += count;
            consume(&bytes[..count])?;
            Ok(false)
        }
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
            ) =>
        {
            Ok(false)
        }
        Err(error) => Err(anyhow::Error::new(error)
            .context("could not read headless pipe")
            .into()),
    }
}

fn run_process(
    request: &HeadlessJobRequest,
    spec: &super::HeadlessCommand,
    args: &[String],
    prompt: &[u8],
    state: &AtomicU8,
    started: Instant,
    consume: &mut impl FnMut(&[u8]) -> AttemptResult<()>,
) -> AttemptResult<(ExitStatus, String)> {
    stop_reason(state, started, &request.limits)?;
    let mut process = GuardedChild::spawn(&spec.binary, args, &spec.envs, &request.workdir)?;
    let mut stdin = process.child.stdin.take();
    let mut stdout = process
        .child
        .stdout
        .take()
        .context("missing headless stdout")?;
    let mut stderr = process
        .child
        .stderr
        .take()
        .context("missing headless stderr")?;
    nonblocking(&stdout)?;
    nonblocking(&stderr)?;
    if let Some(pipe) = &stdin {
        nonblocking(pipe)?;
    }
    let mut written = 0;
    let mut stdout_bytes = 0;
    let mut stderr_bytes = 0;
    let mut diagnostics = Vec::new();
    let mut stdout_done = false;
    let mut stderr_done = false;
    let mut status = None;
    let mut write_error = None;
    loop {
        stop_reason(state, started, &request.limits)?;
        if let Some(pipe) = &mut stdin {
            if written == prompt.len() {
                stdin = None;
            } else {
                let end = written.saturating_add(8192).min(prompt.len());
                match pipe.write(&prompt[written..end]) {
                    Ok(0) => {
                        write_error = Some("headless stdin closed".to_string());
                        stdin = None;
                    }
                    Ok(count) => written += count,
                    Err(error)
                        if matches!(
                            error.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                        ) => {}
                    Err(error) => {
                        write_error = Some(error.to_string());
                        stdin = None;
                    }
                }
            }
        }
        if !stdout_done {
            stdout_done = read_chunk(
                &mut stdout,
                &mut stdout_bytes,
                request.limits.stdout_bytes,
                consume,
            )?;
        }
        if !stderr_done {
            stderr_done = read_chunk(
                &mut stderr,
                &mut stderr_bytes,
                request.limits.stderr_bytes,
                &mut |bytes| {
                    diagnostics.extend_from_slice(bytes);
                    Ok(())
                },
            )?;
        }
        if status.is_none() {
            status = process
                .child
                .try_wait()
                .context("could not poll headless child")?;
            if status.is_some() {
                process.request_cleanup();
            }
        }
        if let Some(status) = status
            && stdout_done
            && stderr_done
        {
            if status.success() && (written != prompt.len() || write_error.is_some()) {
                return Err(AttemptFailure(
                    HeadlessJobStatus::Failed,
                    format!(
                        "could not send headless prompt: {}",
                        write_error.unwrap_or_else(|| {
                            "process exited before the full prompt was written".into()
                        })
                    ),
                ));
            }
            return Ok((status, String::from_utf8_lossy(&diagnostics).into_owned()));
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

struct EventReader<'a> {
    request: &'a HeadlessJobRequest,
    output: JsonlOutput,
    line: Vec<u8>,
    progress: &'a Mutex<HeadlessJobProgress>,
}

impl EventReader<'_> {
    fn consume(&mut self, bytes: &[u8]) -> AttemptResult<()> {
        for &byte in bytes {
            if byte == b'\n' {
                self.line()?;
            } else {
                if self.line.len() >= self.request.limits.line_bytes {
                    return Err(AttemptFailure(
                        HeadlessJobStatus::Incomplete,
                        "headless JSONL line exceeded its byte limit".into(),
                    ));
                }
                self.line.push(byte);
            }
        }
        Ok(())
    }

    fn line(&mut self) -> AttemptResult<()> {
        if self.line.iter().all(u8::is_ascii_whitespace) {
            self.line.clear();
            return Ok(());
        }
        let event = serde_json::from_slice(&self.line).map_err(|_| {
            AttemptFailure(
                HeadlessJobStatus::Incomplete,
                "harness emitted invalid JSONL".into(),
            )
        })?;
        super::apply_jsonl_event(&self.request.harness, &event, &mut self.output, &|event| {
            let mut progress = self.progress.lock().unwrap_or_else(|e| e.into_inner());
            match event {
                HeadlessProgress::Activity(_) => {
                    progress.activity = Some("Examining consultation evidence".into())
                }
                HeadlessProgress::Usage(usage) => progress.usage = usage,
            }
        });
        self.line.clear();
        if self
            .output
            .final_message
            .as_ref()
            .is_some_and(|text| text.len() > self.request.limits.response_bytes)
        {
            self.output.final_message = None;
            return Err(AttemptFailure(
                HeadlessJobStatus::Incomplete,
                "expert response exceeded its byte limit".into(),
            ));
        }
        Ok(())
    }
}

fn run_attempt(
    request: &HeadlessJobRequest,
    state: &AtomicU8,
    progress: &Mutex<HeadlessJobProgress>,
    started: Instant,
) -> HeadlessJobResult {
    let mut reader = EventReader {
        request,
        output: JsonlOutput::default(),
        line: Vec::new(),
        progress,
    };
    let result: AttemptResult<String> = (|| {
        let spec = super::policy::command_for_policy_with_binary(
            &request.harness,
            request.policy,
            Some(&request.binary),
        )?;
        let args: Vec<String> = match request.harness {
            AgentKind::Codex => vec!["exec".into(), "--help".into()],
            AgentKind::Opencode => vec!["run".into(), "--help".into()],
            _ => vec!["--help".into()],
        };
        progress.lock().unwrap_or_else(|e| e.into_inner()).activity =
            Some("Checking expert profile".into());
        let mut help = Vec::new();
        let (status, stderr) =
            run_process(request, &spec, &args, &[], state, started, &mut |bytes| {
                help.extend_from_slice(bytes);
                Ok(())
            })?;
        if !status.success() {
            return Err(anyhow::anyhow!(
                "harness could not describe its headless mode: {}",
                stderr.trim()
            )
            .into());
        }
        let help = format!("{}\n{}", String::from_utf8_lossy(&help), stderr);
        let missing = super::policy::missing_command_flags(
            &request.harness,
            &spec,
            Some(&request.model),
            &help,
        );
        if !missing.is_empty() {
            return Err(anyhow::anyhow!(
                "harness lacks required policy/progress flags: {}",
                missing.join(", ")
            )
            .into());
        }
        let args = super::assemble_jsonl_args(&request.harness, &spec, Some(&request.model));
        let (status, stderr) = run_process(
            request,
            &spec,
            &args,
            request.prompt.as_bytes(),
            state,
            started,
            &mut |bytes| reader.consume(bytes),
        )?;
        if !status.success() {
            return Err(anyhow::anyhow!("expert process failed: {}", stderr.trim()).into());
        }
        // A final non-newline-terminated event is legal JSONL.
        reader.line()?;
        if reader.output.event_error.is_some() || reader.output.retryable_error.is_some() {
            let message = reader
                .output
                .event_error
                .as_ref()
                .or(reader.output.retryable_error.as_ref())
                .unwrap();
            return Err(AttemptFailure(HeadlessJobStatus::Failed, message.clone()));
        }
        if !reader.output.terminal_complete {
            return Err(AttemptFailure(
                HeadlessJobStatus::Incomplete,
                "harness exited without a successful terminal event".into(),
            ));
        }
        reader
            .output
            .final_message
            .take()
            .filter(|text| !text.trim().is_empty())
            .ok_or_else(|| {
                AttemptFailure(
                    HeadlessJobStatus::Incomplete,
                    "expert returned no final answer".into(),
                )
            })
    })();
    let (status, response, error) = match result {
        Ok(text) => (HeadlessJobStatus::Completed, Some(text), None),
        Err(AttemptFailure(status, message)) => (status, None, Some(message)),
    };
    HeadlessJobResult {
        status,
        response,
        error,
        usage: reader.output.usage,
        usage_complete: false,
        elapsed: started.elapsed(),
    }
}

#[cfg(test)]
mod tests;
