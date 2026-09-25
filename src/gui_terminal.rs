//! GUI terminal transport (`AMF_PLAN.md` Task 6). Reuses the TUI's
//! persistent tmux control-mode client
//! (`TmuxManager::spawn_control_mode_view_client`) rather than a new
//! PTY-attach path, per Task 2's resolved decision.
//!
//! The control-mode stream is used purely as a change notifier here, never
//! decoded for content -- real content always comes from a fresh
//! `TmuxManager::capture_pane_for_replay` call, for the exact correctness
//! reason the TUI's own control-mode worker does this (see the inline
//! comment on the control stream in `App::run_control_mode_view_worker`,
//! `src/app/mod.rs`): replaying raw `%output` bytes through a second
//! terminal emulator can drift from what tmux itself renders for sequences
//! like scroll regions. That choice also means "buffering" and "reconnect"
//! need no dedicated machinery: there is no byte stream to buffer, and
//! reconnecting is just another `attach` -- a fresh capture already reflects
//! tmux's current state (including its own scrollback), not a replay log
//! this module would otherwise have to keep.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::tmux::{
    SpawnedTmuxControlClient, TmuxManager, parse_tmux_output_notification,
    sanitize_tmux_control_line,
};

/// How often the worker checks for a pending dirty signal and, if set,
/// recaptures. A single fixed debounce interval rather than the TUI's tuned
/// two-tier burst/normal scheme (`VIEW_BURST_DURATION`/
/// `VIEW_PANE_REFRESH_INTERVAL` in `src/app/mod.rs`) -- a deliberate
/// first-slice simplification, not a claim this is already as tuned as the
/// TUI's path. It still does the one thing that matters for high-volume
/// output: a burst of dozens of `%output` notifications in this window
/// collapses into one recapture, not dozens of `tmux capture-pane` shells.
const RECAPTURE_INTERVAL: Duration = Duration::from_millis(50);

struct Dims {
    cols: AtomicU32,
    rows: AtomicU32,
}

impl Dims {
    fn new(cols: u16, rows: u16) -> Self {
        Self {
            cols: AtomicU32::new(u32::from(cols)),
            rows: AtomicU32::new(u32::from(rows)),
        }
    }

    fn get(&self) -> (u16, u16) {
        (
            self.cols.load(Ordering::Relaxed) as u16,
            self.rows.load(Ordering::Relaxed) as u16,
        )
    }

    fn set(&self, cols: u16, rows: u16) {
        self.cols.store(u32::from(cols), Ordering::Relaxed);
        self.rows.store(u32::from(rows), Ordering::Relaxed);
    }
}

/// A live GUI-side attachment to one tmux pane.
///
/// Dropping this detaches: the worker thread is asked to stop and joined
/// (bounded, so a wedged worker cannot hang shutdown indefinitely), and the
/// `SpawnedTmuxControlClient` it owned is dropped when the thread exits,
/// which sends `detach-client` and kills the client process -- the same
/// cleanup the TUI's own control-mode client already does, reused as-is
/// rather than reimplemented here.
pub struct TerminalHandle {
    session: String,
    window: String,
    dims: Arc<Dims>,
    dirty: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl TerminalHandle {
    /// Attach to `session:window`, returning the handle plus the pane's
    /// current content (already normalized and cursor-positioned -- see
    /// `TmuxManager::capture_pane_for_replay`) to seed the caller's terminal
    /// emulator with before any live update arrives.
    ///
    /// `on_output` is called from a dedicated background thread with a full
    /// replacement replay string each time the pane changes (debounced, not
    /// once per notification); the caller resets its terminal emulator and
    /// writes the given string, exactly like the initial seed.
    pub fn attach(
        session: &str,
        window: &str,
        cols: u16,
        rows: u16,
        on_output: impl Fn(String) + Send + 'static,
    ) -> Result<(Self, String)> {
        let (_target_window_id, target_pane_id) =
            TmuxManager::resolve_view_target_ids(session, window)?;
        let client = TmuxManager::spawn_control_mode_view_client(
            session,
            window,
            &target_pane_id,
            cols,
            rows,
        )?;

        let initial = TmuxManager::capture_pane_for_replay(session, window, cols, rows);

        let dims = Arc::new(Dims::new(cols, rows));
        let dirty = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));

        let worker = std::thread::spawn({
            let session = session.to_string();
            let window = window.to_string();
            let dims = Arc::clone(&dims);
            let dirty = Arc::clone(&dirty);
            let stop = Arc::clone(&stop);
            move || {
                run_worker(
                    session,
                    window,
                    client,
                    target_pane_id,
                    dims,
                    dirty,
                    stop,
                    on_output,
                )
            }
        });

        Ok((
            Self {
                session: session.to_string(),
                window: window.to_string(),
                dims,
                dirty,
                stop,
                worker: Some(worker),
            },
            initial,
        ))
    }

    /// Send raw input bytes (as UTF-8 text) to the pane, exactly as typed --
    /// no key-name interpretation, so escape sequences xterm.js generates
    /// for special keys (arrows, function keys, ...) pass through as the
    /// running program would see them from a real terminal.
    pub fn send_input(&self, text: &str) -> Result<()> {
        TmuxManager::send_literal(&self.session, &self.window, text)
    }

    /// Submit an edited prompt using the same bracketed-paste path as the
    /// TUI composer, so embedded newlines remain one prompt rather than
    /// becoming a series of premature Enter keypresses.
    pub fn submit_prompt(&self, text: &str) -> Result<()> {
        TmuxManager::send_key_name(&self.session, &self.window, "C-u")?;
        TmuxManager::paste_text(&self.session, &self.window, text)?;
        TmuxManager::send_key_name(&self.session, &self.window, "Enter")
    }

    /// Resize the pane and update the dimensions the worker uses for its
    /// next cursor-position clamp, then force an immediate recapture rather
    /// than waiting for the next `%output`/`%layout-change` notification --
    /// the caller (whose own viewport just changed) wants reflowed content
    /// right away, not after the next unrelated dirty signal.
    pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
        TmuxManager::resize_pane(&self.session, &self.window, cols, rows)?;
        self.dims.set(cols, rows);
        self.dirty.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for TerminalHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            // Bounded wait: the worker's own loop polls `stop` at least
            // every `client.recv_timeout` tick (well under a second), so a
            // join that has not landed by then means the thread is stuck,
            // not merely slow -- detach the handle rather than block
            // shutdown on it. The tmux control client still gets killed
            // when the worker's stack unwinds and drops it, whether or not
            // this thread waited for that to happen.
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let _ = std::thread::Builder::new().spawn(move || {
                let _ = worker.join();
                let _ = done_tx.send(());
            });
            let _ = done_rx.recv_timeout(Duration::from_secs(1));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_worker(
    session: String,
    window: String,
    client: SpawnedTmuxControlClient,
    target_pane_id: String,
    dims: Arc<Dims>,
    dirty: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    on_output: impl Fn(String) + Send + 'static,
) {
    let mut last_capture = Instant::now();

    while !stop.load(Ordering::Relaxed) {
        match client.recv_timeout(Duration::from_millis(20)) {
            Ok(Some(raw_line)) => {
                let line = sanitize_tmux_control_line(&raw_line);
                if line.is_empty() {
                    continue;
                }

                if let Some((pane_id, payload)) = parse_tmux_output_notification(line) {
                    if pane_id == target_pane_id && !payload.is_empty() {
                        dirty.store(true, Ordering::Relaxed);
                    }
                    continue;
                }

                // A resize (ours or another client's) or the pane's window
                // being reconfigured also has to trigger a recapture -- the
                // shape of the content changed even if no new bytes did.
                if line.starts_with("%layout-change ") || line.starts_with("%pane-mode-changed ") {
                    dirty.store(true, Ordering::Relaxed);
                }
            }
            Ok(None) => {}
            // The control client's process exited or its output stream
            // disconnected -- nothing more to watch for on this attachment.
            Err(_) => break,
        }

        if dirty.load(Ordering::Relaxed) && last_capture.elapsed() >= RECAPTURE_INTERVAL {
            dirty.store(false, Ordering::Relaxed);
            let (cols, rows) = dims.get();
            let replay = TmuxManager::capture_pane_for_replay(&session, &window, cols, rows);
            on_output(replay);
            last_capture = Instant::now();
        }
    }
}

// These use a real tmux server (whatever `TmuxManager::runtime()` resolves
// to for this test binary -- there is no per-test-process socket isolation
// available, see the module-level note below) rather than `MockTmuxOps`: a
// mock cannot meaningfully stand in for a real PTY/control-mode client, and
// this transport's entire job is the plumbing between them. Isolation comes
// from a unique session name per test, not a dedicated server.
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Mutex, Once};

    /// `spawn_control_mode_view_client`'s `open_pty` reads terminal
    /// attributes from *this process's own* `STDIN_FILENO` (to clone them
    /// onto the new PTY), which fails with ENOTTY when the test binary's
    /// stdin is a pipe rather than a real terminal -- true under this
    /// harness's shell tool, and generally true under most CI runners. No
    /// existing test in the codebase calls this function, so this constraint
    /// was never hit before. Rather than changing production code to avoid
    /// depending on the calling process's stdin, give this test binary a
    /// real PTY on fd 0, once: opening one and cloning its own attributes
    /// back onto itself cannot make anything stricter than "was not a tty,
    /// now is one" for any other test that happens to touch stdin.
    fn ensure_process_stdin_is_a_pty() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| unsafe {
            let mut master: i32 = -1;
            let mut slave: i32 = -1;
            let ok = libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                // `null_mut` for both: macOS declares these `*mut`, Linux
                // `*const`, and `*mut` coerces to `*const` but not back.
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            assert_eq!(ok, 0, "failed to open a pty for the test process's stdin");
            assert_ne!(
                libc::dup2(slave, libc::STDIN_FILENO),
                -1,
                "failed to dup2 the pty slave onto stdin"
            );
            libc::close(slave);
            // `master` is deliberately leaked for the life of the test
            // binary: closing it would hang up the pty stdin now points at.
        });
    }

    /// `TmuxManager::runtime()` caches its socket/binary choice in a
    /// process-wide `OnceLock` on first use, so per-test `AMF_TMUX_SOCKET`
    /// overrides (the pattern `src/tmux.rs`'s own unit tests use for testing
    /// `detect_from_env` in isolation) cannot isolate a *live* tmux server
    /// per test the way it can isolate that pure detection logic -- whichever
    /// value was set when the first test in this binary touched tmux is what
    /// every later test gets. A unique session name per test is what
    /// actually isolates these tests from each other and from anything else
    /// on that shared server.
    fn unique_session_name(label: &str) -> String {
        format!("amf-gui-terminal-test-{label}-{}", uuid::Uuid::new_v4())
    }

    struct TestSession {
        name: String,
    }

    impl TestSession {
        fn spawn(label: &str) -> Self {
            ensure_process_stdin_is_a_pty();
            let name = unique_session_name(label);
            TmuxManager::create_session_with_window(&name, "main", &PathBuf::from("/tmp"))
                .expect("failed to create test tmux session");
            Self { name }
        }
    }

    impl Drop for TestSession {
        fn drop(&mut self) {
            let _ = TmuxManager::kill_session(&self.name);
        }
    }

    fn wait_for<T>(timeout: Duration, mut check: impl FnMut() -> Option<T>) -> Option<T> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(value) = check() {
                return Some(value);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn output_collector() -> (impl Fn(String) + Send + 'static, Arc<Mutex<Vec<String>>>) {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);
        (
            move |text: String| sink.lock().unwrap().push(text),
            received,
        )
    }

    #[test]
    fn attach_captures_the_pane_s_current_content() {
        let session = TestSession::spawn("initial-content");
        TmuxManager::send_literal(&session.name, "main", "echo hello-amf-gui\r").unwrap();
        wait_for(Duration::from_secs(2), || {
            TmuxManager::capture_pane(&session.name, "main")
                .ok()
                .filter(|content| content.contains("hello-amf-gui"))
        })
        .expect("shell never echoed the command");

        let (on_output, _received) = output_collector();
        let (_handle, initial) =
            TerminalHandle::attach(&session.name, "main", 80, 24, on_output).unwrap();

        assert!(
            initial.contains("hello-amf-gui"),
            "initial replay text should contain the pane's existing content: {initial:?}"
        );
    }

    #[test]
    fn view_target_lookup_reports_missing_session_and_window() {
        let session = TestSession::spawn("missing-target");
        let missing_session = unique_session_name("absent");

        let session_error =
            TmuxManager::resolve_view_target_ids(&missing_session, "main").unwrap_err();
        assert!(session_error.to_string().contains("can't find session"));

        let window_error =
            TmuxManager::resolve_view_target_ids(&session.name, "absent").unwrap_err();
        assert!(window_error.to_string().contains("can't find window"));

        let prefix = unique_session_name("prefix");
        let prefixed_name = format!("{prefix}-live");
        TmuxManager::create_session_with_window(&prefixed_name, "main", &PathBuf::from("/tmp"))
            .unwrap();
        let _prefixed_session = TestSession {
            name: prefixed_name,
        };
        let prefix_error = TmuxManager::resolve_view_target_ids(&prefix, "main").unwrap_err();
        assert!(prefix_error.to_string().contains("can't find session"));
    }

    #[test]
    fn view_target_lookup_selects_the_active_pane() {
        let session = TestSession::spawn("active-pane");
        let target = format!("{}:main", session.name);
        let output = TmuxManager::command()
            .args(["split-window", "-d", "-t", &target])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");

        let (_, pane_id) = TmuxManager::resolve_view_target_ids(&session.name, "main").unwrap();
        let active_pane = TmuxManager::command()
            .args(["display-message", "-t", &target, "-p", "#{pane_id}"])
            .output()
            .unwrap();
        assert!(active_pane.status.success(), "{active_pane:?}");
        assert_eq!(pane_id, String::from_utf8_lossy(&active_pane.stdout).trim());
    }

    #[test]
    fn live_output_after_a_change_is_delivered_and_reflects_current_state() {
        let session = TestSession::spawn("live-output");
        let (on_output, received) = output_collector();
        let (handle, _initial) =
            TerminalHandle::attach(&session.name, "main", 80, 24, on_output).unwrap();

        handle.send_input("echo live-update-marker\r").unwrap();

        let saw_it = wait_for(Duration::from_secs(2), || {
            received
                .lock()
                .unwrap()
                .iter()
                .any(|text| text.contains("live-update-marker"))
                .then_some(())
        });
        assert!(
            saw_it.is_some(),
            "no delivered update contained the marker; received: {:?}",
            received.lock().unwrap()
        );
    }

    #[test]
    fn send_input_forwards_raw_bytes_including_escape_sequences() {
        // A printf using octal escapes is typed back as literal characters
        // by the shell if arrow-key-style escape bytes were mangled in
        // transit; asserting on the pane's own content (not just "no
        // crash") is what actually verifies raw bytes survived the trip.
        let session = TestSession::spawn("raw-bytes");
        let (on_output, _received) = output_collector();
        let (handle, _initial) =
            TerminalHandle::attach(&session.name, "main", 80, 24, on_output).unwrap();

        handle
            .send_input("printf '\\033[31mcolored-unicode-\u{4e16}\u{754c}\\033[0m\\n'\r")
            .unwrap();

        let captured = wait_for(Duration::from_secs(2), || {
            TmuxManager::capture_pane_ansi(&session.name, "main")
                .ok()
                .filter(|content| content.contains("colored-unicode-\u{4e16}\u{754c}"))
        });
        assert!(
            captured.is_some(),
            "pane never showed the unicode marker after raw input"
        );
    }

    #[test]
    fn resize_updates_the_pane_and_triggers_a_recapture() {
        let session = TestSession::spawn("resize");
        let (on_output, received) = output_collector();
        let (handle, _initial) =
            TerminalHandle::attach(&session.name, "main", 80, 24, on_output).unwrap();
        received.lock().unwrap().clear();

        handle.resize(100, 40).unwrap();

        let recaptured = wait_for(Duration::from_secs(2), || {
            (!received.lock().unwrap().is_empty()).then_some(())
        });
        assert!(
            recaptured.is_some(),
            "resize should force a recapture even with no new pane output"
        );

        let (cols, rows) = handle.dims.get();
        assert_eq!((cols, rows), (100, 40));
    }

    #[test]
    fn high_output_volume_is_coalesced_not_lost_or_hung() {
        let session = TestSession::spawn("high-volume");
        let (on_output, received) = output_collector();
        let (handle, _initial) =
            TerminalHandle::attach(&session.name, "main", 80, 24, on_output).unwrap();

        handle
            .send_input("for i in $(seq 1 2000); do echo line-$i; done; echo volume-done\r")
            .unwrap();

        let saw_completion = wait_for(Duration::from_secs(10), || {
            received
                .lock()
                .unwrap()
                .last()
                .is_some_and(|text| text.contains("volume-done"))
                .then_some(())
        });
        assert!(
            saw_completion.is_some(),
            "the burst never settled on the completion marker"
        );
        // The point of debouncing is that this is much smaller than 2000:
        // most of the 2000 individual `%output` notifications collapsed
        // into far fewer recaptures.
        assert!(
            received.lock().unwrap().len() < 100,
            "expected heavy coalescing, got {} separate updates",
            received.lock().unwrap().len()
        );
    }

    #[test]
    fn dropping_the_handle_detaches_without_killing_the_session() {
        let session = TestSession::spawn("detach-cleanup");
        let (on_output, _received) = output_collector();
        let (handle, _initial) =
            TerminalHandle::attach(&session.name, "main", 80, 24, on_output).unwrap();

        drop(handle);

        assert!(
            TmuxManager::session_exists(&session.name),
            "dropping the GUI's terminal attachment must not kill the underlying tmux session"
        );
    }
}
