//! Runs `git clone` into a directory this process created and therefore owns.
//!
//! Ownership rules (the safety core of the feature):
//! - The destination leaf is created with [`std::fs::create_dir`], which is
//!   atomic. Success is the *only* proof that we own the path: `AlreadyExists`
//!   means refuse and touch nothing. The parent must already exist and is never
//!   created.
//! - We only ever delete that one directory, never a parent or the projects
//!   root. Before deleting we re-check with `symlink_metadata` that the path is
//!   still a real directory (not a symlink) with the device/inode we created,
//!   so a path swapped under us is left alone ([`RemoveOutcome::Refused`]).
//! - The `git` child leads its own session/process group (see [`GitEnv`]), so
//!   cancel and drop kill `git` together with `ssh` / `git-remote-https`
//!   (`kill(-pgid, ..)`: SIGTERM, then SIGKILL after a grace period).
//!
//! On success the directory stays owned by the caller via [`CloneHandle`]:
//! `finalize` either calls [`CloneHandle::rollback`] (registration failed) or
//! [`CloneHandle::release`] (registered). Dropping a handle that still owns the
//! directory before the point of no return kills any child and removes it.

use std::fs;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use super::classify::{FailureClass, classify_stderr};
use super::env::{GitEnv, Output};

/// How long SIGTERM gets before the group is SIGKILLed.
const DEFAULT_GRACE: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(20);
/// After git exits, how long to wait for the stderr pipe to close before
/// concluding that a straggler still holds it.
const READER_EOF_WAIT: Duration = Duration::from_millis(500);
/// Cap on retained non-progress stderr (kept from the end).
const TAIL_CAP: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressPhase {
    Counting,
    Compressing,
    Receiving,
    Resolving,
    UpdatingFiles,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgressUpdate {
    pub phase: ProgressPhase,
    pub percent: Option<u8>,
    /// The cleaned segment as git printed it.
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneOutcome {
    Success,
    Failed { class: FailureClass, stderr: String },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloneEvent {
    Progress(ProgressUpdate),
    /// Always the last event. The owned directory has already been removed for
    /// `Failed` and `Cancelled`.
    Finished(CloneOutcome),
}

#[derive(Debug)]
pub enum StartError {
    /// The destination already exists (any type). Nothing was touched.
    DestExists,
    /// The parent directory does not exist. Nothing was created.
    ParentMissing,
    /// `create_dir` failed for another reason (permissions, ...).
    Create(io::Error),
    /// git could not be spawned; the directory we created was removed.
    Spawn(io::Error),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DestExists => write!(f, "the destination already exists"),
            Self::ParentMissing => write!(f, "the destination's parent directory does not exist"),
            Self::Create(e) => write!(f, "could not create the destination: {e}"),
            Self::Spawn(e) => write!(f, "could not start git: {e}"),
        }
    }
}

impl std::error::Error for StartError {}

#[derive(Debug)]
pub enum RemoveOutcome {
    Removed,
    /// Nothing at the path any more.
    Gone,
    /// Ownership was already released or the directory already removed.
    NotOwned,
    /// The path is no longer the directory we created (symlink, file, or a
    /// different inode). Left untouched.
    Refused(&'static str),
    Failed(io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OwnState {
    Owned,
    Released,
    Removed,
}

/// A directory this process created with `create_dir`.
#[derive(Debug)]
struct OwnedDir {
    path: PathBuf,
    dev: u64,
    ino: u64,
    state: Mutex<OwnState>,
}

impl OwnedDir {
    fn create(path: &Path) -> Result<Self, StartError> {
        match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => {
                if !p.is_dir() {
                    return Err(StartError::ParentMissing);
                }
            }
            Some(_) => {}
            None => return Err(StartError::ParentMissing),
        }
        match fs::create_dir(path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                return Err(StartError::DestExists);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(StartError::ParentMissing);
            }
            Err(e) => return Err(StartError::Create(e)),
        }
        // Record identity of what we just created. If this fails we still
        // created the dir, so try to take it back before reporting.
        match fs::symlink_metadata(path) {
            Ok(md) => Ok(Self {
                path: path.to_path_buf(),
                dev: md.dev(),
                ino: md.ino(),
                state: Mutex::new(OwnState::Owned),
            }),
            Err(e) => {
                let _ = fs::remove_dir(path);
                Err(StartError::Create(e))
            }
        }
    }

    fn is_owned(&self) -> bool {
        *self.state.lock().unwrap() == OwnState::Owned
    }

    fn release(&self) {
        let mut s = self.state.lock().unwrap();
        if *s == OwnState::Owned {
            *s = OwnState::Released;
        }
    }

    /// Remove the directory tree iff we still own it and it is still the real
    /// directory we created.
    fn remove(&self) -> RemoveOutcome {
        let mut s = self.state.lock().unwrap();
        if *s != OwnState::Owned {
            return RemoveOutcome::NotOwned;
        }
        let md = match fs::symlink_metadata(&self.path) {
            Ok(md) => md,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                *s = OwnState::Removed;
                return RemoveOutcome::Gone;
            }
            Err(e) => return RemoveOutcome::Failed(e),
        };
        // `file_type()` from symlink_metadata never follows links, so a
        // symlink reports `is_symlink`, not `is_dir`.
        if !md.file_type().is_dir() {
            *s = OwnState::Removed; // no longer ours; do not retry
            return RemoveOutcome::Refused("path is no longer a real directory");
        }
        if md.dev() != self.dev || md.ino() != self.ino {
            *s = OwnState::Removed;
            return RemoveOutcome::Refused("path is no longer the directory we created");
        }
        // std's remove_dir_all does not follow symlinks (top level included),
        // which narrows the residual check/remove window further.
        match fs::remove_dir_all(&self.path) {
            Ok(()) => {
                *s = OwnState::Removed;
                RemoveOutcome::Removed
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                *s = OwnState::Removed;
                RemoveOutcome::Gone
            }
            Err(e) => RemoveOutcome::Failed(e),
        }
    }
}

#[derive(Debug, Default)]
struct ControlState {
    /// Stop requested (user cancel, rollback, or drop).
    stop: bool,
    /// Registration has begun; a user cancel is refused.
    point_of_no_return: bool,
    /// Clone finished successfully and was not stopped.
    succeeded: bool,
    /// Worker is over for any reason.
    finished: bool,
}

#[derive(Debug)]
struct Control {
    state: Mutex<ControlState>,
    grace: Duration,
}

/// Handle to a running (or finished) clone. See the module docs.
pub struct CloneHandle {
    rx: Receiver<CloneEvent>,
    ctl: Arc<Control>,
    dir: Arc<OwnedDir>,
    worker: Option<JoinHandle<CloneOutcome>>,
    outcome: Option<CloneOutcome>,
    pgid: i32,
}

impl std::fmt::Debug for CloneHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloneHandle")
            .field("path", &self.dir.path)
            .field("pgid", &self.pgid)
            .finish()
    }
}

/// Start a clone of `url` into `dest`. `dest` must not exist and its parent
/// must. Spawns through [`GitEnv`] only.
pub fn start(env: &GitEnv, url: &str, dest: &Path) -> Result<CloneHandle, StartError> {
    start_with_grace(env, url, dest, DEFAULT_GRACE)
}

fn start_with_grace(
    env: &GitEnv,
    url: &str,
    dest: &Path,
    grace: Duration,
) -> Result<CloneHandle, StartError> {
    let dir = Arc::new(OwnedDir::create(dest)?);

    let mut cmd = env.command(GitEnv::clone_args(url, dest), Output::Null, Output::Piped);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            dir.remove();
            return Err(StartError::Spawn(e));
        }
    };
    let pgid = child.id() as i32;
    let Some(stderr) = child.stderr.take() else {
        kill_and_reap(&mut child, pgid, Duration::ZERO);
        dir.remove();
        return Err(StartError::Spawn(io::Error::other("no stderr pipe")));
    };

    let (tx, rx) = mpsc::channel();
    let ctl = Arc::new(Control {
        state: Mutex::new(ControlState::default()),
        grace,
    });

    let worker = {
        let dir = Arc::clone(&dir);
        let ctl = Arc::clone(&ctl);
        thread::Builder::new()
            .name("git-clone-worker".into())
            .spawn(move || worker_main(child, pgid, stderr, tx, ctl, dir))
    };
    let worker = match worker {
        Ok(w) => w,
        Err(e) => {
            // `child` moved into the failed closure and was dropped, which
            // does not kill it; the pgid is still ours to signal.
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
            dir.remove();
            return Err(StartError::Spawn(e));
        }
    };

    Ok(CloneHandle {
        rx,
        ctl,
        dir,
        worker: Some(worker),
        outcome: None,
        pgid,
    })
}

impl CloneHandle {
    /// The owned destination directory.
    pub fn path(&self) -> &Path {
        &self.dir.path
    }

    /// Process-group id of the git child (equals its pid).
    pub fn pgid(&self) -> i32 {
        self.pgid
    }

    /// Request cancellation. Returns `false` (and does nothing) once the point
    /// of no return has been passed or the clone has already finished.
    pub fn cancel(&self) -> bool {
        let mut s = self.ctl.state.lock().unwrap();
        if s.point_of_no_return || s.finished {
            return false;
        }
        s.stop = true;
        true
    }

    /// Called by `finalize` before registration. After this returns `true`,
    /// [`cancel`](Self::cancel) is refused and drop leaves the directory
    /// alone. Returns `false` if the clone has not succeeded (still running,
    /// failed, or cancelled): the caller must not register.
    pub fn begin_point_of_no_return(&self) -> bool {
        let mut s = self.ctl.state.lock().unwrap();
        if !s.succeeded || s.stop {
            return false;
        }
        s.point_of_no_return = true;
        true
    }

    pub fn try_recv(&self) -> Result<CloneEvent, TryRecvError> {
        self.rx.try_recv()
    }

    pub fn recv_timeout(&self, d: Duration) -> Result<CloneEvent, RecvTimeoutError> {
        self.rx.recv_timeout(d)
    }

    /// Blocking iterator over events; ends after `Finished`.
    pub fn iter(&self) -> impl Iterator<Item = CloneEvent> + '_ {
        self.rx.iter()
    }

    /// Wait for the worker to finish and return its outcome (idempotent).
    pub fn wait(&mut self) -> CloneOutcome {
        if self.outcome.is_none() {
            let outcome = match self.worker.take() {
                Some(w) => w.join().unwrap_or_else(|_| CloneOutcome::Failed {
                    class: FailureClass::Unknown("clone worker panicked".into()),
                    stderr: String::new(),
                }),
                None => CloneOutcome::Cancelled,
            };
            self.outcome = Some(outcome);
        }
        self.outcome.clone().unwrap()
    }

    /// Stop any running clone (even past the point of no return) and delete
    /// the owned directory with the symlink/identity re-check. For `finalize`
    /// when registration fails.
    pub fn rollback(&mut self) -> RemoveOutcome {
        {
            let mut s = self.ctl.state.lock().unwrap();
            if !s.finished {
                s.stop = true;
            }
        }
        self.wait();
        self.dir.remove()
    }

    /// Give up ownership: the directory now belongs to the registered project
    /// and neither drop nor rollback will touch it.
    pub fn release(&mut self) {
        self.dir.release();
    }
}

impl Drop for CloneHandle {
    fn drop(&mut self) {
        if !self.dir.is_owned() {
            return;
        }
        {
            let mut s = self.ctl.state.lock().unwrap();
            if s.point_of_no_return {
                // Registration owns the outcome; leave everything alone.
                return;
            }
            if !s.finished {
                s.stop = true;
            }
        }
        self.wait();
        self.dir.remove();
    }
}

// ---------------------------------------------------------------- worker

/// Removes the owned dir on drop unless disarmed (panic / early return).
struct Rollback {
    dir: Arc<OwnedDir>,
    armed: bool,
}

impl Drop for Rollback {
    fn drop(&mut self) {
        if self.armed {
            self.dir.remove();
        }
    }
}

/// Kills the process group and reaps the child on drop (panic safety).
struct ChildGuard {
    child: Option<Child>,
    pgid: i32,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            kill_and_reap(&mut c, self.pgid, Duration::ZERO);
        }
    }
}

/// True once the child has exited, *without* reaping it. Keeping the zombie
/// leader around means the pgid cannot be recycled while we still signal it.
fn has_exited(pid: i32) -> bool {
    // SAFETY: zeroed siginfo_t is a valid out-parameter; waitid with WNOWAIT
    // does not consume the child's status.
    unsafe {
        let mut info: libc::siginfo_t = std::mem::zeroed();
        let r = libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        );
        // With WNOHANG and nothing to report, si_signo stays 0.
        r == 0 && info.si_signo != 0
    }
}

/// SIGTERM the whole group, give the leader `grace` to exit, then SIGKILL the
/// whole group (catching grandchildren that ignored or outlived the leader)
/// and reap. The group is signalled *before* the leader is reaped.
fn kill_and_reap(child: &mut Child, pgid: i32, grace: Duration) {
    // SAFETY: plain signal delivery to a group we created via setsid.
    unsafe { libc::kill(-pgid, libc::SIGTERM) };
    let deadline = Instant::now() + grace;
    while !has_exited(pgid) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    unsafe { libc::kill(-pgid, libc::SIGKILL) };
    let _ = child.wait();
}

fn worker_main(
    child: Child,
    pgid: i32,
    stderr: std::process::ChildStderr,
    tx: Sender<CloneEvent>,
    ctl: Arc<Control>,
    dir: Arc<OwnedDir>,
) -> CloneOutcome {
    // Drop order (reverse of declaration): child group killed first, then dir.
    let mut rollback = Rollback {
        dir: Arc::clone(&dir),
        armed: true,
    };
    let mut guard = ChildGuard {
        child: Some(child),
        pgid,
    };

    let tail = Arc::new(Mutex::new(String::new()));
    let eof = Arc::new(AtomicBool::new(false));
    let reader = {
        let (tx, tail, eof) = (tx.clone(), Arc::clone(&tail), Arc::clone(&eof));
        thread::spawn(move || read_stderr(stderr, &tx, &tail, &eof))
    };

    // Wait for exit or a stop request.
    let stopped = loop {
        if ctl.state.lock().unwrap().stop {
            break true;
        }
        if has_exited(pgid) {
            break false;
        }
        thread::sleep(POLL);
    };

    let finish = |outcome: CloneOutcome, ctl: &Control| {
        {
            let mut s = ctl.state.lock().unwrap();
            s.finished = true;
            s.succeeded = outcome == CloneOutcome::Success;
        }
        let _ = tx.send(CloneEvent::Finished(outcome.clone()));
        outcome
    };

    if stopped {
        let mut c = guard.child.take().unwrap();
        kill_and_reap(&mut c, pgid, ctl.grace);
        let _ = reader.join(); // every pipe holder is dead; EOF is certain
        dir.remove();
        rollback.armed = false;
        return finish(CloneOutcome::Cancelled, &ctl);
    }

    // Natural exit: reap for the status.
    let status = guard.child.as_mut().unwrap().wait();
    guard.child = None;

    // If a straggler (e.g. ssh) still holds the stderr pipe, the group is
    // still populated, so killing it cannot hit a recycled pgid.
    let start = Instant::now();
    while !eof.load(Ordering::Acquire) && start.elapsed() < READER_EOF_WAIT {
        thread::sleep(Duration::from_millis(10));
    }
    if !eof.load(Ordering::Acquire) {
        unsafe { libc::kill(-pgid, libc::SIGKILL) };
    }
    let _ = reader.join();

    let ok = matches!(&status, Ok(s) if s.success());
    if ok {
        // Decide success vs a cancel that raced the exit, under one lock.
        let mut s = ctl.state.lock().unwrap();
        if !s.stop {
            s.finished = true;
            s.succeeded = true;
            drop(s);
            rollback.armed = false;
            let _ = tx.send(CloneEvent::Finished(CloneOutcome::Success));
            return CloneOutcome::Success;
        }
        drop(s);
        dir.remove();
        rollback.armed = false;
        return finish(CloneOutcome::Cancelled, &ctl);
    }

    let stderr_text = tail.lock().unwrap().trim().to_string();
    let stderr_text = if stderr_text.is_empty() {
        match &status {
            Ok(s) => format!("git exited with {s}"),
            Err(e) => format!("failed to wait for git: {e}"),
        }
    } else {
        stderr_text
    };
    dir.remove();
    rollback.armed = false;
    finish(
        CloneOutcome::Failed {
            class: classify_stderr(&stderr_text),
            stderr: stderr_text,
        },
        &ctl,
    )
}

fn read_stderr(
    mut stderr: std::process::ChildStderr,
    tx: &Sender<CloneEvent>,
    tail: &Mutex<String>,
    eof: &AtomicBool,
) {
    let mut seg: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match stderr.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                for &b in &buf[..n] {
                    if b == b'\r' || b == b'\n' {
                        flush_segment(&mut seg, tx, tail);
                    } else {
                        seg.push(b);
                    }
                }
            }
        }
    }
    flush_segment(&mut seg, tx, tail);
    eof.store(true, Ordering::Release);
}

fn flush_segment(seg: &mut Vec<u8>, tx: &Sender<CloneEvent>, tail: &Mutex<String>) {
    if seg.is_empty() {
        return;
    }
    let text = String::from_utf8_lossy(seg).trim().to_string();
    seg.clear();
    if text.is_empty() {
        return;
    }
    match parse_progress(&text) {
        Some(update) => {
            let _ = tx.send(CloneEvent::Progress(update));
        }
        None => {
            let mut t = tail.lock().unwrap();
            if !t.is_empty() {
                t.push('\n');
            }
            t.push_str(&text);
            if t.len() > TAIL_CAP {
                let mut cut = t.len() - TAIL_CAP;
                while !t.is_char_boundary(cut) {
                    cut += 1;
                }
                t.drain(..cut);
            }
        }
    }
}

/// Parse one `\r`/`\n`-delimited stderr segment. `None` for anything that is
/// not a progress line (it is kept for failure classification instead).
pub fn parse_progress(segment: &str) -> Option<ProgressUpdate> {
    let line = segment.trim();
    let body = line.strip_prefix("remote:").unwrap_or(line).trim();
    let (label, rest) = body.split_once(':')?;
    let phase = match label.trim() {
        "Counting objects" | "Enumerating objects" => ProgressPhase::Counting,
        "Compressing objects" => ProgressPhase::Compressing,
        "Receiving objects" => ProgressPhase::Receiving,
        "Resolving deltas" => ProgressPhase::Resolving,
        "Updating files" | "Checking out files" => ProgressPhase::UpdatingFiles,
        _ => return None,
    };
    let percent = rest
        .trim_start()
        .split_once('%')
        .and_then(|(n, _)| n.trim().parse::<u16>().ok().map(|p| p.min(100) as u8));
    Some(ProgressUpdate {
        phase,
        percent,
        detail: body.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::process::Command;

    const WAIT: Duration = Duration::from_secs(30);

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    fn git(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Bare repo whose default branch is `trunk`, with one commit.
    fn trunk_remote(root: &Path) -> String {
        let bare = root.join("remote.git");
        let work = root.join("work");
        fs::create_dir_all(&work).unwrap();
        git(root, &["init", "--bare", "-b", "trunk", "remote.git"]);
        git(&work, &["init", "-b", "trunk"]);
        fs::write(work.join("README"), "hi").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "init"]);
        git(&work, &["push", bare.to_str().unwrap(), "trunk"]);
        format!("file://{}", bare.display())
    }

    fn file_env() -> GitEnv {
        GitEnv::new().with_file_transport(true)
    }

    fn pid_gone(pid: i32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let r = unsafe { libc::kill(pid, 0) };
            if r != 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                return true;
            }
            if Instant::now() > deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn drain(h: &CloneHandle) -> Vec<CloneEvent> {
        let mut v = Vec::new();
        loop {
            match h.recv_timeout(WAIT) {
                Ok(e) => {
                    let done = matches!(e, CloneEvent::Finished(_));
                    v.push(e);
                    if done {
                        return v;
                    }
                }
                Err(_) => return v,
            }
        }
    }

    #[test]
    fn parses_progress_segments() {
        let p = parse_progress("Receiving objects:  45% (45/100), 1.2 MiB | 1 MiB/s").unwrap();
        assert_eq!(p.phase, ProgressPhase::Receiving);
        assert_eq!(p.percent, Some(45));
        let p = parse_progress("remote: Counting objects: 100% (5/5), done.").unwrap();
        assert_eq!(p.phase, ProgressPhase::Counting);
        assert_eq!(p.percent, Some(100));
        let p = parse_progress("remote: Enumerating objects: 5, done.").unwrap();
        assert_eq!((p.phase, p.percent), (ProgressPhase::Counting, None));
        assert!(parse_progress("Cloning into 'x'...").is_none());
        assert!(parse_progress("fatal: repository not found").is_none());
    }

    #[test]
    fn success_with_non_main_default_branch() {
        let tmp = tempfile::tempdir().unwrap();
        let url = trunk_remote(tmp.path());
        let projects = tmp.path().join("projects");
        fs::create_dir(&projects).unwrap();
        let dest = projects.join("repo");

        let mut h = start(&file_env(), &url, &dest).unwrap();
        let events = drain(&h);
        assert_eq!(
            events.last(),
            Some(&CloneEvent::Finished(CloneOutcome::Success))
        );
        assert_eq!(h.wait(), CloneOutcome::Success);
        assert!(dest.join("README").is_file());
        let out = Command::new("git")
            .arg("-C")
            .arg(&dest)
            .args(["symbolic-ref", "--short", "HEAD"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "trunk");
        h.release();
        drop(h);
        assert!(dest.is_dir(), "released dir must survive drop");
    }

    #[test]
    fn preexisting_empty_dir_is_refused_and_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("repo");
        fs::create_dir(&dest).unwrap();
        let err = start(&file_env(), "file:///nope", &dest).unwrap_err();
        assert!(matches!(err, StartError::DestExists));
        assert!(dest.is_dir());
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    }

    #[test]
    fn preexisting_symlink_is_refused_and_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let sentinel = tmp.path().join("sentinel");
        fs::create_dir(&sentinel).unwrap();
        fs::write(sentinel.join("keep"), "x").unwrap();
        let dest = tmp.path().join("repo");
        symlink(&sentinel, &dest).unwrap();
        let err = start(&file_env(), "file:///nope", &dest).unwrap_err();
        assert!(matches!(err, StartError::DestExists));
        assert!(
            fs::symlink_metadata(&dest)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(sentinel.join("keep").is_file());

        // Dangling symlink too.
        let dangling = tmp.path().join("dangling");
        symlink(tmp.path().join("missing"), &dangling).unwrap();
        assert!(matches!(
            start(&file_env(), "file:///nope", &dangling).unwrap_err(),
            StartError::DestExists
        ));
        assert!(fs::symlink_metadata(&dangling).is_ok());
    }

    #[test]
    fn missing_parent_is_refused_and_not_created() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("no/such/repo");
        let err = start(&file_env(), "file:///nope", &dest).unwrap_err();
        assert!(matches!(err, StartError::ParentMissing));
        assert!(!tmp.path().join("no").exists());
    }

    #[test]
    fn failed_remote_removes_owned_dir_only() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("repo");
        let bogus = format!("file://{}/does-not-exist.git", tmp.path().display());
        let mut h = start(&file_env(), &bogus, &dest).unwrap();
        let events = drain(&h);
        match events.last() {
            Some(CloneEvent::Finished(CloneOutcome::Failed { stderr, .. })) => {
                assert!(!stderr.is_empty());
            }
            other => panic!("expected failure, got {other:?}"),
        }
        assert!(matches!(h.wait(), CloneOutcome::Failed { .. }));
        assert!(!dest.exists());
        assert!(tmp.path().is_dir(), "parent must survive");
    }

    const FAKE_BODY: &str = r#"
sleep 300 &
echo $! > "$PIDFILE"
printf 'Receiving objects:  10%% (1/10)\r' >&2
printf 'Receiving objects:  50%% (5/10)\r' >&2
printf 'Receiving objects: 100%% (10/10), done.\n' >&2
wait
"#;

    fn fake(tmp: &Path, stubborn: bool) -> (GitEnv, PathBuf) {
        let pidfile = tmp.join("gc.pid");
        let trap = if stubborn { "trap '' TERM\n" } else { "" };
        let body = format!("PIDFILE='{}'\n{trap}{FAKE_BODY}", pidfile.display());
        let bin = script(tmp, "fakegit", &body);
        (GitEnv::new().with_git_binary(bin), pidfile)
    }

    fn wait_for_100(h: &CloneHandle) -> Vec<u8> {
        let mut pct = Vec::new();
        while let Ok(CloneEvent::Progress(p)) = h.recv_timeout(WAIT) {
            pct.push(p.percent.unwrap());
            if p.percent == Some(100) {
                break;
            }
        }
        pct
    }

    fn cancel_scenario(stubborn: bool) {
        let tmp = tempfile::tempdir().unwrap();
        let (env, pidfile) = fake(tmp.path(), stubborn);
        let dest = tmp.path().join("repo");
        let mut h =
            start_with_grace(&env, "https://h/o/r.git", &dest, Duration::from_millis(300)).unwrap();
        let pgid = h.pgid();

        let pct = wait_for_100(&h);
        assert_eq!(pct, vec![10, 50, 100], "progress arrives in order");
        let gc: i32 = fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(
            unsafe { libc::kill(gc, 0) },
            0,
            "grandchild alive pre-cancel"
        );

        assert!(h.cancel());
        assert_eq!(h.wait(), CloneOutcome::Cancelled);
        let rest = drain(&h);
        assert_eq!(rest, vec![CloneEvent::Finished(CloneOutcome::Cancelled)]);
        assert!(!dest.exists(), "owned dir removed");
        assert!(pid_gone(gc), "grandchild survived cancel");
        assert!(pid_gone(-pgid), "process group survived cancel");
        assert!(tmp.path().is_dir());
    }

    #[test]
    fn cancel_kills_whole_group_and_cleans_up() {
        cancel_scenario(false);
    }

    #[test]
    fn cancel_escalates_to_sigkill_when_term_is_ignored() {
        cancel_scenario(true);
    }

    #[test]
    fn drop_mid_clone_kills_group_and_cleans_up() {
        let tmp = tempfile::tempdir().unwrap();
        let (env, pidfile) = fake(tmp.path(), false);
        let dest = tmp.path().join("repo");
        let h = start(&env, "https://h/o/r.git", &dest).unwrap();
        let pgid = h.pgid();
        assert_eq!(wait_for_100(&h), vec![10, 50, 100]);
        let gc: i32 = fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        drop(h);
        assert!(!dest.exists());
        assert!(pid_gone(gc));
        assert!(pid_gone(-pgid));
    }

    #[test]
    fn cancel_refused_after_point_of_no_return() {
        let tmp = tempfile::tempdir().unwrap();
        let url = trunk_remote(tmp.path());
        let dest = tmp.path().join("repo");
        let mut h = start(&file_env(), &url, &dest).unwrap();
        assert_eq!(h.wait(), CloneOutcome::Success);
        assert!(h.begin_point_of_no_return());
        assert!(!h.cancel(), "cancel must be refused");
        drop(h); // past the point of no return: drop leaves the dir alone
        assert!(dest.join("README").is_file());
    }

    #[test]
    fn point_of_no_return_refused_before_success() {
        let tmp = tempfile::tempdir().unwrap();
        let (env, _pid) = fake(tmp.path(), false);
        let dest = tmp.path().join("repo");
        let h = start(&env, "https://h/o/r.git", &dest).unwrap();
        assert!(!h.begin_point_of_no_return());
        assert!(h.cancel());
    }

    #[test]
    fn rollback_after_success_removes_dir_even_past_point_of_no_return() {
        let tmp = tempfile::tempdir().unwrap();
        let url = trunk_remote(tmp.path());
        let dest = tmp.path().join("repo");
        let mut h = start(&file_env(), &url, &dest).unwrap();
        assert_eq!(h.wait(), CloneOutcome::Success);
        assert!(h.begin_point_of_no_return());
        assert!(matches!(h.rollback(), RemoveOutcome::Removed));
        assert!(!dest.exists());
    }

    #[test]
    fn drop_after_success_without_release_rolls_back() {
        let tmp = tempfile::tempdir().unwrap();
        let url = trunk_remote(tmp.path());
        let dest = tmp.path().join("repo");
        let mut h = start(&file_env(), &url, &dest).unwrap();
        assert_eq!(h.wait(), CloneOutcome::Success);
        drop(h);
        assert!(!dest.exists());
    }

    #[test]
    fn cleanup_never_removes_a_swapped_in_symlink_target() {
        let tmp = tempfile::tempdir().unwrap();
        let sentinel = tmp.path().join("sentinel");
        fs::create_dir(&sentinel).unwrap();
        fs::write(sentinel.join("keep"), "x").unwrap();
        let dest = tmp.path().join("repo");

        let dir = OwnedDir::create(&dest).unwrap();
        fs::remove_dir(&dest).unwrap();
        symlink(&sentinel, &dest).unwrap();

        assert!(matches!(dir.remove(), RemoveOutcome::Refused(_)));
        assert!(sentinel.join("keep").is_file(), "sentinel contents deleted");
        assert!(
            fs::symlink_metadata(&dest)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }

    #[test]
    fn cleanup_refuses_a_swapped_in_foreign_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("repo");
        let dir = OwnedDir::create(&dest).unwrap();
        // Keep our inode alive elsewhere so the new dir cannot reuse it.
        let moved = tmp.path().join("moved");
        fs::rename(&dest, &moved).unwrap();
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("theirs"), "x").unwrap();

        assert!(matches!(dir.remove(), RemoveOutcome::Refused(_)));
        assert!(dest.join("theirs").is_file());
    }

    #[test]
    fn released_dir_is_never_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("repo");
        let dir = OwnedDir::create(&dest).unwrap();
        dir.release();
        assert!(matches!(dir.remove(), RemoveOutcome::NotOwned));
        assert!(dest.is_dir());
    }
}
