//! GUI terminal transport (`AMF_PLAN.md` Task 6). Reuses the TUI's
//! persistent tmux control-mode client
//! (`TmuxManager::spawn_control_mode_view_client`) rather than a new
//! PTY-attach path, per Task 2's resolved decision.
//!
//! The control-mode stream is used purely as a change notifier here, never
//! decoded for content -- real content always comes from a fresh
//! `capture_frame` (`capture-pane` plus the cursor), for the exact correctness
//! reason the TUI's own control-mode worker does this (see the inline
//! comment on the control stream in `App::run_control_mode_view_worker`,
//! `src/app/mod.rs`): replaying raw `%output` bytes through a second
//! terminal emulator can drift from what tmux itself renders for sequences
//! like scroll regions. That choice also means "buffering" and "reconnect"
//! need no dedicated machinery: there is no byte stream to buffer, and
//! reconnecting is just another `attach` -- a fresh capture already reflects
//! tmux's current state (including its own scrollback), not a replay log
//! this module would otherwise have to keep.
//!
//! ## Scrolling
//!
//! Recapturing the visible pane means xterm.js never accumulates scrollback
//! of its own, so a GUI scroll gesture is routed explicitly, the same way the
//! TUI's scroll mode (`App::toggle_scroll_mode`) and the Remote Control PWA's
//! History view already do it: a read-only snapshot of tmux history
//! ([`TerminalHandle::history`], built on the shared
//! [`TmuxManager::capture_scrollback`]) that the frontend loads into xterm's
//! own scrollback and freezes while the user reads. Nothing is sent to the
//! pane and tmux copy-mode is never entered, so no tmux state can be left
//! behind by a closed tab or seen by a TUI attached to the same session.
//!
//! The one case where a gesture does reach the program is a full-screen
//! (alternate-screen) program that has itself asked for mouse reporting --
//! OpenCode, Neovim. tmux keeps no history for it, and the program scrolls
//! its own view, so [`TerminalHandle::scroll_program`] forwards a mouse
//! *wheel report* exactly as a native terminal would -- never a keystroke,
//! and only after re-reading the pane's modes, so a program that has since
//! left the alternate screen or turned reporting off receives nothing.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::tmux::{
    MouseReporting, PaneTerminalModes, Scrollback, SpawnedTmuxControlClient, TmuxManager,
    parse_tmux_output_notification, sanitize_tmux_control_line,
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

/// The most wheel reports one [`TerminalHandle::scroll_program`] call sends,
/// so a runaway trackpad fling can't flood the program.
pub const MAX_WHEEL_STEPS: u8 = 10;

/// One full-pane update: the replay string plus the modes the frontend needs
/// to decide, synchronously inside a wheel event, where a scroll gesture
/// goes (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminalFrame {
    /// Normalized, cursor-positioned content to write after a full reset.
    pub replay: String,
    /// The program is on the alternate screen, so tmux has no history of it.
    pub alternate_screen: bool,
    /// The program asked for mouse reporting (its own wheel scrolling).
    pub mouse_reporting: bool,
}

impl TerminalFrame {
    /// `modes` is `None` when tmux couldn't report them; the frame then
    /// renders without a cursor move and claims neither mode, so a scroll
    /// falls back to the read-only history view rather than input.
    fn from_capture(
        captured: &str,
        modes: Option<PaneTerminalModes>,
        cols: u16,
        rows: u16,
    ) -> Self {
        Self {
            replay: replay_with_cursor(
                captured,
                modes.map(|m| (m.cursor_x, m.cursor_y)),
                cols,
                rows,
            ),
            alternate_screen: modes.is_some_and(|m| m.alternate_screen),
            mouse_reporting: modes.is_some_and(|m| m.mouse != MouseReporting::Off),
        }
    }
}

/// A read-only snapshot of the pane's tmux history plus its screen, for the
/// frontend to load into xterm's scrollback (see the module docs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TerminalHistory {
    /// History then screen, cursor-positioned like a [`TerminalFrame`]'s
    /// replay, so the screen lands on xterm's last rows. Empty when
    /// `alternate_screen` is set.
    pub replay: String,
    /// Lines above the current screen: zero means there is nothing earlier.
    pub earlier_lines: usize,
    /// The program is on the alternate screen; tmux has no history for it.
    pub alternate_screen: bool,
}

impl TerminalHistory {
    fn from_scrollback(
        scrollback: Scrollback,
        cursor: Option<(u16, u16)>,
        cols: u16,
        rows: u16,
    ) -> Self {
        match scrollback {
            Scrollback::AlternateScreen => Self {
                replay: String::new(),
                earlier_lines: 0,
                alternate_screen: true,
            },
            Scrollback::History { content, lines } => Self {
                replay: replay_with_cursor(&content, cursor, cols, rows),
                earlier_lines: lines.saturating_sub(usize::from(rows)),
                alternate_screen: false,
            },
        }
    }
}

/// Direction of one wheel step, as the frontend reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WheelDirection {
    Up,
    Down,
}

/// Captured pane text, normalized for a terminal emulator that has just been
/// reset, with the cursor placed by an explicit CUP escape -- `capture-pane`
/// output alone carries no cursor position. The cursor is clamped to the
/// given dimensions exactly as the TUI's own `position_parser_cursor` does.
/// See `crate::ui::pane::normalize_captured_pane` for why the newline
/// handling matters. A history capture puts the screen on the emulator's
/// last rows, so the same screen-relative CUP is correct for it too.
fn replay_with_cursor(captured: &str, cursor: Option<(u16, u16)>, cols: u16, rows: u16) -> String {
    let mut normalized = crate::ui::pane::normalize_captured_pane(captured);
    if let Some((x, y)) = cursor {
        let row = y.min(rows.saturating_sub(1)).saturating_add(1);
        let col = x.min(cols.saturating_sub(1)).saturating_add(1);
        normalized.push_str(&format!("\x1b[{row};{col}H"));
    }
    normalized
}

/// Deliberately separate from the TUI's `reseed_control_view_parser`
/// (`src/app/mod.rs`) rather than a shared refactor of it: that function
/// sits on a timing-sensitive, already-tuned rendering hot path.
fn capture_frame(session: &str, window: &str, cols: u16, rows: u16) -> TerminalFrame {
    let captured = TmuxManager::capture_pane_ansi(session, window).unwrap_or_default();
    let modes = TmuxManager::pane_terminal_modes(session, window).ok();
    TerminalFrame::from_capture(&captured, modes, cols, rows)
}

/// The mouse wheel report a native terminal would send for one wheel step at
/// zero-based cell `(col, row)`, or `None` when the pane's program has not
/// asked for one: off the alternate screen (tmux history is the scroll view
/// there) or without mouse reporting (the bytes would arrive as input).
pub fn wheel_report(
    modes: &PaneTerminalModes,
    direction: WheelDirection,
    col: u16,
    row: u16,
) -> Option<String> {
    if !modes.alternate_screen {
        return None;
    }
    let button: u16 = match direction {
        WheelDirection::Up => 64,
        WheelDirection::Down => 65,
    };
    let (x, y) = (col.saturating_add(1), row.saturating_add(1));
    match modes.mouse {
        MouseReporting::Off => None,
        MouseReporting::Sgr => Some(format!("\x1b[<{button};{x};{y}M")),
        // Each field is one byte offset by 32. Clamp to the ASCII range: the
        // text reaches tmux as UTF-8, where a larger value would arrive as a
        // two-byte sequence rather than the single byte this encoding means.
        MouseReporting::Legacy => {
            let byte = |value: u16| char::from((32 + value).min(126) as u8);
            Some(format!("\x1b[M{}{}{}", byte(button), byte(x), byte(y)))
        }
    }
}

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
    /// `replay_with_cursor`) to seed the caller's terminal
    /// emulator with before any live update arrives.
    ///
    /// `on_output` is called from a dedicated background thread with a full
    /// replacement frame each time the pane changes (debounced, not once per
    /// notification); the caller resets its terminal emulator and writes the
    /// frame's replay, exactly like the initial seed.
    pub fn attach(
        session: &str,
        window: &str,
        cols: u16,
        rows: u16,
        on_output: impl Fn(TerminalFrame) + Send + 'static,
    ) -> Result<(Self, TerminalFrame)> {
        let (_target_window_id, target_pane_id) =
            TmuxManager::resolve_view_target_ids(session, window)?;
        let client = TmuxManager::spawn_control_mode_view_client(
            session,
            window,
            &target_pane_id,
            cols,
            rows,
        )?;

        let initial = capture_frame(session, window, cols, rows);

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

    /// A read-only snapshot of the pane's tmux history plus its screen (see
    /// the module docs). Sends nothing to the pane and leaves tmux's modes
    /// untouched.
    pub fn history(&self) -> Result<TerminalHistory> {
        let (cols, rows) = self.dims.get();
        let scrollback = TmuxManager::capture_scrollback(&self.session, &self.window)?;
        let cursor = TmuxManager::pane_terminal_modes(&self.session, &self.window)
            .ok()
            .map(|modes| (modes.cursor_x, modes.cursor_y));
        Ok(TerminalHistory::from_scrollback(
            scrollback, cursor, cols, rows,
        ))
    }

    /// Forward `steps` wheel steps at zero-based cell `(col, row)` to a
    /// full-screen program that asked for mouse reporting. The pane's modes
    /// are re-read first rather than trusted from the frontend's last frame:
    /// if the program has left the alternate screen or turned reporting off,
    /// nothing is sent and this returns `false`.
    pub fn scroll_program(
        &self,
        direction: WheelDirection,
        steps: u8,
        col: u16,
        row: u16,
    ) -> Result<bool> {
        let modes = TmuxManager::pane_terminal_modes(&self.session, &self.window)?;
        let Some(report) = wheel_report(&modes, direction, col, row) else {
            return Ok(false);
        };
        let steps = usize::from(steps.clamp(1, MAX_WHEEL_STEPS));
        TmuxManager::send_literal(&self.session, &self.window, &report.repeat(steps))?;
        Ok(true)
    }

    /// Ask for a fresh frame even if the pane hasn't changed -- the frontend
    /// leaving its frozen history view wants the current screen, not
    /// whichever frame it last set aside.
    pub fn refresh(&self) {
        self.dirty.store(true, Ordering::Relaxed);
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
    on_output: impl Fn(TerminalFrame) + Send + 'static,
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
            on_output(capture_frame(&session, &window, cols, rows));
            last_capture = Instant::now();
        }
    }
}

// These use a real tmux server rather than `MockTmuxOps`: a mock cannot
// meaningfully stand in for a real PTY/control-mode client, and this
// transport's entire job is the plumbing between them. Under `cfg(test)`
// `TmuxManager::runtime()` resolves to a throwaway per-process socket
// (`TmuxRuntime::isolated_for_tests`), never the user's live AMF server --
// this suite's control-client churn has crashed tmux 3.2a, and on the shared
// server that took every real session down with it. Tests within one binary
// still share that server; a unique session name per test isolates them from
// each other.
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
    /// process-wide `OnceLock` on first use, so every test in this binary
    /// shares one (throwaway, per-process) tmux server. A unique session name
    /// per test is what isolates these tests from each other on it.
    fn unique_session_name(label: &str) -> String {
        format!("{TEST_SESSION_PREFIX}{label}-{}", uuid::Uuid::new_v4())
    }

    const TEST_SESSION_PREFIX: &str = "amf-gui-terminal-test-";

    /// Older than any run of these tests takes, so no live run owns it.
    const LEAKED_TEST_SESSION_AGE_SECS: i64 = 15 * 60;

    /// `TestSession::drop` never runs when the test binary is killed (an OOM
    /// kill, Ctrl+C), and those sessions then outlive it on the user's real
    /// AMF tmux server. Each binary sweeps them once, before its first
    /// session, leaving any a concurrent run may still own.
    fn sweep_leaked_test_sessions() {
        static SWEEP: std::sync::Once = std::sync::Once::new();
        SWEEP.call_once(|| {
            let Ok(output) = TmuxManager::command()
                .args(["list-sessions", "-F", "#{session_created} #{session_name}"])
                .output()
            else {
                return;
            };
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs() as i64);
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let Some((created, name)) = line.split_once(' ') else {
                    continue;
                };
                let stale = created
                    .parse::<i64>()
                    .is_ok_and(|created| now - created > LEAKED_TEST_SESSION_AGE_SECS);
                if stale && name.starts_with(TEST_SESSION_PREFIX) {
                    let _ = TmuxManager::kill_session(name);
                }
            }
        });
    }

    struct TestSession {
        name: String,
    }

    impl TestSession {
        fn spawn(label: &str) -> Self {
            ensure_process_stdin_is_a_pty();
            sweep_leaked_test_sessions();
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

    fn output_collector() -> (
        impl Fn(TerminalFrame) + Send + 'static,
        Arc<Mutex<Vec<String>>>,
    ) {
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&received);
        (
            move |frame: TerminalFrame| sink.lock().unwrap().push(frame.replay),
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

        assert!(!initial.alternate_screen && !initial.mouse_reporting);
        assert!(
            initial.replay.contains("hello-amf-gui"),
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

    fn modes(alternate_screen: bool, mouse: MouseReporting) -> PaneTerminalModes {
        PaneTerminalModes {
            cursor_x: 3,
            cursor_y: 40,
            alternate_screen,
            mouse,
        }
    }

    #[test]
    fn frames_place_the_cursor_and_carry_the_scroll_routing_modes() {
        let frame = TerminalFrame::from_capture(
            "one\ntwo\n",
            Some(modes(true, MouseReporting::Sgr)),
            80,
            24,
        );
        // The cursor row is clamped to the screen, as the TUI clamps it.
        assert_eq!(frame.replay, "one\r\ntwo\x1b[24;4H");
        assert!(frame.alternate_screen && frame.mouse_reporting);

        let unknown = TerminalFrame::from_capture("one\n", None, 80, 24);
        assert_eq!(unknown.replay, "one");
        assert!(!unknown.alternate_screen && !unknown.mouse_reporting);
    }

    #[test]
    fn history_counts_only_lines_above_the_screen() {
        let content = (1..=30).map(|n| format!("line-{n}\n")).collect::<String>();
        let history = TerminalHistory::from_scrollback(
            Scrollback::History { content, lines: 30 },
            Some((0, 9)),
            80,
            10,
        );
        assert_eq!(history.earlier_lines, 20);
        assert!(!history.alternate_screen);
        assert!(history.replay.starts_with("line-1\r\nline-2\r\n"));
        assert!(history.replay.ends_with("line-30\x1b[10;1H"));

        let short = TerminalHistory::from_scrollback(
            Scrollback::History {
                content: "prompt$\n".into(),
                lines: 1,
            },
            None,
            80,
            10,
        );
        assert_eq!(short.earlier_lines, 0);

        let full_screen =
            TerminalHistory::from_scrollback(Scrollback::AlternateScreen, Some((0, 0)), 80, 10);
        assert!(full_screen.alternate_screen);
        assert_eq!(
            (full_screen.replay.as_str(), full_screen.earlier_lines),
            ("", 0)
        );
    }

    #[test]
    fn wheel_reports_only_reach_full_screen_programs_that_asked_for_them() {
        assert_eq!(
            wheel_report(&modes(true, MouseReporting::Sgr), WheelDirection::Up, 4, 2).as_deref(),
            Some("\x1b[<64;5;3M")
        );
        assert_eq!(
            wheel_report(
                &modes(true, MouseReporting::Sgr),
                WheelDirection::Down,
                0,
                0
            )
            .as_deref(),
            Some("\x1b[<65;1;1M")
        );
        assert_eq!(
            wheel_report(
                &modes(true, MouseReporting::Legacy),
                WheelDirection::Up,
                4,
                2
            )
            .as_deref(),
            Some("\x1b[M`%#")
        );
        // Legacy coordinates past the one-byte range clamp instead of
        // arriving as multi-byte UTF-8.
        let far = wheel_report(
            &modes(true, MouseReporting::Legacy),
            WheelDirection::Down,
            300,
            300,
        )
        .unwrap();
        assert!(far.is_ascii(), "{far:?}");
        assert_eq!(far, "\x1b[Ma~~");

        // A program without mouse reporting would read the bytes as typed
        // input, and on the normal screen tmux history is the scroll view.
        assert_eq!(
            wheel_report(&modes(true, MouseReporting::Off), WheelDirection::Up, 0, 0),
            None
        );
        assert_eq!(
            wheel_report(&modes(false, MouseReporting::Sgr), WheelDirection::Up, 0, 0),
            None
        );
    }

    /// The screen once the shell has finished drawing: an interactive
    /// prompt can repaint asynchronously after a command's output lands.
    fn settled_screen(session: &str) -> String {
        let mut last = TmuxManager::capture_pane(session, "main").unwrap();
        for _ in 0..50 {
            std::thread::sleep(Duration::from_millis(200));
            let now = TmuxManager::capture_pane(session, "main").unwrap();
            if now == last {
                return now;
            }
            last = now;
        }
        last
    }

    fn pane_in_copy_mode(session: &str) -> bool {
        let output = TmuxManager::command()
            .args([
                "display-message",
                "-t",
                &format!("{session}:main"),
                "-p",
                "#{pane_in_mode}",
            ])
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).trim() != "0"
    }

    #[test]
    fn history_reads_earlier_output_without_touching_the_pane() {
        let session = TestSession::spawn("history");
        let (on_output, _received) = output_collector();
        let (handle, _initial) =
            TerminalHandle::attach(&session.name, "main", 80, 24, on_output).unwrap();
        handle
            // No `$`: the persistent input client's tmux command quoting
            // expands `$name` (see the report on this increment).
            .send_input("seq -f history-line-%g 1 200\r")
            .unwrap();
        wait_for(Duration::from_secs(10), || {
            TmuxManager::capture_pane(&session.name, "main")
                .ok()
                .filter(|content| content.contains("history-line-200"))
        })
        .unwrap_or_else(|| {
            panic!(
                "the shell never printed the burst: {:?}",
                TmuxManager::capture_pane(&session.name, "main")
            )
        });
        let before = settled_screen(&session.name);

        let history = handle.history().unwrap();

        assert!(!history.alternate_screen);
        assert!(
            history.earlier_lines >= 170,
            "expected the burst above the screen, got {} earlier lines",
            history.earlier_lines
        );
        assert!(history.replay.contains("history-line-1\r\n"));
        assert!(history.replay.contains("history-line-200"));
        // Read-only: no copy-mode, no input, the screen unchanged.
        assert!(!pane_in_copy_mode(&session.name));
        assert_eq!(settled_screen(&session.name), before);
        // A shell on the normal screen never receives wheel reports.
        assert!(!handle.scroll_program(WheelDirection::Up, 3, 0, 0).unwrap());
        assert_eq!(settled_screen(&session.name), before);
    }

    #[test]
    fn full_screen_programs_get_wheel_reports_only_when_they_ask() {
        let session = TestSession::spawn("full-screen");
        let received = tempfile::NamedTempFile::new().unwrap();
        let path = received.path().display().to_string();
        let (on_output, _frames) = output_collector();
        let (handle, _initial) =
            TerminalHandle::attach(&session.name, "main", 80, 24, on_output).unwrap();

        // Alternate screen without mouse reporting: no history, no input.
        handle
            .send_input(&format!(
                "printf '\\033[?1049h'; stty -echo -icanon; head -c 10 > {path}\r"
            ))
            .unwrap();
        wait_for(Duration::from_secs(10), || {
            TmuxManager::pane_terminal_modes(&session.name, "main")
                .ok()
                .filter(|modes| modes.alternate_screen)
        })
        .expect("the program never entered the alternate screen");
        assert!(handle.history().unwrap().alternate_screen);
        assert!(!handle.scroll_program(WheelDirection::Up, 1, 4, 2).unwrap());

        // Once it asks for SGR mouse reporting, one wheel step arrives as
        // exactly one report -- read back by the program itself.
        handle.send_input("\x03").unwrap();
        handle
            .send_input(&format!(
                "printf '\\033[?1049h\\033[?1000h\\033[?1006h'; stty -echo -icanon; head -c 10 > {path}\r"
            ))
            .unwrap();
        wait_for(Duration::from_secs(10), || {
            TmuxManager::pane_terminal_modes(&session.name, "main")
                .ok()
                .filter(|modes| modes.alternate_screen && modes.mouse == MouseReporting::Sgr)
        })
        .expect("the program never asked for mouse reporting");
        assert!(handle.scroll_program(WheelDirection::Up, 1, 4, 2).unwrap());
        let report = wait_for(Duration::from_secs(10), || {
            std::fs::read(received.path())
                .ok()
                .filter(|bytes| bytes.len() == 10)
        })
        .expect("the program never read a complete wheel report");
        assert_eq!(report, b"\x1b[<64;5;3M");
        assert!(!pane_in_copy_mode(&session.name));
    }

    /// The TUI's own embedded scrolling reads the shared snapshot too: scroll
    /// mode opens at the bottom of the same tmux history, scrolls within it,
    /// and still hands a full-screen program its keys (passthrough).
    #[test]
    fn tui_scroll_mode_reads_the_shared_history_snapshot() {
        use crate::app::{App, AppMode, ViewState};
        use crate::project::{ProjectStore, SessionKind, VibeMode};
        use crate::traits::{MockTmuxOps, MockWorktreeOps};

        let session = TestSession::spawn("tui-scroll");
        TmuxManager::send_literal(&session.name, "main", "seq -f tui-line-%g 1 120\r").unwrap();
        wait_for(Duration::from_secs(10), || {
            TmuxManager::capture_pane(&session.name, "main")
                .ok()
                .filter(|content| content.contains("tui-line-120"))
        })
        .expect("the shell never printed the burst");
        settled_screen(&session.name);

        let store = ProjectStore {
            version: 5,
            projects: Vec::new(),
            session_bookmarks: Vec::new(),
            available_harnesses: Vec::new(),
            prompt_templates: Vec::new(),
            extra: Default::default(),
        };
        let mut app = App::new_for_test(
            store,
            Box::new(MockTmuxOps::new()),
            Box::new(MockWorktreeOps::new()),
        );
        app.mode = AppMode::Viewing(ViewState::new(
            "demo".into(),
            "feature".into(),
            session.name.clone(),
            "main".into(),
            "Claude".into(),
            SessionKind::Claude,
            VibeMode::Vibeless,
            false,
        ));
        let view = |app: &App| match &app.mode {
            AppMode::Viewing(view) => view.clone(),
            _ => panic!("expected Viewing mode"),
        };

        app.toggle_scroll_mode(20);
        let opened = view(&app);
        assert!(opened.scroll_mode && !opened.scroll_passthrough);
        assert!(opened.scroll_content.contains("tui-line-1\n"));
        assert_eq!(opened.scroll_offset, opened.scroll_total_lines - 20);
        app.scroll_up(5);
        assert_eq!(view(&app).scroll_offset, opened.scroll_offset - 5);
        app.toggle_scroll_mode(20);
        assert!(!view(&app).scroll_mode);
        assert!(!pane_in_copy_mode(&session.name));

        TmuxManager::send_literal(&session.name, "main", "printf '\\033[?1049h'; sleep 30\r")
            .unwrap();
        wait_for(Duration::from_secs(10), || {
            TmuxManager::pane_terminal_modes(&session.name, "main")
                .ok()
                .filter(|modes| modes.alternate_screen)
        })
        .expect("the program never entered the alternate screen");
        app.toggle_scroll_mode(20);
        let full_screen = view(&app);
        assert!(full_screen.scroll_mode && full_screen.scroll_passthrough);
        assert_eq!(full_screen.scroll_total_lines, 0);
    }
}
