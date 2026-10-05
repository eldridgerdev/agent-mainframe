//! Shared build/test check execution, independent of finishing or dispatching a review.
use std::io::{Read, Result};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::headless::{CHECK_OUTPUT_MAX_CHARS, CheckOutcome};

/// How long output may stay open after the shell exits. Past it, a background
/// descendant (`server & run-tests`) is holding the pipes and EOF may never come.
const OUTPUT_GRACE: Duration = Duration::from_secs(1);

const TRUNCATED: &str = "… (earlier output truncated)";

/// The most recent output of one stream. The end of a build or test log is
/// where its failure summary is, so the tail is kept rather than the head.
#[derive(Debug, Default)]
struct Tail {
    bytes: Vec<u8>,
    dropped: bool,
}

impl Tail {
    // Four bytes per Unicode scalar, plus one to detect truncation.
    const MAX: usize = (CHECK_OUTPUT_MAX_CHARS + 1) * 4;

    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        // Trim in batches so a noisy stream isn't a memmove per read.
        if self.bytes.len() > 2 * Self::MAX {
            self.bytes.drain(..self.bytes.len() - Self::MAX);
            self.dropped = true;
        }
    }

    fn text(&self) -> (String, bool) {
        let mut bytes = &self.bytes[..];
        let mut dropped = self.dropped;
        if bytes.len() > Self::MAX {
            bytes = &bytes[bytes.len() - Self::MAX..];
            dropped = true;
        }
        if dropped {
            // The cut may land inside a multi-byte character.
            let partial = bytes
                .iter()
                .take(3)
                .take_while(|b| *b & 0xC0 == 0x80)
                .count();
            bytes = &bytes[partial..];
        }
        (String::from_utf8_lossy(bytes).trim().to_string(), dropped)
    }
}

/// One drained pipe: output is readable while the reader is still blocked,
/// so a run can report what it has even if EOF never arrives.
#[derive(Debug)]
struct Stream {
    tail: Arc<Mutex<Tail>>,
    done: Receiver<Result<()>>,
    finished: bool,
}

impl Stream {
    fn drain(mut pipe: impl Read + Send + 'static) -> Result<Self> {
        let tail = Arc::new(Mutex::new(Tail::default()));
        let (tx, done) = mpsc::channel();
        let shared = tail.clone();
        std::thread::Builder::new()
            .name("amf-review-check-output".into())
            .spawn(move || {
                let mut buffer = [0; 8192];
                let result = loop {
                    match pipe.read(&mut buffer) {
                        Ok(0) => break Ok(()),
                        Ok(count) => shared.lock().unwrap().push(&buffer[..count]),
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                        Err(error) => break Err(error),
                    }
                };
                let _ = tx.send(result);
            })?;
        Ok(Self {
            tail,
            done,
            finished: false,
        })
    }

    fn receive(&mut self) -> Result<()> {
        if !self.finished {
            match self.done.try_recv() {
                Ok(result) => {
                    self.finished = true;
                    result?;
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    return Err(std::io::Error::other("check output reader disconnected"));
                }
            }
        }
        Ok(())
    }

    fn text(&self) -> (String, bool) {
        self.tail.lock().unwrap().text()
    }
}

/// Joins both streams within `CHECK_OUTPUT_MAX_CHARS`. stderr usually carries
/// the failure diagnostic, so a long stdout can't crowd it out: when both
/// overflow, each keeps the last half of the budget; otherwise the shorter
/// one is kept whole and the other gets the rest.
fn combine_output(stdout: (String, bool), stderr: (String, bool)) -> String {
    let budget = CHECK_OUTPUT_MAX_CHARS;
    let out_len = stdout.0.chars().count();
    let err_len = stderr.0.chars().count();
    let err_keep = err_len.min(budget - out_len.min(budget / 2));
    let out_keep = out_len.min(budget - err_keep);
    [(stdout, out_keep), (stderr, err_keep)]
        .into_iter()
        .filter(|((text, _), _)| !text.is_empty())
        .map(|((text, dropped), keep)| {
            let len = text.chars().count();
            if len <= keep && !dropped {
                return text;
            }
            let start = text
                .char_indices()
                .nth(len - keep.min(len))
                .map_or(text.len(), |(i, _)| i);
            format!("{TRUNCATED}\n{}", text[start..].trim_start())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Drains both pipes while the command runs, retaining only bounded output.
/// Owning the child also makes cancellation and closing terminate/reap it.
#[derive(Debug)]
pub(crate) struct ReviewCheckRun {
    child: Child,
    command: String,
    stdout: Stream,
    stderr: Stream,
    exited: Option<(ExitStatus, Instant)>,
    completed: bool,
}

impl ReviewCheckRun {
    pub(crate) fn spawn(workdir: &Path, command: &str) -> Result<Self> {
        let mut cmd = Command::new("bash");
        cmd.arg("-c")
            .arg(command)
            .current_dir(workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }
        let mut child = cmd.spawn()?;
        let pipes = (|| {
            let stdout = Stream::drain(child.stdout.take().unwrap())?;
            let stderr = Stream::drain(child.stderr.take().unwrap())?;
            Ok((stdout, stderr))
        })();
        let (stdout, stderr) = match pipes {
            Ok(pipes) => pipes,
            Err(error) => {
                stop(&mut child);
                return Err(error);
            }
        };
        Ok(Self {
            child,
            command: command.into(),
            stdout,
            stderr,
            exited: None,
            completed: false,
        })
    }

    pub(crate) fn poll(&mut self) -> Result<Option<CheckOutcome>> {
        let (status, exited_at) = match self.exited {
            Some(exited) => exited,
            None => {
                let Some(status) = self.child.try_wait()? else {
                    return Ok(None);
                };
                *self.exited.insert((status, Instant::now()))
            }
        };
        self.stdout.receive()?;
        self.stderr.receive()?;
        let held_open = !(self.stdout.finished && self.stderr.finished);
        if held_open {
            if exited_at.elapsed() < OUTPUT_GRACE {
                return Ok(None);
            }
            // The command is over; don't wait on an EOF a leftover background
            // process may never give. Stop the group and report what arrived.
            kill_group(&self.child);
        }
        let mut output = combine_output(self.stdout.text(), self.stderr.text());
        if held_open {
            output.push_str(
                "\n… (background processes kept the output open after the command exited; they were stopped)",
            );
        }
        self.completed = true;
        Ok(Some(CheckOutcome {
            command: self.command.clone(),
            passed: status.success(),
            output: output.trim_start().to_string(),
        }))
    }

    #[cfg(test)]
    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }
    #[cfg(test)]
    pub(crate) fn kill(&mut self) -> Result<()> {
        self.child.kill()
    }
    #[cfg(test)]
    pub(crate) fn wait(&mut self) -> Result<std::process::ExitStatus> {
        self.child.wait()
    }
}

fn kill_group(child: &Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
}

fn stop(child: &mut Child) {
    // The shell may have exited while a descendant still owns its output pipes.
    // Terminate the isolated process group as well as reaping the direct child.
    kill_group(child);
    let _ = child.kill();
    let _ = child.wait();
}

impl Drop for ReviewCheckRun {
    fn drop(&mut self) {
        if !self.completed {
            stop(&mut self.child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn finish(run: &mut ReviewCheckRun) -> CheckOutcome {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(outcome) = run.poll().unwrap() {
                return outcome;
            }
            assert!(Instant::now() < deadline, "check did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn assert_not_running(pid: &str) {
        // A killed descendant may remain a zombie until adopted/reaped by init.
        let status = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", pid.trim()])
            .output()
            .unwrap();
        let status = String::from_utf8_lossy(&status.stdout);
        assert!(
            status.trim().is_empty() || status.trim().starts_with('Z'),
            "descendant still running: {status}"
        );
    }

    #[test]
    fn noisy_check_drains_both_pipes_without_deadlocking_and_bounds_unicode_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut run = ReviewCheckRun::spawn(
            dir.path(),
            "for ((i=0;i<15000;i++)); do printf '🦀 output %d\\n' $i; printf 'error %d\\n' $i >&2; done",
        )
        .unwrap();
        let outcome = finish(&mut run);
        assert!(outcome.passed);
        assert!(outcome.output.starts_with(TRUNCATED));
        assert!(outcome.output.contains("🦀 output 14999\n"));
        assert!(outcome.output.ends_with("error 14999"));
        assert!(outcome.output.chars().count() < CHECK_OUTPUT_MAX_CHARS + 100);
        assert!(!outcome.output.contains('�'));
    }

    #[test]
    fn long_stdout_cannot_crowd_out_the_stderr_diagnostic() {
        let dir = tempfile::tempdir().unwrap();
        let mut run = ReviewCheckRun::spawn(
            dir.path(),
            "seq 1 20000; echo 'error[E0308]: mismatched types' >&2; exit 101",
        )
        .unwrap();
        let outcome = finish(&mut run);
        assert!(!outcome.passed);
        assert!(outcome.output.starts_with(TRUNCATED));
        assert!(
            outcome.output.contains("\n20000\n"),
            "stdout keeps its tail"
        );
        assert!(outcome.output.ends_with("error[E0308]: mismatched types"));
        assert!(outcome.output.chars().count() <= CHECK_OUTPUT_MAX_CHARS + TRUNCATED.len() + 2);
    }

    #[test]
    fn combine_output_shares_the_budget_only_when_both_streams_overflow() {
        let budget = CHECK_OUTPUT_MAX_CHARS;
        let short = |s: &str| (s.to_string(), false);
        assert_eq!(combine_output(short("out"), short("err")), "out\nerr");
        assert_eq!(combine_output(short(""), short("err")), "err");
        let long = "x".repeat(budget * 2);
        let output = combine_output(short(&long), short("err"));
        assert_eq!(output.matches('x').count(), budget - 3);
        let output = combine_output(short(&long), short(&"y".repeat(budget * 2)));
        assert_eq!(output.matches('x').count(), budget / 2);
        assert_eq!(output.matches('y').count(), budget / 2);
        let output = combine_output(("tail".into(), true), short(""));
        assert_eq!(output, format!("{TRUNCATED}\ntail"));
    }

    #[test]
    fn background_descendant_holding_the_pipes_cannot_keep_the_check_running() {
        let dir = tempfile::tempdir().unwrap();
        let mut run =
            ReviewCheckRun::spawn(dir.path(), "sleep 30 & echo $! > descendant; echo started")
                .unwrap();
        let outcome = finish(&mut run);
        assert!(outcome.passed);
        assert!(outcome.output.starts_with("started\n"));
        assert!(
            outcome
                .output
                .contains("background processes kept the output open")
        );
        assert_not_running(&std::fs::read_to_string(dir.path().join("descendant")).unwrap());
    }

    #[test]
    fn check_runs_in_checkout_combines_streams_and_reports_nonzero_exit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("input"), "from checkout").unwrap();
        let mut run =
            ReviewCheckRun::spawn(dir.path(), "cat input; printf 'diagnostic' >&2; exit 7")
                .unwrap();
        let outcome = finish(&mut run);
        assert!(!outcome.passed);
        assert_eq!(outcome.output, "from checkout\ndiagnostic");
        assert!(ReviewCheckRun::spawn(&dir.path().join("missing"), "true").is_err());
    }

    #[test]
    fn dropping_check_kills_and_reaps_shell_and_its_children() {
        let dir = tempfile::tempdir().unwrap();
        let run =
            ReviewCheckRun::spawn(dir.path(), "sleep 30 & echo $! > descendant; wait").unwrap();
        let pid = run.id();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !dir.path().join("descendant").exists() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(run);
        assert_eq!(
            unsafe { libc::kill(pid as i32, 0) },
            -1,
            "direct child was reaped"
        );
        assert_not_running(&std::fs::read_to_string(dir.path().join("descendant")).unwrap());
    }
}
