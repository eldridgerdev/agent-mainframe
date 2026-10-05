//! Shared build/test check execution, independent of finishing or dispatching a review.
use std::io::{Read, Result};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use super::headless::{CheckOutcome, truncate_check_output};

/// Drains both pipes while the command runs, retaining only bounded output.
/// Owning the child also makes cancellation and closing terminate/reap it.
#[derive(Debug)]
pub(crate) struct ReviewCheckRun {
    child: Child,
    command: String,
    stdout: Receiver<Result<Vec<u8>>>,
    stderr: Receiver<Result<Vec<u8>>>,
    out: Option<Vec<u8>>,
    err: Option<Vec<u8>>,
    completed: bool,
}

fn drain(mut pipe: impl Read + Send + 'static) -> Result<Receiver<Result<Vec<u8>>>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("amf-review-check-output".into())
        .spawn(move || {
            let result = (|| {
                // Four bytes per Unicode scalar, plus one to detect truncation.
                let max = (super::headless::CHECK_OUTPUT_MAX_CHARS + 1) * 4;
                let mut saved = Vec::new();
                let mut buffer = [0; 8192];
                loop {
                    let count = match pipe.read(&mut buffer) {
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        result => result?,
                    };
                    if count == 0 {
                        break;
                    }
                    let keep = count.min(max.saturating_sub(saved.len()));
                    saved.extend_from_slice(&buffer[..keep]);
                }
                Ok(saved)
            })();
            let _ = tx.send(result);
        })?;
    Ok(rx)
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
            let stdout = drain(child.stdout.take().unwrap())?;
            let stderr = drain(child.stderr.take().unwrap())?;
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
            out: None,
            err: None,
            completed: false,
        })
    }

    pub(crate) fn poll(&mut self) -> Result<Option<CheckOutcome>> {
        let Some(status) = self.child.try_wait()? else {
            return Ok(None);
        };
        fn receive(rx: &Receiver<Result<Vec<u8>>>, saved: &mut Option<Vec<u8>>) -> Result<()> {
            if saved.is_none() {
                match rx.try_recv() {
                    Ok(result) => *saved = Some(result?),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        return Err(std::io::Error::other("check output reader disconnected"));
                    }
                }
            }
            Ok(())
        }
        receive(&self.stdout, &mut self.out)?;
        receive(&self.stderr, &mut self.err)?;
        let (Some(out), Some(err)) = (&self.out, &self.err) else {
            return Ok(None);
        };
        let stdout = String::from_utf8_lossy(out);
        let stderr = String::from_utf8_lossy(err);
        let combined = [stdout.trim(), stderr.trim()]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        self.completed = true;
        Ok(Some(CheckOutcome {
            command: self.command.clone(),
            passed: status.success(),
            output: truncate_check_output(&combined),
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

fn stop(child: &mut Child) {
    // The shell may have exited while a descendant still owns its output pipes.
    // Terminate the isolated process group as well as reaping the direct child.
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
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

    #[test]
    fn noisy_check_drains_both_pipes_without_deadlocking_and_bounds_unicode_output() {
        let dir = tempfile::tempdir().unwrap();
        let mut run = ReviewCheckRun::spawn(
            dir.path(),
            "for ((i=0;i<15000;i++)); do printf '🦀 output\\n'; printf 'error\\n' >&2; done",
        )
        .unwrap();
        let outcome = finish(&mut run);
        assert!(outcome.passed);
        assert!(outcome.output.starts_with("🦀 output"));
        assert!(outcome.output.ends_with("… (truncated)"));
        assert!(outcome.output.chars().count() < 4050);
        assert!(!outcome.output.contains('�'));
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
        let descendant: i32 = std::fs::read_to_string(dir.path().join("descendant"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // A killed descendant may remain a zombie until adopted/reaped by init.
        let status = std::process::Command::new("ps")
            .args(["-o", "stat=", "-p", &descendant.to_string()])
            .output()
            .unwrap();
        let status = String::from_utf8_lossy(&status.stdout);
        assert!(
            status.trim().is_empty() || status.trim().starts_with('Z'),
            "descendant still running: {status}"
        );
    }
}
