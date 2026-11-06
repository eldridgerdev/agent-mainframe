use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use super::ViewSnapshot;

/// Latest-wins snapshot mailbox: the renderer only ever needs the newest
/// frame, so a single merged slot replaces an unbounded channel — backlog
/// cannot form if the main thread stalls. Every delivery also writes to
/// the wakeup pipe so the main loop leaves `poll()` immediately.
#[derive(Clone)]
pub(super) struct SnapshotSender {
    slot: Arc<StdMutex<Option<ViewSnapshot>>>,
    wakeup_fd: Arc<OwnedFd>,
}

pub(super) struct PaneRuntime {
    pub(super) view_snapshot_tx: SnapshotSender,
    pub(super) view_snapshot_wakeup_rx: Option<OwnedFd>,
    pub(super) view_snapshot_stop: Option<Arc<AtomicBool>>,
    pub(super) view_snapshot_refresh: Option<Arc<AtomicU8>>,
    pub(super) view_snapshot_include_content: Option<Arc<AtomicBool>>,
    pub(super) view_snapshot_condvar: Option<Arc<(StdMutex<()>, StdCondvar)>>,
    pub(super) view_snapshot_target: Option<(String, String, u16, u16)>,
    pub(super) view_display_frozen_until: Option<Instant>,
}

impl SnapshotSender {
    pub(super) fn new() -> (Self, OwnedFd) {
        let mut fds = [0i32; 2];
        unsafe {
            libc::pipe(fds.as_mut_ptr());
            libc::fcntl(fds[0], libc::F_SETFL, libc::O_NONBLOCK);
            // A full pipe already wakes poll, so losing a write is harmless.
            libc::fcntl(fds[1], libc::F_SETFL, libc::O_NONBLOCK);
            let read_end = OwnedFd::from_raw_fd(fds[0]);
            let write_end = OwnedFd::from_raw_fd(fds[1]);
            (
                Self {
                    slot: Arc::new(StdMutex::new(None)),
                    wakeup_fd: Arc::new(write_end),
                },
                read_end,
            )
        }
    }

    pub(super) fn send(&self, snap: ViewSnapshot) {
        {
            let mut slot = self.slot.lock().unwrap();
            merge_snapshot_into_slot(&mut slot, snap);
        }
        let byte = 1u8;
        unsafe {
            libc::write(
                self.wakeup_fd.as_raw_fd(),
                &byte as *const u8 as *const _,
                1,
            )
        };
    }

    pub(super) fn take_pending(&self) -> Option<ViewSnapshot> {
        self.slot.lock().unwrap().take()
    }

    pub(super) fn wakeup_fd(&self) -> Arc<OwnedFd> {
        Arc::clone(&self.wakeup_fd)
    }
}

/// Merge an undelivered snapshot with a newer one for the same pane:
/// newest value wins per field, so a cursor-only update cannot wipe out
/// pane content the main loop has not consumed yet.
fn merge_snapshot_into_slot(slot: &mut Option<ViewSnapshot>, new: ViewSnapshot) {
    match slot {
        Some(old) if old.session == new.session && old.window == new.window => {
            if new.pane_content.is_some() {
                old.pane_content = new.pane_content;
            }
            if new.rendered_lines.is_some() {
                old.rendered_lines = new.rendered_lines;
            }
            if new.cursor.is_some() {
                old.cursor = new.cursor;
            }
            if new.capture_duration.is_some() {
                old.capture_duration = new.capture_duration;
            }
            if new.render_duration.is_some() {
                old.render_duration = new.render_duration;
            }
            if new.cursor_duration.is_some() {
                old.cursor_duration = new.cursor_duration;
            }
            if new.pipe_read_duration.is_some() {
                old.pipe_read_duration = new.pipe_read_duration;
            }
        }
        _ => *slot = Some(new),
    }
}

use std::sync::Condvar as StdCondvar;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use super::{
    App, AppMode, VIEW_BURST_CURSOR_REFRESH_INTERVAL, VIEW_BURST_DURATION,
    VIEW_BURST_PANE_REFRESH_INTERVAL, VIEW_CONTROL_HEAL_INTERVAL,
    VIEW_CONTROL_SNAPSHOT_MIN_INTERVAL, VIEW_CURSOR_REFRESH_INTERVAL, VIEW_PANE_REFRESH_INTERVAL,
    VIEW_SNAPSHOT_REFRESH_BURST, VIEW_SNAPSHOT_REFRESH_NONE, VIEW_SNAPSHOT_REFRESH_NORMAL,
    VIEW_SNAPSHOT_REFRESH_PANE_BURST, VIEW_STARTUP_CURSOR_REFRESH_INTERVAL,
    VIEW_STARTUP_PANE_REFRESH_INTERVAL, VIEW_STARTUP_WARM_DURATION, parse_tmux_output_notification,
    render_ansi_lines, reseed_control_view_parser, sanitize_tmux_control_line,
};
use crate::tmux::TmuxManager;

impl App {
    pub(super) fn current_view_snapshot_target(&self) -> Option<(String, String, u16, u16)> {
        // The compose box keeps the live pane visible above it, so the
        // snapshot worker must stay on the viewed session while typing.
        let view = match &self.mode {
            AppMode::Viewing(view) => view,
            AppMode::Compose(state) => &state.view,
            _ => return None,
        };

        if self.pane_content_cols == 0 || self.pane_content_rows == 0 {
            return None;
        }

        Some((
            view.session.clone(),
            view.window.clone(),
            self.pane_content_cols,
            self.pane_content_rows,
        ))
    }

    pub(super) fn stop_view_snapshot_worker(&mut self) {
        if let Some(stop) = self.pane_runtime.view_snapshot_stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        // Wake the worker so it notices the stop flag immediately.
        if let Some(cv) = &self.pane_runtime.view_snapshot_condvar {
            cv.1.notify_one();
        }
        self.pane_runtime.view_snapshot_refresh = None;
        self.pane_runtime.view_snapshot_include_content = None;
        self.pane_runtime.view_snapshot_condvar = None;
        self.pane_runtime.view_snapshot_target = None;
    }

    /// Keep the worker's include-content flag in sync with whether
    /// selection currently needs `pane_content`. On the rising edge,
    /// request a refresh so full content arrives promptly.
    pub(super) fn sync_view_snapshot_content_flag(&self) {
        let Some(flag) = &self.pane_runtime.view_snapshot_include_content else {
            return;
        };
        let want = match &self.mode {
            AppMode::Viewing(view) => view.selection.is_selecting || view.selection.has_selection,
            _ => false,
        };
        if flag.swap(want, Ordering::Relaxed) != want && want {
            self.request_view_snapshot_refresh();
        }
    }

    pub fn request_view_snapshot_refresh(&self) {
        self.request_view_snapshot_refresh_kind(super::VIEW_SNAPSHOT_REFRESH_NORMAL);
    }

    pub fn request_view_snapshot_burst(&self) {
        self.request_view_snapshot_refresh_kind(super::VIEW_SNAPSHOT_REFRESH_BURST);
    }

    pub fn request_view_snapshot_pane_burst(&self) {
        self.request_view_snapshot_refresh_kind(super::VIEW_SNAPSHOT_REFRESH_PANE_BURST);
    }

    pub(super) fn request_view_snapshot_refresh_kind(&self, kind: u8) {
        if let Some(refresh) = &self.pane_runtime.view_snapshot_refresh {
            let mut current = refresh.load(Ordering::Relaxed);
            while current < kind {
                match refresh.compare_exchange_weak(
                    current,
                    kind,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => break,
                    Err(observed) => current = observed,
                }
            }
        }
        if let Some(cv) = &self.pane_runtime.view_snapshot_condvar {
            cv.1.notify_one();
        }
    }

    /// Fallback worker: polls capture-pane at regular intervals.
    // Worker wiring: every arg is a distinct channel/flag shared with the
    // spawning thread.
    #[allow(clippy::too_many_arguments)]
    fn run_capture_pane_worker(
        session: &str,
        window: &str,
        cols: u16,
        rows: u16,
        stop: &AtomicBool,
        refresh: &AtomicU8,
        condvar: &(StdMutex<()>, StdCondvar),
        tx: &SnapshotSender,
    ) {
        let mut next_pane_refresh = Instant::now() - VIEW_PANE_REFRESH_INTERVAL;
        let mut next_cursor_refresh = Instant::now() - VIEW_CURSOR_REFRESH_INTERVAL;
        let mut burst_until = Instant::now();
        let worker_started_at = Instant::now();

        while !stop.load(Ordering::Relaxed) {
            let now = Instant::now();
            let refresh_kind = refresh.swap(VIEW_SNAPSHOT_REFRESH_NONE, Ordering::Relaxed);
            let refresh_requested = refresh_kind != VIEW_SNAPSHOT_REFRESH_NONE;
            let cursor_refresh_requested = matches!(
                refresh_kind,
                VIEW_SNAPSHOT_REFRESH_NORMAL | VIEW_SNAPSHOT_REFRESH_BURST
            );
            if matches!(
                refresh_kind,
                VIEW_SNAPSHOT_REFRESH_BURST | VIEW_SNAPSHOT_REFRESH_PANE_BURST
            ) {
                burst_until = now + VIEW_BURST_DURATION;
            }
            let burst_active = now < burst_until;
            let startup_active = now.duration_since(worker_started_at) < VIEW_STARTUP_WARM_DURATION;
            let pane_interval = if burst_active {
                VIEW_BURST_PANE_REFRESH_INTERVAL
            } else if startup_active {
                VIEW_STARTUP_PANE_REFRESH_INTERVAL
            } else {
                VIEW_PANE_REFRESH_INTERVAL
            };
            let cursor_interval = if burst_active {
                VIEW_BURST_CURSOR_REFRESH_INTERVAL
            } else if startup_active {
                VIEW_STARTUP_CURSOR_REFRESH_INTERVAL
            } else {
                VIEW_CURSOR_REFRESH_INTERVAL
            };
            let pane_due =
                refresh_requested || now.duration_since(next_pane_refresh) >= pane_interval;
            let cursor_due = cursor_refresh_requested
                || now.duration_since(next_cursor_refresh) >= cursor_interval;

            if pane_due || cursor_due {
                let mut pane_content = None;
                let mut rendered_lines = None;
                let mut capture_duration = None;
                let mut render_duration = None;
                let mut cursor = None;
                let mut cursor_duration = None;

                if pane_due {
                    let started_at = Instant::now();
                    let captured =
                        TmuxManager::capture_pane_ansi(session, window).unwrap_or_default();
                    capture_duration = Some(started_at.elapsed());
                    let render_started_at = Instant::now();
                    rendered_lines = Some(render_ansi_lines(&captured, cols, rows));
                    render_duration = Some(render_started_at.elapsed());
                    pane_content = Some(captured);
                    next_pane_refresh = Instant::now();
                }

                if cursor_due {
                    let cursor_started_at = Instant::now();
                    cursor = Some(TmuxManager::cursor_position(session, window).ok());
                    cursor_duration = Some(cursor_started_at.elapsed());
                    next_cursor_refresh = Instant::now();
                }

                tx.send(ViewSnapshot {
                    session: session.to_string(),
                    window: window.to_string(),
                    pane_content,
                    rendered_lines,
                    cursor,
                    capture_duration,
                    render_duration,
                    cursor_duration,
                    pipe_read_duration: None,
                });
                continue;
            }

            let guard = condvar.0.lock().unwrap();
            let _ = condvar.1.wait_timeout(guard, Duration::from_millis(10));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_control_mode_view_worker(
        session: &str,
        window: &str,
        cols: u16,
        rows: u16,
        stop: &AtomicBool,
        refresh: &AtomicU8,
        _include_content: &AtomicBool,
        auto_refresh: bool,
        condvar: &(StdMutex<()>, StdCondvar),
        tx: &SnapshotSender,
    ) -> anyhow::Result<()> {
        let (target_window_id, mut target_pane_id) =
            TmuxManager::resolve_view_target_ids(session, window)?;
        let mut client = TmuxManager::spawn_control_mode_view_client(
            session,
            window,
            &target_pane_id,
            cols,
            rows,
        )?;
        let mut parser = vt100::Parser::new(rows, cols, 0);
        let mut debug_lines_remaining = 20usize;

        crate::debug::log_to_file(
            crate::debug::LogLevel::Info,
            "perf",
            &format!(
                "control-mode worker started for {session}:{window} window_id={target_window_id} pane_id={target_pane_id}"
            ),
        );

        let mut debug_initial_lines_remaining = 10usize;
        while let Some(raw_line) = client.try_recv()? {
            if debug_initial_lines_remaining > 0 {
                let line = sanitize_tmux_control_line(&raw_line);
                if !line.is_empty() {
                    crate::debug::log_to_file(
                        crate::debug::LogLevel::Debug,
                        "tmux",
                        &format!("control view initial recv: {line:?}"),
                    );
                    debug_initial_lines_remaining -= 1;
                }
            }
        }

        tx.send(reseed_control_view_parser(
            session,
            window,
            cols,
            rows,
            &mut parser,
        ));

        // The control stream is used purely as a change notifier;
        // content always comes from capture-pane reseeds. Feeding
        // %output into a persistent parser drifts whenever the agent
        // emits sequences vt100 interprets differently than tmux
        // (scroll regions, insert/delete line), which lands typed echo
        // on the wrong row. A reseed costs ~5ms in this worker thread,
        // so correctness wins: keystroke bursts reseed immediately for
        // low-latency echo, streaming output is coalesced so the main
        // thread is not saturated with full-frame redraws.
        let mut burst_until = Instant::now();
        let mut last_snapshot = Instant::now();
        let mut pane_dirty = false;

        while !stop.load(Ordering::Relaxed) {
            match refresh.swap(VIEW_SNAPSHOT_REFRESH_NONE, Ordering::Relaxed) {
                VIEW_SNAPSHOT_REFRESH_NORMAL => {
                    // Periodic drift-correction reseed.
                    tx.send(reseed_control_view_parser(
                        session,
                        window,
                        cols,
                        rows,
                        &mut parser,
                    ));
                    pane_dirty = false;
                    last_snapshot = Instant::now();
                }
                VIEW_SNAPSHOT_REFRESH_BURST | VIEW_SNAPSHOT_REFRESH_PANE_BURST => {
                    burst_until = Instant::now() + VIEW_BURST_DURATION;
                    pane_dirty = true;
                }
                _ => {}
            }

            // Non-blocking recv first; wait on condvar so burst signals from
            // request_view_snapshot_refresh_kind() wake this loop immediately.
            let line = match client.try_recv()? {
                Some(line) => Some(line),
                None => {
                    let guard = condvar.0.lock().unwrap();
                    let _ = condvar.1.wait_timeout(guard, Duration::from_millis(5));
                    client.try_recv()?
                }
            };

            if let Some(line) = line {
                let mut pending_lines = vec![line];
                while let Some(extra_line) = client.try_recv()? {
                    pending_lines.push(extra_line);
                }

                for raw_line in pending_lines {
                    let line = sanitize_tmux_control_line(&raw_line);
                    if line.is_empty() {
                        continue;
                    }
                    if debug_lines_remaining > 0 {
                        crate::debug::log_to_file(
                            crate::debug::LogLevel::Debug,
                            "tmux",
                            &format!("control view recv: {line:?}"),
                        );
                        debug_lines_remaining -= 1;
                    }

                    if let Some((pane_id, payload)) = parse_tmux_output_notification(line) {
                        if pane_id == target_pane_id && !payload.is_empty() {
                            pane_dirty = true;
                        }
                        continue;
                    }

                    if let Some(rest) = line.strip_prefix("%window-pane-changed ") {
                        let mut parts = rest.split_whitespace();
                        if parts.next() == Some(target_window_id.as_str())
                            && let Some(new_pane_id) = parts.next()
                        {
                            target_pane_id = new_pane_id.to_string();
                            pane_dirty = true;
                        }
                        continue;
                    }

                    // The window was resized (by us or by another client).
                    if let Some(rest) = line.strip_prefix("%layout-change ") {
                        if rest.split_whitespace().next() == Some(target_window_id.as_str()) {
                            pane_dirty = true;
                        }
                        continue;
                    }

                    if let Some(rest) = line.strip_prefix("%pane-mode-changed ")
                        && rest.trim() == target_pane_id
                    {
                        pane_dirty = true;
                        continue;
                    }

                    if let Some(rest) = line.strip_prefix("%pause ") {
                        let paused_pane = rest.trim();
                        if paused_pane == target_pane_id {
                            let _ = client.send_command(
                                &crate::tmux::TmuxManager::refresh_client_pane_command(
                                    paused_pane,
                                    "continue",
                                ),
                            );
                            pane_dirty = true;
                        }
                        continue;
                    }
                }
            }

            if pane_dirty {
                let now = Instant::now();
                if now < burst_until
                    || now.duration_since(last_snapshot) >= VIEW_CONTROL_SNAPSHOT_MIN_INTERVAL
                {
                    tx.send(reseed_control_view_parser(
                        session,
                        window,
                        cols,
                        rows,
                        &mut parser,
                    ));
                    pane_dirty = false;
                    last_snapshot = Instant::now();
                }
            } else if auto_refresh
                && Instant::now().duration_since(last_snapshot) >= VIEW_CONTROL_HEAL_INTERVAL
            {
                // Unconditional self-heal: the change-notifier can miss a
                // redraw, so re-capture the full pane on a steady floor to
                // keep a stale frame from persisting until the 3s drift
                // reseed. See VIEW_CONTROL_HEAL_INTERVAL.
                tx.send(reseed_control_view_parser(
                    session,
                    window,
                    cols,
                    rows,
                    &mut parser,
                ));
                last_snapshot = Instant::now();
            }
        }

        let _ = client.child.kill();
        Ok(())
    }

    /// Primary worker: uses pipe-pane to stream output and a persistent
    /// vt100 parser for incremental screen updates. No capture-pane
    /// subprocess is spawned after the initial seed.
    /// Primary worker: uses pipe-pane as a change-notification mechanism.
    /// When data arrives on the pipe, we know the pane has new output and
    /// run capture-pane to get the current screen state. When nothing
    /// arrives, we skip capture-pane entirely (zero subprocess overhead
    /// while idle). This avoids the persistent-parser state-mismatch
    /// problems while still eliminating polling when the pane is quiet.
    #[allow(clippy::too_many_arguments)]
    fn run_pipe_pane_worker(
        session: &str,
        window: &str,
        cols: u16,
        rows: u16,
        stop: &AtomicBool,
        refresh: &AtomicU8,
        condvar: &(StdMutex<()>, StdCondvar),
        tx: &SnapshotSender,
    ) -> anyhow::Result<()> {
        use std::fs;
        use std::io::Read as _;
        use std::os::unix::fs::OpenOptionsExt;

        // Slug the session name for a safe FIFO path.
        let slug: String = session
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let fifo_dir = std::path::PathBuf::from("/tmp/amf-pipes");
        fs::create_dir_all(&fifo_dir)?;
        let fifo_path = fifo_dir.join(format!("{slug}-{window}.pipe"));

        // Clean up any stale FIFO, then create a fresh one.
        let _ = fs::remove_file(&fifo_path);
        let c_path = std::ffi::CString::new(fifo_path.to_string_lossy().as_bytes())?;
        let rc = unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) };
        if rc != 0 {
            anyhow::bail!(
                "mkfifo failed for {}: {}",
                fifo_path.display(),
                std::io::Error::last_os_error()
            );
        }

        // Start pipe-pane: tmux streams pane output into our FIFO.
        TmuxManager::start_pipe_pane(session, window, &fifo_path)?;

        // Open FIFO with O_RDWR | O_NONBLOCK. O_RDWR keeps the FIFO open
        // (no EOF) even before the pipe-pane writer connects, because
        // we act as both reader and writer. O_NONBLOCK makes reads
        // return WouldBlock instead of blocking when no data is ready.
        let fifo_fd = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(&fifo_path)?;
        let mut fifo = std::io::BufReader::with_capacity(64 * 1024, fifo_fd);

        let mut read_buf = [0u8; 16384];
        let mut next_cursor_refresh = Instant::now() - VIEW_CURSOR_REFRESH_INTERVAL;
        let mut last_capture = Instant::now() - VIEW_PANE_REFRESH_INTERVAL;
        let mut pane_has_new_output = true; // Capture immediately on start.
        let mut burst_until = Instant::now();

        crate::debug::log_to_file(
            crate::debug::LogLevel::Info,
            "perf",
            &format!("pipe-pane worker started for {session}:{window}"),
        );

        while !stop.load(Ordering::Relaxed) {
            let read_started = Instant::now();

            // Drain all available data from the FIFO (non-blocking).
            // We don't parse it — we only care that *something* changed.
            loop {
                match fifo.read(&mut read_buf) {
                    Ok(0) => {
                        // With O_RDWR this shouldn't happen, but handle it.
                        break;
                    }
                    Ok(_) => {
                        pane_has_new_output = true;
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(e) => {
                        let _ = TmuxManager::stop_pipe_pane(session, window);
                        let _ = fs::remove_file(&fifo_path);
                        return Err(e.into());
                    }
                }
            }

            let pipe_read_duration = if pane_has_new_output {
                Some(read_started.elapsed())
            } else {
                None
            };

            let now = Instant::now();
            let refresh_kind = refresh.swap(VIEW_SNAPSHOT_REFRESH_NONE, Ordering::Relaxed);
            let refresh_requested = refresh_kind != VIEW_SNAPSHOT_REFRESH_NONE;
            let pane_refresh_requested = refresh_requested;
            if refresh_requested {
                pane_has_new_output = true;
            }
            let cursor_refresh_requested = matches!(
                refresh_kind,
                VIEW_SNAPSHOT_REFRESH_NORMAL | VIEW_SNAPSHOT_REFRESH_BURST
            );
            if matches!(
                refresh_kind,
                VIEW_SNAPSHOT_REFRESH_BURST | VIEW_SNAPSHOT_REFRESH_PANE_BURST
            ) {
                burst_until = now + VIEW_BURST_DURATION;
            }
            let burst_active = now < burst_until;

            // Throttle captures: use burst interval after input,
            // normal interval otherwise.
            let pane_interval = if burst_active {
                VIEW_BURST_PANE_REFRESH_INTERVAL
            } else {
                VIEW_PANE_REFRESH_INTERVAL
            };
            let pane_due = pane_refresh_requested
                || (pane_has_new_output && now.duration_since(last_capture) >= pane_interval);

            let cursor_interval = if burst_active {
                VIEW_BURST_CURSOR_REFRESH_INTERVAL
            } else {
                VIEW_CURSOR_REFRESH_INTERVAL
            };
            // Only poll the cursor (a `display-message` subprocess) when a
            // pane capture happens anyway — cursor and content move
            // together, and an idle pane must spawn no subprocesses.
            let cursor_due = cursor_refresh_requested
                || (pane_due && now.duration_since(next_cursor_refresh) >= cursor_interval);

            if pane_due || cursor_due {
                let mut pane_content = None;
                let mut rendered_lines = None;
                let mut capture_duration = None;
                let mut render_duration = None;
                let mut cursor = None;
                let mut cursor_duration = None;

                if pane_due {
                    pane_has_new_output = false;
                    let started_at = Instant::now();
                    let captured =
                        TmuxManager::capture_pane_ansi(session, window).unwrap_or_default();
                    capture_duration = Some(started_at.elapsed());
                    let render_started = Instant::now();
                    rendered_lines = Some(render_ansi_lines(&captured, cols, rows));
                    render_duration = Some(render_started.elapsed());
                    pane_content = Some(captured);
                    last_capture = Instant::now();
                }

                if cursor_due {
                    let cursor_started = Instant::now();
                    cursor = Some(TmuxManager::cursor_position(session, window).ok());
                    cursor_duration = Some(cursor_started.elapsed());
                    next_cursor_refresh = Instant::now();
                }

                tx.send(ViewSnapshot {
                    session: session.to_string(),
                    window: window.to_string(),
                    pane_content,
                    rendered_lines,
                    cursor,
                    capture_duration,
                    render_duration,
                    cursor_duration,
                    pipe_read_duration,
                });
                continue;
            }

            // No work to do — wait for wake or timeout.
            let guard = condvar.0.lock().unwrap();
            let _ = condvar.1.wait_timeout(guard, Duration::from_millis(10));
        }

        // Cleanup.
        let _ = TmuxManager::stop_pipe_pane(session, window);
        let _ = fs::remove_file(&fifo_path);
        Ok(())
    }

    /// Freeze the displayed pane on its current frame for `dur`. Incoming
    /// snapshots keep merging in the latest-wins mailbox but are not
    /// applied until the freeze lapses, at which point the freshest frame
    /// is revealed. Used to hide the re-anchor bounce.
    pub fn freeze_view_display(&mut self, dur: Duration) {
        self.pane_runtime.view_display_frozen_until = Some(Instant::now() + dur);
    }

    pub fn ensure_view_snapshot_worker(&mut self) {
        let Some((session, window, cols, rows)) = self.current_view_snapshot_target() else {
            self.stop_view_snapshot_worker();
            return;
        };

        if self.pane_runtime.view_snapshot_target.as_ref()
            == Some(&(session.clone(), window.clone(), cols, rows))
        {
            self.sync_view_snapshot_content_flag();
            return;
        }

        self.stop_view_snapshot_worker();
        self.pane_runtime.view_display_frozen_until = None;
        self.drain_view_snapshots();
        self.pane_lines.clear();

        let stop = Arc::new(AtomicBool::new(false));
        let refresh = Arc::new(AtomicU8::new(super::VIEW_SNAPSHOT_REFRESH_NORMAL));
        let include_content = Arc::new(AtomicBool::new(true));
        let condvar = Arc::new((StdMutex::new(()), StdCondvar::new()));
        let tx = self.pane_runtime.view_snapshot_tx.clone();
        let worker_session = session.clone();
        let worker_window = window.clone();
        let worker_cols = cols;
        let worker_rows = rows;
        let worker_stop = stop.clone();
        let worker_refresh = refresh.clone();
        let worker_include_content = include_content.clone();
        let worker_condvar = condvar.clone();
        let worker_auto_refresh = self.config.view_auto_refresh;

        std::thread::spawn(move || {
            if TmuxManager::uses_control_pty_input() {
                let control_result = Self::run_control_mode_view_worker(
                    &worker_session,
                    &worker_window,
                    worker_cols,
                    worker_rows,
                    &worker_stop,
                    &worker_refresh,
                    &worker_include_content,
                    worker_auto_refresh,
                    &worker_condvar,
                    &tx,
                );

                if let Err(e) = &control_result {
                    crate::debug::log_to_file(
                        crate::debug::LogLevel::Warn,
                        "perf",
                        &format!(
                            "control-mode view failed ({e}), falling back to pipe-pane snapshots"
                        ),
                    );
                }

                if control_result.is_ok() || worker_stop.load(Ordering::Relaxed) {
                    return;
                }
            }

            // Try pipe-pane approach first, fall back to polling capture-pane.
            let pipe_result = Self::run_pipe_pane_worker(
                &worker_session,
                &worker_window,
                worker_cols,
                worker_rows,
                &worker_stop,
                &worker_refresh,
                &worker_condvar,
                &tx,
            );

            if let Err(e) = &pipe_result {
                crate::debug::log_to_file(
                    crate::debug::LogLevel::Warn,
                    "perf",
                    &format!("pipe-pane failed ({e}), falling back to capture-pane polling"),
                );
            }

            // Fallback: original polling approach if pipe-pane failed
            // or if it exited early (e.g. FIFO issue).
            if pipe_result.is_err() && !worker_stop.load(Ordering::Relaxed) {
                Self::run_capture_pane_worker(
                    &worker_session,
                    &worker_window,
                    worker_cols,
                    worker_rows,
                    &worker_stop,
                    &worker_refresh,
                    &worker_condvar,
                    &tx,
                );
            }
        });

        self.pane_runtime.view_snapshot_stop = Some(stop);
        self.pane_runtime.view_snapshot_refresh = Some(refresh);
        self.pane_runtime.view_snapshot_include_content = Some(include_content);
        self.pane_runtime.view_snapshot_condvar = Some(condvar);
        self.pane_runtime.view_snapshot_target = Some((session, window, cols, rows));
        self.sync_view_snapshot_content_flag();
    }
}
