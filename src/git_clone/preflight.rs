//! Pre-flight checks for "new project from git URL".
//!
//! Runs `git --version`, then `git ls-remote --symref -- <url> HEAD` with a
//! timeout and a cancel token. Only when that fails with an auth-class error
//! are the diagnostic probes (`gh auth status` for github.com over HTTPS,
//! `ssh-add -l` for SSH) run, purely to add context to the failure.
//!
//! Every child is built by [`GitEnv`] (no prompts, own session/process
//! group); on timeout or cancel the whole group is killed and reaped.

use std::io::Read;
use std::process::{Child, Command, ExitStatus};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::classify::{FailureClass, FixId, advise, classify_stderr};
use super::env::{GitEnv, Output};
use super::url::ParsedUrl;

/// Cooperative cancellation flag shared with the UI.
pub type CancelToken = Arc<AtomicBool>;

pub const DEFAULT_LS_REMOTE_TIMEOUT: Duration = Duration::from_secs(30);
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for the pipe readers once the child is gone.
const READER_GRACE: Duration = Duration::from_secs(2);
const MAX_CAPTURE: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckId {
    Git,
    Remote,
    GhAuth,
    SshAgent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub id: CheckId,
    pub status: CheckStatus,
    pub message: String,
    pub fix: Option<FixId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Remote reachable and has a HEAD.
    Reachable,
    /// Remote reachable but has no refs at all.
    EmptyRepo,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreflightReport {
    pub outcome: Outcome,
    pub checks: Vec<Check>,
    /// Branch HEAD points at on the remote (from `--symref`).
    pub default_branch: Option<String>,
}

impl PreflightReport {
    pub fn has_fail(&self) -> bool {
        self.checks.iter().any(|c| c.status == CheckStatus::Fail)
    }

    pub fn has_warn(&self) -> bool {
        self.checks.iter().any(|c| c.status == CheckStatus::Warn)
    }

    /// Start is allowed when nothing failed (a Warn still allows it).
    pub fn can_start(&self) -> bool {
        self.outcome == Outcome::Reachable && !self.has_fail()
    }
}

/// Injectable knobs; `Default` is production behaviour.
#[derive(Debug, Clone)]
pub struct PreflightOptions {
    pub env: GitEnv,
    pub ls_remote_timeout: Duration,
    pub probe_timeout: Duration,
    pub gh_program: String,
    pub ssh_add_program: String,
}

impl Default for PreflightOptions {
    fn default() -> Self {
        Self {
            env: GitEnv::new(),
            ls_remote_timeout: DEFAULT_LS_REMOTE_TIMEOUT,
            probe_timeout: PROBE_TIMEOUT,
            gh_program: "gh".into(),
            ssh_add_program: "ssh-add".into(),
        }
    }
}

enum Captured {
    Exited {
        status: ExitStatus,
        stdout: String,
        stderr: String,
    },
    TimedOut,
    Cancelled,
    SpawnFailed(std::io::Error),
}

fn kill_group(child: &mut Child) {
    // The child is a session leader (see `GitEnv`), so pgid == pid.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

fn spawn_reader<R: Read + Send + 'static>(stream: Option<R>) -> mpsc::Receiver<String> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(s) = stream {
            let _ = s.take(MAX_CAPTURE).read_to_end(&mut buf);
        }
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    rx
}

/// Run `cmd` (stdout/stderr piped), reading both streams concurrently with
/// the wait. Timeout and cancel kill the whole process group and reap it.
fn run_capture(mut cmd: Command, timeout: Duration, cancel: &AtomicBool) -> Captured {
    if cancel.load(Ordering::SeqCst) {
        return Captured::Cancelled;
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return Captured::SpawnFailed(e),
    };
    let out_rx = spawn_reader(child.stdout.take());
    let err_rx = spawn_reader(child.stderr.take());
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {}
            Err(e) => {
                kill_group(&mut child);
                return Captured::SpawnFailed(e);
            }
        }
        if cancel.load(Ordering::SeqCst) {
            kill_group(&mut child);
            return Captured::Cancelled;
        }
        if start.elapsed() >= timeout {
            kill_group(&mut child);
            return Captured::TimedOut;
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    // A surviving grandchild could hold the pipes open; do not wait forever.
    let stdout = out_rx.recv_timeout(READER_GRACE).unwrap_or_default();
    let stderr = err_rx.recv_timeout(READER_GRACE).unwrap_or_default();
    Captured::Exited {
        status,
        stdout,
        stderr,
    }
}

/// Parse `ref: refs/heads/<branch>\tHEAD` from `ls-remote --symref` output.
pub fn parse_default_branch(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let rest = line.strip_prefix("ref:")?.trim_start();
        let (target, name) = rest.split_once('\t')?;
        if name.trim() != "HEAD" {
            return None;
        }
        let branch = target.trim().strip_prefix("refs/heads/")?;
        (!branch.is_empty()).then(|| branch.to_string())
    })
}

fn fail(id: CheckId, message: impl Into<String>, fix: Option<FixId>) -> Check {
    Check {
        id,
        status: CheckStatus::Fail,
        message: message.into(),
        fix,
    }
}

fn first_line(s: &str) -> &str {
    s.lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
}

/// The report for a containerized clone whose sandbox cannot be used (Docker
/// missing, runtime not registered, image missing or failed to build). Shown
/// as the failed `git` check, since the sandbox is where git runs.
pub fn container_unavailable(message: impl Into<String>) -> PreflightReport {
    PreflightReport {
        outcome: Outcome::Failed,
        checks: vec![fail(CheckId::Git, message, None)],
        default_branch: None,
    }
}

/// Run the pre-flight checks for `url`. Blocking; call from a worker thread.
pub fn run(url: &ParsedUrl, opts: &PreflightOptions, cancel: &CancelToken) -> PreflightReport {
    let mut checks = Vec::new();
    let report = |outcome, checks, default_branch| PreflightReport {
        outcome,
        checks,
        default_branch,
    };

    // 1. git --version
    match run_capture(
        opts.env
            .command(["--version"], Output::Piped, Output::Piped),
        VERSION_TIMEOUT,
        cancel,
    ) {
        Captured::Exited { status, stdout, .. } if status.success() => checks.push(Check {
            id: CheckId::Git,
            status: CheckStatus::Pass,
            message: first_line(&stdout).to_string(),
            fix: None,
        }),
        Captured::Cancelled => return report(Outcome::Cancelled, checks, None),
        Captured::SpawnFailed(e) if e.kind() == std::io::ErrorKind::NotFound => {
            checks.push(fail(
                CheckId::Git,
                "git was not found. Install git and make sure it is on your PATH.",
                None,
            ));
            return report(Outcome::Failed, checks, None);
        }
        _ => {
            checks.push(fail(CheckId::Git, "`git --version` failed.", None));
            return report(Outcome::Failed, checks, None);
        }
    }

    // 2. ls-remote --symref -- <url> HEAD
    let mut args = GitEnv::ls_remote_args(&url.raw);
    args.push("HEAD".into());
    let cmd = opts.env.command(args, Output::Piped, Output::Piped);
    match run_capture(cmd, opts.ls_remote_timeout, cancel) {
        Captured::Cancelled => report(Outcome::Cancelled, checks, None),
        Captured::TimedOut => {
            let advice = advise(&FailureClass::Timeout, url);
            checks.push(fail(
                CheckId::Remote,
                format!(
                    "{} (gave up after {}s) {}",
                    advice.summary,
                    opts.ls_remote_timeout.as_secs(),
                    advice.detail
                ),
                advice.fixes.first().copied(),
            ));
            report(Outcome::Failed, checks, None)
        }
        Captured::SpawnFailed(e) => {
            checks.push(fail(
                CheckId::Remote,
                format!("Could not run git: {e}"),
                None,
            ));
            report(Outcome::Failed, checks, None)
        }
        Captured::Exited {
            status,
            stdout,
            stderr,
        } if status.success() => {
            if stdout.trim().is_empty() {
                let advice = advise(&FailureClass::EmptyRepo, url);
                checks.push(fail(
                    CheckId::Remote,
                    format!("{} {}", advice.summary, advice.detail),
                    None,
                ));
                return report(Outcome::EmptyRepo, checks, None);
            }
            let branch = parse_default_branch(&stdout);
            let message = match &branch {
                Some(b) => format!("Reachable. Default branch: {b}."),
                None => "Reachable. The default branch could not be determined.".to_string(),
            };
            let _ = stderr;
            checks.push(Check {
                id: CheckId::Remote,
                status: CheckStatus::Pass,
                message,
                fix: None,
            });
            report(Outcome::Reachable, checks, branch)
        }
        Captured::Exited { stderr, .. } => {
            let class = classify_stderr(&stderr);
            let advice = advise(&class, url);
            let mut fix = advice.fixes.first().copied();
            let mut message = format!("{} {}", advice.summary, advice.detail);
            let mut probes = Vec::new();
            if class == FailureClass::Auth {
                probe_auth(url, opts, cancel, &mut probes, &mut fix);
                if cancel.load(Ordering::SeqCst) {
                    return report(Outcome::Cancelled, checks, None);
                }
            }
            if let Some(p) = probes.first() {
                message.push(' ');
                message.push_str(&p.message);
            }
            checks.push(fail(CheckId::Remote, message, fix));
            checks.extend(probes);
            report(Outcome::Failed, checks, None)
        }
    }
}

/// Diagnostic probes; run only after an auth-class ls-remote failure.
fn probe_auth(
    url: &ParsedUrl,
    opts: &PreflightOptions,
    cancel: &CancelToken,
    probes: &mut Vec<Check>,
    fix: &mut Option<FixId>,
) {
    let (id, program, args): (CheckId, &str, &[&str]) = if url.is_https() && url.is_github() {
        (CheckId::GhAuth, &opts.gh_program, &["auth", "status"])
    } else if url.is_ssh() {
        (CheckId::SshAgent, &opts.ssh_add_program, &["-l"])
    } else {
        return;
    };
    let cmd = GitEnv::helper_command(program, args, Output::Piped, Output::Piped);
    let check = match run_capture(cmd, opts.probe_timeout, cancel) {
        Captured::Cancelled => return,
        Captured::SpawnFailed(e) => {
            if id == CheckId::GhAuth {
                *fix = None;
            }
            Check {
                id,
                status: CheckStatus::Warn,
                message: format!("`{program}` could not be run ({e}), so credentials were not checked."),
                fix: None,
            }
        }
        Captured::TimedOut => Check {
            id,
            status: CheckStatus::Warn,
            message: format!("`{program}` timed out, so credentials were not checked."),
            fix: None,
        },
        Captured::Exited { status, .. } => match (id, status.code()) {
            (CheckId::GhAuth, Some(0)) => Check {
                id,
                status: CheckStatus::Pass,
                message: "The GitHub CLI is logged in, but git's credentials were rejected; the account may lack access to this repository.".into(),
                fix: None,
            },
            (CheckId::GhAuth, _) => fail(id, "The GitHub CLI is not logged in.", Some(FixId::GhAuthLogin)),
            (_, Some(0)) => Check {
                id,
                status: CheckStatus::Pass,
                message: "An SSH agent holds keys, but the host rejected them; check the key is registered with the host.".into(),
                fix: None,
            },
            (_, Some(1)) => fail(id, "The SSH agent is running but holds no keys.", Some(FixId::SshAdd)),
            (_, _) => fail(id, "No SSH agent is reachable (SSH_AUTH_SOCK is unset or stale).", Some(FixId::SshAdd)),
        },
    };
    probes.push(check);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git_clone::url::UrlKind;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    fn test_env() -> GitEnv {
        GitEnv::new().with_file_transport(true)
    }

    fn git(args: &[&str]) -> String {
        let out = test_env()
            .command(args.iter().copied(), Output::Piped, Output::Piped)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn have_git() -> bool {
        test_env()
            .command(["--version"], Output::Null, Output::Null)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn bare(dir: &Path, name: &str, branch: &str) -> PathBuf {
        let p = dir.join(name);
        git(&[
            "init",
            "--bare",
            "-q",
            &format!("--initial-branch={branch}"),
            p.to_str().unwrap(),
        ]);
        p
    }

    fn commit_on(repo: &Path, branch: &str) {
        let gd = repo.to_str().unwrap();
        let tree = git(&["--git-dir", gd, "mktree"]);
        let c = git(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "--git-dir",
            gd,
            "commit-tree",
            &tree,
            "-m",
            "x",
        ]);
        git(&[
            "--git-dir",
            gd,
            "update-ref",
            &format!("refs/heads/{branch}"),
            &c,
        ]);
    }

    fn url_for(raw: &str, kind: UrlKind, host: &str) -> ParsedUrl {
        ParsedUrl {
            raw: raw.to_string(),
            kind,
            host: host.to_string(),
            port: None,
            repo_name: "r".into(),
        }
    }

    fn file_url(p: &Path) -> ParsedUrl {
        url_for(
            &format!("file://{}", p.display()),
            UrlKind::Https,
            "localhost",
        )
    }

    fn opts() -> PreflightOptions {
        PreflightOptions {
            env: test_env(),
            ..Default::default()
        }
    }

    fn token() -> CancelToken {
        Arc::new(AtomicBool::new(false))
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn alive(pid: i32) -> bool {
        if unsafe { libc::kill(pid, 0) } != 0 {
            return false;
        }
        // An unreaped zombie still answers kill(0).
        match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(s) => !s.rsplit(") ").next().unwrap_or("").starts_with('Z'),
            Err(_) => true,
        }
    }

    fn assert_dies(pid: i32) {
        let start = Instant::now();
        while alive(pid) {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "pid {pid} survived"
            );
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Fake git: answers --version, and for anything else starts a long
    /// grandchild (pid recorded) and waits on it.
    fn sleepy_git(dir: &Path) -> (PathBuf, PathBuf) {
        let pidfile = dir.join("pid");
        let git = script(
            dir,
            "fakegit",
            &format!(
                "case \"$*\" in *--version*) echo 'git version 9.9'; exit 0;; esac\nsleep 300 &\necho $! > '{}'\nwait",
                pidfile.display()
            ),
        );
        (git, pidfile)
    }

    fn read_pid(pidfile: &Path) -> i32 {
        for _ in 0..100 {
            if let Ok(s) = fs::read_to_string(pidfile)
                && let Ok(p) = s.trim().parse()
            {
                return p;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("grandchild never started");
    }

    #[test]
    fn parses_symref_line() {
        assert_eq!(
            parse_default_branch("ref: refs/heads/trunk\tHEAD\nabc123\tHEAD\n"),
            Some("trunk".into())
        );
        assert_eq!(
            parse_default_branch("ref: refs/heads/feat/x\tHEAD\n"),
            Some("feat/x".into())
        );
        assert_eq!(parse_default_branch("abc123\tHEAD\n"), None);
        assert_eq!(
            parse_default_branch("ref: refs/heads/a\trefs/heads/a\n"),
            None
        );
        assert_eq!(parse_default_branch(""), None);
    }

    #[test]
    fn reachable_bare_repo_reports_default_branch() {
        if !have_git() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let r = bare(tmp.path(), "main.git", "main");
        commit_on(&r, "main");
        let rep = run(&file_url(&r), &opts(), &token());
        assert_eq!(rep.outcome, Outcome::Reachable, "{rep:?}");
        assert_eq!(rep.default_branch.as_deref(), Some("main"));
        assert!(rep.can_start());
        assert_eq!(rep.checks[0].id, CheckId::Git);
        assert_eq!(rep.checks[0].status, CheckStatus::Pass);
    }

    #[test]
    fn head_named_trunk_is_detected() {
        if !have_git() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let r = bare(tmp.path(), "trunk.git", "trunk");
        commit_on(&r, "trunk");
        let rep = run(&file_url(&r), &opts(), &token());
        assert_eq!(rep.default_branch.as_deref(), Some("trunk"), "{rep:?}");
    }

    #[test]
    fn empty_bare_repo_is_a_distinct_blocking_result() {
        if !have_git() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let r = bare(tmp.path(), "empty.git", "main");
        let rep = run(&file_url(&r), &opts(), &token());
        assert_eq!(rep.outcome, Outcome::EmptyRepo, "{rep:?}");
        assert_eq!(rep.default_branch, None);
        assert!(rep.has_fail());
        assert!(!rep.can_start());
    }

    #[test]
    fn missing_remote_fails_through_classify() {
        if !have_git() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let rep = run(&file_url(&tmp.path().join("nope.git")), &opts(), &token());
        assert_eq!(rep.outcome, Outcome::Failed);
        let remote = rep.checks.iter().find(|c| c.id == CheckId::Remote).unwrap();
        assert_eq!(remote.status, CheckStatus::Fail);
        assert!(
            rep.checks
                .iter()
                .all(|c| c.id != CheckId::SshAgent && c.id != CheckId::GhAuth)
        );
    }

    #[test]
    fn missing_git_binary_fails_the_git_check() {
        let o = PreflightOptions {
            env: GitEnv::new().with_git_binary("/nonexistent/git"),
            ..Default::default()
        };
        let rep = run(&file_url(Path::new("/x")), &o, &token());
        assert_eq!(rep.outcome, Outcome::Failed);
        assert_eq!(rep.checks.len(), 1);
        assert_eq!(rep.checks[0].id, CheckId::Git);
        assert_eq!(rep.checks[0].status, CheckStatus::Fail);
    }

    #[test]
    fn timeout_kills_the_whole_group() {
        let tmp = tempfile::tempdir().unwrap();
        let (git, pidfile) = sleepy_git(tmp.path());
        let o = PreflightOptions {
            env: GitEnv::new().with_git_binary(git),
            ls_remote_timeout: Duration::from_millis(400),
            ..Default::default()
        };
        let started = Instant::now();
        let rep = run(&file_url(Path::new("/x")), &o, &token());
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(rep.outcome, Outcome::Failed);
        let remote = rep.checks.iter().find(|c| c.id == CheckId::Remote).unwrap();
        assert_eq!(remote.status, CheckStatus::Fail);
        assert!(remote.message.contains("Timed out"), "{}", remote.message);
        assert_dies(read_pid(&pidfile));
    }

    #[test]
    fn cancel_kills_the_whole_group_promptly() {
        let tmp = tempfile::tempdir().unwrap();
        let (git, pidfile) = sleepy_git(tmp.path());
        let o = PreflightOptions {
            env: GitEnv::new().with_git_binary(git),
            ls_remote_timeout: Duration::from_secs(60),
            ..Default::default()
        };
        let cancel = token();
        let c2 = cancel.clone();
        let pf = pidfile.clone();
        std::thread::spawn(move || {
            read_pid(&pf);
            c2.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let rep = run(&file_url(Path::new("/x")), &o, &cancel);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(rep.outcome, Outcome::Cancelled);
        assert_dies(read_pid(&pidfile));
    }

    fn auth_failing_git(dir: &Path) -> PathBuf {
        script(
            dir,
            "authgit",
            "case \"$*\" in *--version*) echo 'git version 9.9'; exit 0;; esac\necho 'git@host: Permission denied (publickey).' >&2\nexit 128",
        )
    }

    #[test]
    fn ssh_auth_failure_runs_ssh_add_probe_for_context() {
        let tmp = tempfile::tempdir().unwrap();
        let o = PreflightOptions {
            env: GitEnv::new().with_git_binary(auth_failing_git(tmp.path())),
            ssh_add_program: script(tmp.path(), "ssh-add", "exit 2")
                .display()
                .to_string(),
            ..Default::default()
        };
        let u = url_for("git@host:o/r.git", UrlKind::ScpSsh, "host");
        let rep = run(&u, &o, &token());
        assert_eq!(rep.outcome, Outcome::Failed);
        let remote = &rep.checks[1];
        assert_eq!(remote.id, CheckId::Remote);
        assert_eq!(remote.status, CheckStatus::Fail);
        assert!(
            remote.message.contains("No SSH agent"),
            "{}",
            remote.message
        );
        let probe = rep
            .checks
            .iter()
            .find(|c| c.id == CheckId::SshAgent)
            .unwrap();
        assert_eq!(probe.status, CheckStatus::Fail);
    }

    #[test]
    fn gh_probe_runs_only_for_github_https_and_missing_gh_is_a_warn() {
        let tmp = tempfile::tempdir().unwrap();
        let mk = |gh: &str| PreflightOptions {
            env: GitEnv::new().with_git_binary(auth_failing_git(tmp.path())),
            gh_program: gh.into(),
            ..Default::default()
        };
        let gh_url = url_for("https://github.com/o/r.git", UrlKind::Https, "github.com");
        let rep = run(&gh_url, &mk("/nonexistent/gh"), &token());
        let probe = rep.checks.iter().find(|c| c.id == CheckId::GhAuth).unwrap();
        assert_eq!(probe.status, CheckStatus::Warn);
        assert!(
            rep.checks[1].fix.is_none(),
            "no login fix when gh is missing"
        );

        let gh_fail = script(tmp.path(), "gh", "exit 1").display().to_string();
        let rep = run(&gh_url, &mk(&gh_fail), &token());
        let probe = rep.checks.iter().find(|c| c.id == CheckId::GhAuth).unwrap();
        assert_eq!(probe.status, CheckStatus::Fail);
        assert_eq!(probe.fix, Some(FixId::GhAuthLogin));

        let other = url_for("https://gitlab.com/o/r.git", UrlKind::Https, "gitlab.com");
        let rep = run(&other, &mk(&gh_fail), &token());
        assert!(rep.checks.iter().all(|c| c.id != CheckId::GhAuth));
    }

    #[test]
    fn non_auth_failure_runs_no_probes() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("probed");
        let git = script(
            tmp.path(),
            "netgit",
            "case \"$*\" in *--version*) echo 'git version 9.9'; exit 0;; esac\necho 'fatal: Could not resolve host: x' >&2\nexit 128",
        );
        let probe = script(
            tmp.path(),
            "ssh-add",
            &format!("touch '{}'", marker.display()),
        );
        let o = PreflightOptions {
            env: GitEnv::new().with_git_binary(git),
            ssh_add_program: probe.display().to_string(),
            ..Default::default()
        };
        let u = url_for("git@host:o/r.git", UrlKind::ScpSsh, "host");
        let rep = run(&u, &o, &token());
        assert_eq!(rep.checks.len(), 2);
        assert!(!marker.exists());
    }

    #[test]
    fn large_output_does_not_deadlock() {
        let tmp = tempfile::tempdir().unwrap();
        let git = script(
            tmp.path(),
            "noisy",
            "case \"$*\" in *--version*) echo 'git version 9.9'; exit 0;; esac\nhead -c 300000 /dev/zero | tr '\\0' 'e' >&2\nhead -c 300000 /dev/zero | tr '\\0' 'o'\nexit 128",
        );
        let o = PreflightOptions {
            env: GitEnv::new().with_git_binary(git),
            ..Default::default()
        };
        let started = Instant::now();
        let rep = run(&file_url(Path::new("/x")), &o, &token());
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(rep.outcome, Outcome::Failed);
    }
}
