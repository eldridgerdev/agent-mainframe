//! Persistent tmux control-mode observer (architecture item 3 of the
//! performance plan).
//!
//! One long-lived `tmux -C` client stays attached to a hidden
//! `_amf-observer` session (the name deliberately does not match the
//! `amf-` prefix AMF uses for feature sessions). tmux pushes
//! `%sessions-changed` notifications to every control client whenever
//! any session is created, destroyed, or renamed, so feature status
//! reconciliation becomes event-driven: the main loop runs
//! `sync_statuses()` when the observer reports a change (or on a long
//! fallback interval) instead of spawning `tmux list-sessions` every
//! five seconds.
//!
//! The connection self-heals: if the client or the tmux server dies,
//! the worker marks statuses dirty (sessions may have vanished) and
//! reconnects with backoff. The per-view snapshot worker remains the
//! renderer for the focused pane — this client only observes lifecycle.

use std::io::BufRead;
use std::os::fd::{AsRawFd, OwnedFd};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::debug::{LogLevel, log_to_file};

const OBSERVER_SESSION: &str = "_amf-observer";

#[derive(Default)]
pub struct TmuxObserverFlags {
    sessions_dirty: AtomicBool,
}

impl TmuxObserverFlags {
    pub fn take_sessions_dirty(&self) -> bool {
        self.sessions_dirty.swap(false, Ordering::Relaxed)
    }

    fn mark_dirty_and_wake(&self, wakeup_fd: &OwnedFd) {
        self.sessions_dirty.store(true, Ordering::Relaxed);
        let byte = 1u8;
        unsafe {
            libc::write(wakeup_fd.as_raw_fd(), &byte as *const u8 as *const _, 1);
        }
    }
}

pub struct TmuxObserver {
    stop: Arc<AtomicBool>,
    child: Arc<Mutex<Option<Child>>>,
    pub flags: Arc<TmuxObserverFlags>,
}

impl TmuxObserver {
    pub fn start(wakeup_fd: Arc<OwnedFd>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let child = Arc::new(Mutex::new(None));
        let flags = Arc::new(TmuxObserverFlags::default());

        let worker_stop = stop.clone();
        let worker_child = child.clone();
        let worker_flags = flags.clone();
        let _ = std::thread::Builder::new()
            .name("tmux-observer".to_string())
            .spawn(move || {
                observer_loop(&worker_stop, &worker_child, &worker_flags, &wakeup_fd);
            });

        Self { stop, child, flags }
    }
}

impl Drop for TmuxObserver {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(mut slot) = self.child.lock()
            && let Some(child) = slot.as_mut()
        {
            let _ = child.kill();
        }
        // Best-effort, non-blocking removal of the hidden session so an
        // AMF-started tmux server does not outlive AMF. A concurrently
        // running AMF instance simply recreates it on its next retry.
        let _ = Command::new("tmux")
            .args(["kill-session", "-t", OBSERVER_SESSION])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

fn observer_loop(
    stop: &AtomicBool,
    child_slot: &Mutex<Option<Child>>,
    flags: &TmuxObserverFlags,
    wakeup_fd: &OwnedFd,
) {
    let mut backoff = Duration::from_secs(1);

    while !stop.load(Ordering::Relaxed) {
        match run_observer_client(stop, child_slot, flags, wakeup_fd) {
            Ok(()) => {
                backoff = Duration::from_secs(1);
            }
            Err(e) => {
                log_to_file(
                    LogLevel::Debug,
                    "tmux",
                    &format!("observer client ended: {e}"),
                );
            }
        }

        if stop.load(Ordering::Relaxed) {
            break;
        }

        // The connection dropped (tmux server restart, client killed):
        // session state is unknown, so force a reconciliation.
        flags.mark_dirty_and_wake(wakeup_fd);
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

fn run_observer_client(
    stop: &AtomicBool,
    child_slot: &Mutex<Option<Child>>,
    flags: &TmuxObserverFlags,
    wakeup_fd: &OwnedFd,
) -> anyhow::Result<()> {
    // -A attaches if the hidden session already exists (e.g. a second
    // AMF instance). The inert command keeps the window alive without
    // a shell.
    let mut command = Command::new("tmux");
    command
        .args([
            "-C",
            "new-session",
            "-A",
            "-s",
            OBSERVER_SESSION,
            "sleep",
            "2147483647",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command.env_remove("TMUX").env_remove("TMUX_PANE");

    let mut child = command.spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("observer client has no stdout"))?;
    *child_slot.lock().unwrap() = Some(child);

    log_to_file(LogLevel::Info, "tmux", "control-mode observer attached");
    // State at (re)connect is unknown — reconcile once.
    flags.mark_dirty_and_wake(wakeup_fd);

    let reader = std::io::BufReader::new(stdout);
    for line in reader.lines() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let line = line?;
        if line.starts_with("%sessions-changed") || line.starts_with("%session-renamed") {
            flags.mark_dirty_and_wake(wakeup_fd);
        } else if line.starts_with("%exit") {
            break;
        }
    }

    if let Some(mut child) = child_slot.lock().unwrap().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(())
}
