# AMF Performance Investigation & Remediation Plan

Date: 2026-06-10. Branch: `fable-perf`.

Goal: instant visual feedback in view mode (typing echo, pane
updates) and no degradation after AMF has been running for
hours/days. Every fix below is sized to be executed
independently by a future model. Work top-down: P0 items are
the ones that directly cause "delayed visuals after running a
while".

## Measured Evidence (from the live instance)

Collected from `~/.local/state/amf/debug.log` while AMF was
idle in view mode:

- `summary.poll_result n=4200-4400` per 5s summary window →
  the main loop spins at **~870 iterations/sec** in view mode.
- `view.capture_pane_ansi n=65 avg=8.6ms` per 5s → a tmux
  `capture-pane` **subprocess every 75ms (13/sec), even when
  the pane is idle**, each costing ~8.6ms.
- `ui.draw avg=4.4ms`, ~10 redraws/sec while idle.
- `sync.session_status` spiked to **120.84ms on the main
  thread** (single sample) — a visible input stall.
- `~/.local/state/amf/debug.log` is **1.16 GB / 10.4M lines**
  (86k lines today alone). No rotation exists.
- `~/.codex/sessions` is **295 MB** of JSONL transcripts;
  `~/.claude/projects` is 13 MB and grows with every agent
  turn.

## Root-Cause Findings (ranked)

### P0-1: Codex usage calculation runs on the main thread and re-parses growing JSONL files

`src/main.rs:835` and `src/main.rs:933` call
`app.usage.refresh()` synchronously inside `run_loop()`.
`UsageManager::refresh()` → `refresh_codex_stats()`
(`src/usage.rs:448`) calls `calculate_codex_usage()`
(`src/usage.rs:867`) **synchronously** whenever
`codex_usage_signature()` changes — which is every time any
active Codex session appends to its transcript, i.e.
constantly while agents work.

`calculate_codex_usage()`:

- Reads every `.jsonl` under today's + yesterday's
  `~/.codex/sessions/YYYY/MM/DD/` with `read_to_string`.
- Parses **every line twice**: once in
  `extract_rate_limits_from_json_line()` (full
  `serde_json::Value`) and once as `CodexSessionEvent`
  (`src/usage.rs:913-920`).
- Worse: when no rate-limit lines are found,
  `find_latest_codex_rate_limits()` (`src/usage.rs:1074`)
  walks **the entire multi-year session tree (295 MB here)**
  reading and parsing every file — on the main thread.

These files grow all day. This is the primary "slow after it
has been running for a while" mechanism: a main-thread stall
every 30s that grows linearly with the day's transcript
volume, blocking input handling and drawing.

`refresh_claude_stats()` (`src/usage.rs:402`) also does
main-thread I/O (`stats-cache.json` read+parse, plus the
`claude_today_signature()` directory walk), though the token
counting itself is already backgrounded.

**Fix:**

1. Move the whole stats refresh (codex signature, codex
   parse, claude stats-cache read, claude signature walk)
   onto the existing `usage-refresh` background thread (the
   OAuth fetch already lives there, `src/usage.rs:364`).
   `UsageManager.data` is already `Arc<Mutex<UsageData>>`, so
   the threading model already supports this. Keep
   `refresh()` itself non-blocking: it should only decide
   whether to spawn, never touch the filesystem.
2. Parse each line **once**: try
   `serde_json::from_str::<CodexSessionEvent>` first and read
   rate limits from the typed struct; only fall back to the
   raw-`Value` path when the typed parse fails. Or merge both
   extractors into a single function taking the parsed value.
3. Make codex usage **incremental**: cache
   `(path, last_len, accumulated_stats)` per file; on
   refresh, `seek` to `last_len` and parse only appended
   bytes (transcripts are append-only JSONL). Recompute from
   scratch only when a file shrinks.
4. Bound or remove `find_latest_codex_rate_limits()`: limit
   the fallback walk to the most recent N day-directories
   (e.g. 7), cache its result for the process lifetime, and
   run it on the background thread only.

**Verify:** add `record_duration` around the spawn decision;
`usage.refresh` p95 in the perf log should drop to <100us
permanently, and no main-thread metric should correlate with
transcript size.

### P0-2: View mode busy-spins the main loop at ~1000 Hz

`src/main.rs:662-670`: in view mode `poll_duration` is
`Duration::from_millis(1)`, used as the `libc::poll` timeout
(`src/main.rs:691`). The wakeup pipe (`SnapshotSender`,
`src/app/mod.rs:388-407`) already wakes the loop instantly
when a snapshot arrives, and stdin (fd 0) is in the same
`poll()` set, so keys also wake it instantly. The 1ms timeout
is pure busy-wait: ~870 iterations/sec each doing
`terminal.size()` (ioctl), two `redraw_signature()` hashes
(`src/main.rs:572`, `src/main.rs:1003`),
`poll_summary_result` + perf recording, IPC drain, etc. This
burns most of a core, increases scheduling pressure on the
snapshot worker and agent processes, and worsens battery and
contention as more sessions run.

**Fix:**

1. Raise the view-mode poll timeout to a deadline-computed
   value: the next due tick among {animation 125ms, thinking
   sync 500ms, status sync 5s, view idle refresh}. A simple
   first step: 50ms flat timeout. Immediacy is preserved
   because stdin and the wakeup pipe both interrupt `poll()`.
2. **Crossterm buffering caveat:** crossterm may have already
   buffered events internally, in which case fd 0 shows no
   data. Before sleeping in `libc::poll`, call
   `event::poll(Duration::ZERO)?` and skip the sleep if it
   returns true. This is required for correctness once the
   timeout is raised.
3. Have IPC delivery also write to the wakeup pipe (extend
   `ipc::IpcGuard` to carry the wakeup fd, or poll the IPC
   socket fd in the same `poll()` set) so notifications render
   instantly without relying on the tick.
4. Only call `poll_summary_result`/`record_duration` when
   `summary_rx.is_some()` (`src/main.rs:895-900`) to stop
   recording ~870 zero samples/sec.
5. Compute `redraw_signature()` once per iteration; keep the
   previous value across iterations instead of re-hashing at
   the top (`src/main.rs:572`).

**Verify:** `summary.poll_result n=...` (or a new
`loop.iterations` counter) should drop from ~4300/5s to <100/5s
idle; typing latency (`ui.input_to_draw`) must stay p95 <16ms.

### P0-3: The 75ms idle refresh forces a full reseed/capture 13×/sec, defeating event-driven rendering

`src/main.rs:801-811` requests
`request_view_snapshot_refresh()` (kind NORMAL) every
`VIEW_PANE_REFRESH_INTERVAL` (75ms) whenever the view is idle.
Consequences:

- Control-mode worker (`src/app/mod.rs:866-875`): NORMAL →
  `reseed_control_view_parser()` = spawn `capture-pane`
  subprocess + rebuild the vt100 parser + full re-render —
  **13×/sec**, throwing away the incremental `%output`
  streaming that is the whole point of control mode.
- Pipe-pane worker (`src/app/mod.rs:1100-1105`):
  `refresh_requested` sets `pane_has_new_output = true`, so it
  also captures every 75ms even with zero pane output —
  defeating its "zero subprocess overhead while idle" design.

This matches the measured 65 captures/5s at 8.6ms each while
idle (~11% of a core in subprocess spawns alone, plus the
rendering and redraws they trigger).

**Fix:**

1. Both workers already self-detect changes (`%output` events;
   FIFO data). The main loop's periodic request should become
   a slow **drift-correction reseed only**: every 2-5s, not
   75ms. Introduce e.g.
   `VIEW_DRIFT_RESEED_INTERVAL: Duration = 3s` and use it at
   `src/main.rs:804`.
2. In the control-mode worker, distinguish "drift reseed"
   (capture + parser rebuild) from "cursor-only refresh"; the
   parser already tracks the cursor (`parser_cursor`,
   `src/app/mod.rs:151`), so no `display-message` subprocess
   is needed in control mode.
3. In the pipe-pane worker, do not set
   `pane_has_new_output = true` for NORMAL requests; only
   bursts (typing) and FIFO data should trigger captures.
   Keep an interval-based capture as a fallback safety net at
   the new 2-5s drift cadence.
4. Keep the existing burst behavior on keypress
   (`request_view_snapshot_burst`) untouched — it is what
   makes echo feel instant.

**Verify:** with an idle pane, `view.capture_pane_ansi` should
drop to n≤2 per 5s window; with an actively streaming agent,
pane updates must still appear immediately (control mode:
`%output`-driven; pipe mode: FIFO-driven).

### P1-1: Debug log: 1.16 GB file, open/write/close per entry on the main thread

`src/debug.rs:103-119`: every log entry opens the file,
appends, closes — on whichever thread logs, including the main
thread. `log_to_file()` (`src/debug.rs:165`) does the same
from workers. There is no rotation; the file is 1.16 GB.
Logging is also chatty: every IPC message logs
(`src/app/notifications.rs:422`, plus per-message ipc lines
visible in the live log ~6 lines/sec), and perf summaries add
~17 lines/5s.

**Fix:**

1. Add size-based rotation: on startup and once per hour,
   if `debug.log` > 10 MB, rename to `debug.log.1` (keep one
   generation) and start fresh.
2. Keep a single shared `BufWriter<File>` behind a `Mutex`
   (global `OnceLock`), flushed on a 1s cadence by the
   existing flush tick (`should_flush_pending_debug_log_entries`,
   `src/app/mod.rs:2235`) and on exit/panic, instead of
   open/close per line.
3. Demote per-IPC-message logs (`thinking-start`,
   `tool-start/stop`, "Draining N message(s)") behind a
   config flag or a rate limiter (e.g. log only counts once
   per 5s window).
4. The DB `debug_log` table is already capped at 10k rows via
   trigger (`src/db/migrations.rs:84-89`) — no change needed,
   but consider moving the cap trigger to a periodic prune if
   insert profiling shows the trigger subquery matters.

**Verify:** `ls -lh ~/.local/state/amf/debug.log` stays under
the cap across days; no perf regression in `main.handle_key`.

### P1-2: Token tracking re-parses entire session transcripts on every change

`src/token_tracking.rs:212-299` (`read_claude_usage`) and
`src/token_tracking.rs:354-399` (`read_codex_usage`) read and
JSON-parse the **full transcript** whenever its
mtime/len signature changes — and an active session's
signature changes continuously, so this re-parse happens on
every 5s background sync (`sync_session_status_background`,
`src/app/sync.rs:301`), for every active session. CPU and
disk cost grows linearly with session length all day. It runs
off the main thread, but it competes for cores with the
render path and makes the box progressively busier — and the
`read_claude_usage` dedupe `HashSet` of every request id grows
with the transcript.

**Fix:**

1. Token usage is cumulative — make it incremental. Extend
   `UsageCacheEntry` with `parsed_through: u64` (byte offset)
   and running totals (and the dedupe set's recent tail, or
   drop dedupe for appended-only reads). On refresh, open the
   file, `seek(parsed_through)`, parse only new lines, add to
   totals. Full reparse only if `len < parsed_through`
   (truncation/rotation).
2. Same for the codex variant (`latest_total` means only the
   **last** `token_count` line matters — read the file
   backwards from the end, or keep the tail-seek approach and
   remember the last seen total).
3. Persist `parsed_through` + totals in the existing DB token
   cache (`src/db/token_cache.rs`) so restarts stay cheap.

**Verify:** with a multi-hundred-MB transcript, the 5s sync
thread's CPU should be near-zero when idle and O(new bytes)
when streaming.

### P1-3: `calculate_claude_today_tokens` re-reads all of today's transcripts on every change

`src/usage.rs:643-718`: background thread, but re-reads and
re-parses **every JSONL modified today across all projects**
whenever any of them changes (signature at
`src/usage.rs:720`). Same incremental treatment as P1-2:
per-file `(len, mtime, tokens_through_offset)` cache, parse
only appended bytes, and sum cached per-file totals.

### P2-1: PerfCollector leaks samples for never-drained metrics

`src/perf.rs:117-154`: `take_due_summary_lines` only drains a
hard-coded allowlist. Metrics recorded but not listed —
`sync.session_status_bg_start` (`src/main.rs:832`, every 5s),
`startup.*` — accumulate in `interval_samples_us` vectors
forever. Small (KB/day) but a true unbounded leak.

**Fix:** iterate `self.latencies` keys instead of the
hard-coded list (or drain non-listed metrics silently). Also
cap `interval_samples_us` at e.g. 100k samples defensively.

### P2-2: `sync_thinking_status` clones state and can spawn subprocesses every 500ms

`src/app/sync.rs:457-605`:

- Clones `thinking_features` and the entire `pending_inputs`
  vector (each entry ~20 `String`/`Option<String>` fields)
  every 500ms purely for change detection
  (`src/app/sync.rs:464-465`, compared at line 604).
- Opencode features without sidebar cache fall back to
  `TmuxManager::capture_pane` — **a subprocess per such
  feature per 500ms tick** (`src/app/sync.rs:486-496`).

**Fix:**

1. Replace clone-and-compare with a change flag: track
   mutations explicitly (insertions/removals already happen in
   identifiable spots) or compare cheap fingerprints (lengths
   + a hash of session ids).
2. Rate-limit the opencode capture-pane fallback to the 5s
   status sync rather than the 500ms thinking tick, or cache
   the result for N seconds per feature.

### P2-3: DB writes are unconditional full rewrites

- `flush_token_cache_to_db` (`src/app/mod.rs:2254`) writes
  every cache entry every 5s sync regardless of change. Add a
  dirty flag set by `read_usage` when an entry actually
  changes; skip the write otherwise.
- `db::store::save` (`src/db/store.rs:344`) does
  `DELETE FROM projects` + full re-insert. Acceptable at
  current call frequency, but ensure no new periodic caller
  appears; long-term, move to per-row upserts.

### P2-4: Redundant per-frame allocations in the view render path

- `pane_lines.to_vec()` clones all rendered lines on every
  draw (`src/ui/pane.rs:368`). Store
  `Arc<Vec<Line<'static>>>` in `App.pane_lines` and in
  `ViewSnapshot.rendered_lines`; clone the `Arc`, not the
  lines. Also lets `drain_view_snapshots` skip the full
  `Vec<Line>` equality compare (`src/app/mod.rs:1327`) by
  comparing a content hash computed once in the worker.
- `snapshot_from_parser` builds `contents_formatted()` (full
  screen string) on every incremental update
  (`src/app/mod.rs:174`); it is only consumed by selection
  mode and scroll seeding. Make `pane_content` lazy: include
  it only when selection is active or on demand.

### P2-5: Cursor polling via subprocess in pipe/capture workers

`cursor_position()` spawns `tmux display-message` every 125ms
while viewing (pipe worker `src/app/mod.rs:1158-1163`). In
control mode the parser already knows the cursor; in pipe
mode, only refresh the cursor when a pane capture actually
happened (they almost always move together), folding two
subprocesses into one.

## Architecture-Level Direction (larger, optional but recommended)

The codebase already has the right primitives (control-mode
streaming, wakeup pipe, background workers, signatures). The
remaining structural problem is that **the main loop is still
a polling loop with timers**, and **filesystem-derived state
(usage, tokens, prompts, thinking) is recomputed by scanning**
rather than observed.

1. **Single event-driven scheduler.** Replace the fixed
   `poll_duration` ladder with a tiny deadline registry: each
   subsystem registers its next-due `Instant`; the loop
   sleeps in `poll()` until `min(next_deadline)` or an fd
   event (stdin, snapshot pipe, IPC socket). This makes "zero
   work while idle" the default and every new feature pays
   its own scheduling cost explicitly. (P0-2 is the first
   step; this generalizes it.)
2. **One filesystem-watcher worker.** Add the `notify` crate
   (inotify) in a single background thread watching:
   `~/.codex/sessions/<today>`, the active Claude project
   dirs, `latest-prompt` files, and `/tmp/amf-thinking`. It
   converts events into the existing channel messages
   (sidebar load, usage dirty, thinking changed) and wakes
   the main loop via the wakeup pipe. All the 500ms/5s/30s
   scan timers become fallbacks at much longer intervals.
   This removes the entire class of "scan cost grows with
   data size" issues.
3. **One persistent tmux control-mode connection** for
   observation: a single `tmux -C` client subscribed to all
   `amf-*` sessions can deliver `%output`,
   `%session-changed`, `%window-close` etc. for **every**
   feature, replacing `list-sessions` polling
   (`sync_statuses`), per-feature `capture_pane` thinking
   fallbacks, and per-view worker respawns. The per-view
   worker remains only as the renderer for the focused pane.
4. **Latest-wins snapshot channel.** Replace the unbounded
   mpsc snapshot channel with a 1-slot mailbox
   (`Mutex<Option<ViewSnapshot>>` + wakeup): the renderer only
   ever needs the newest frame, so backlog can never form if
   the main thread stalls.

## Execution Order

Each step is independently shippable and verifiable. Use the
existing perf log (`D` in dashboard /
`~/.local/state/amf/debug.log`, `perf:` lines) before/after.

1. **P0-3** — slow the idle reseed, make workers purely
   event-driven (small diff in `src/main.rs` +
   `src/app/mod.rs` worker fns). Biggest visible win, lowest
   risk.
2. **P0-2** — raise view-mode poll timeout with the crossterm
   buffered-event guard; gate `poll_summary_result`; single
   `redraw_signature` per iteration.
3. **P0-1** — move codex/claude stats refresh fully off the
   main thread; single-parse per line; bound the fallback
   walk. (Incremental parsing can land later as P1-2/P1-3.)
4. **P1-1** — debug log rotation + buffered writer + chatty
   log demotion.
5. **P1-2 / P1-3** — incremental transcript parsing with
   persisted offsets.
6. **P2-1 … P2-5** — in any order; each is a contained diff.
7. **Architecture items** — adopt 1 (scheduler) and 4
   (mailbox) opportunistically while touching the loop; 2
   (notify watcher) and 3 (single control connection) as
   dedicated follow-up features.

## Acceptance Criteria

- Idle in view mode: <100 loop iterations / 5s, ≤2 pane
  captures / 5s, near-zero AMF CPU (`top`), no growth after
  24h uptime.
- Typing: `ui.input_to_draw` p95 < 16ms at all times,
  including while another agent streams output and while the
  usage refresh ticks.
- Streaming agent output appears with no added latency vs
  today (control-mode `%output` path).
- No main-loop metric (`main.handle_key`, `ui.draw`,
  `usage.refresh`, `sync.*`) shows max > 50ms in the perf
  summary after a full day of uptime.
- `debug.log` bounded; process RSS flat over 24h.

## Notes for Implementers

- Never use `println!`/`eprintln!` in TUI code; use
  `app.log_*` / `debug::log_to_file`.
- The perf summary allowlist lives in
  `src/perf.rs:130-148` — add any new metric names there (or
  implement P2-1 first so draining is automatic).
- The live instance the evidence came from runs release
  v0.20.0; line numbers above are from branch `fable-perf`.
- There are extensive tests in `src/app/tests.rs`; the view
  worker behavior changes (P0-3) should add tests around
  refresh-kind handling where the worker logic is factorable.
