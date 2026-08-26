# Remote Control — companion app

- **Status:** Ready
- **Owner:** unassigned
- **Relates to:** shipped interactive Remote Control (v0.24.0, see
  `CHANGELOG.md`) — bridges **one Claude session at a time** to
  claude.ai/code or the Claude mobile app via Anthropic's own
  infrastructure (`claude --remote-control`). This plan is a separate,
  AMF-owned capability: a dashboard-wide view across **every feature and
  every harness** (Claude, Codex, opencode, Pi), with its own auth
  model, its own server, and its own client app. The two are
  complementary, not competing — a user could have both enabled.
  [Remote Control — server mode](remote-control-server-mode-plan.md) is
  also related but orthogonal: it is about **spawning new sessions**
  remotely (provision-and-review), where this plan is about **observing
  and driving sessions AMF already created**.
  [Remote Control — QR code overlay](remote-control-qr-overlay-plan.md)
  designs a TUI QR overlay for the *shipped* feature's session URL; this
  plan needs its own, different QR (a pairing code, not a session URL —
  see Epic 4) and does not depend on that overlay landing first.

## Why / problem

Today, checking on or steering an AMF-managed agent session requires
being at the machine running AMF. There is no way to see which features
need attention, answer a blocked agent's question, or type into a
session from a phone. The goal: let a user monitor and, eventually,
fully interact with their AMF agent sessions from a phone — starting
with read-only status/notifications and growing toward full terminal
control — over a connection method the user configures per setup (LAN
and/or tunnel).

## Decisions (settled)

These were confirmed with the user during a Phase 0 interview
(2026-08-25) and should be treated as settled unless revisited
explicitly:

- **Scope/phasing**: all three interaction levels are in scope for v1,
  delivered as ordered phases: (1) read-only status & notifications,
  (2) responding to agent prompts, (3) full interactive terminal
  control.
- **Network exposure**: configurable per setup — both LAN (same Wi-Fi)
  and a tunnel method are supported from v1; the user picks per
  session/device.
- **Server lifecycle**: the remote-control server is on-demand only,
  toggled on/off by the user; it is not a background daemon that runs
  automatically whenever AMF is running.
- **Terminal rendering**: the client supports both a full interactive
  terminal (xterm.js-style, over WebSocket) and a simplified
  mobile-friendly view, user-selectable.
- **Push notifications**: in scope — the phone should be notified when
  an agent needs attention, mirroring AMF's existing attention (`i`)
  view.
- **Concurrent access**: shared read/write between phone and local
  desktop session with no conflict resolution — both sides can type
  into the same pane; last input wins at the terminal level, same as
  two local terminals attached to one tmux session.
- **Client**: a cross-platform native app built with **Flutter** (iOS +
  Android from one Dart codebase) — not a PWA. Chosen over a PWA for
  more reliable push delivery and native terminal performance, at the
  cost of app-store distribution and a new (non-Rust) toolchain. The
  terminal view uses an embedded WebView hosting xterm.js for
  full-terminal mode; the simplified view is built with native Flutter
  widgets.
- **Auth**: QR-code pairing — AMF desktop shows a QR code encoding a
  one-time, short-lived pairing code, the phone scans it and exchanges
  it for a long-lived, per-device secret token stored in the app and in
  a new `remote_devices` table. Every subsequent connection
  authenticates with that per-device token; the desktop side keeps a
  paired-device list with individual revoke.
- **Tunnel mechanism**: integrate with an existing third-party tool
  (Tailscale, ngrok, or cloudflared) rather than building relay
  infrastructure. Which one to document/support first is still open
  (see Open questions).
- **DB concurrency**: channel-routed writes — all remote-triggered DB
  writes are marshalled through the same mpsc channel/main-loop pattern
  as other App-state changes, so the main loop remains the sole SQLite
  writer. No second connection or WAL mode.

## Architecture

- **New always-addressable pieces**:
  - A remote-control server (HTTP + WebSocket) toggled on/off from the
    AMF dashboard, bound to LAN by default, with an optional tunnel
    connection mode via an existing third-party tool the user installs
    and configures — AMF does not run its own relay infrastructure.
  - A `remote_devices` table (SQLite, alongside the existing
    `~/.config/amf/amf.db` schema and migration pattern used by
    `todo_lists`/`todos`) storing device id, per-device token (hashed at
    rest), pairing time, last-seen time, and revoked flag.
  - A Flutter mobile app (iOS + Android), built and distributed
    separately from the AMF server process rather than served as static
    assets, with three views matching the phases: status/notification
    list, prompt-response view, terminal view (xterm.js via embedded
    WebView, or a native simplified view, user-selectable).
- **Integrating with the existing synchronous app**: AMF's event loop
  (`main.rs::run_loop`) and `App` state are synchronous today. The
  remote server needs an async runtime (tokio + an HTTP/WS framework
  such as axum) running on its own thread(s), started and stopped when
  the toggle flips. Rather than let that runtime touch `App` state
  directly, remote requests are marshalled onto the main loop via an
  `mpsc` channel (the same shape as other cross-thread notification
  patterns already in `app/`), and responses/state pushed back the same
  way — the remote server never mutates `App` or reads `TmuxManager`
  output directly from its own thread. This is the single largest
  source of server-side implementation risk in the plan: it is the
  codebase's first async runtime alongside ratatui's synchronous poll
  loop.
- **Database concurrency**: resolved as channel-routed writes (see
  Decisions) — the main loop remains the sole SQLite writer.
- **Notifications vs. the on-demand toggle**: there is a tension between
  "server is off by default, user toggles it on" and "push
  notifications should fire when an agent needs attention while the
  user is away" — if the remote server must be manually enabled, it may
  not be running at the moment an agent actually needs attention. This
  plan resolves it by splitting concerns: attention detection stays in
  AMF's existing polling (`app/notifications.rs`) and runs whenever AMF
  itself is running, independent of the remote-control toggle; only the
  *interactive* remote-control server (pairing, WebSocket
  terminal/status access) is gated by the on/off toggle. Actual
  notification delivery (via Firebase Cloud Messaging) still requires a
  paired device and a reachable push endpoint, so the toggle still
  affects whether push credentials exist, but not whether attention is
  detected. This split is this plan's proposed resolution to a real
  tension between two settled decisions, not something the user
  explicitly confirmed — flagged in Open questions.
- **Reused existing pieces**: `TmuxManager::capture_pane_ansi` for
  terminal snapshots/streaming, `app/notifications.rs` scan logic as the
  source of attention events, `Feature`/`ProjectStatus` for status
  payloads, the existing SQLite migration conventions (`src/db/`) for
  the new `remote_devices` table.

## UI

- **AMF desktop (ratatui)**:
  - A remote-access toggle (on/off) surfaced in settings or the
    leader-command menu, showing current state and connection mode
    (LAN/tunnel).
  - A pairing dialog that renders a QR code and pairing code, and a
    paired-devices list with per-device revoke.
- **Phone (Flutter app, iOS + Android)**:
  - Pairing/scan screen.
  - Phase 1: read-only status/notification list mirroring the attention
    (`i`) view.
  - Phase 2: prompt-response view for answering an agent's question
    without a full terminal.
  - Phase 3: terminal view with a toggle between full xterm.js rendering
    (embedded WebView) and the native simplified mobile view; a
    connection-mode indicator (LAN/tunnel).

## Progress

| Epic | Priority | Needs | Summary |
|---|---|---|---|
| 1. Server skeleton | P0 | — | tokio/axum thread, on/off toggle, mpsc channel to main loop |
| 2. Device storage | P0 | — | `remote_devices` migration + `src/db/` module |
| 3. Native app groundwork | P0 | — | Flutter scaffold, store accounts, signing, CI build |
| 4. Pairing flow | P1 | 1, 2 | QR/code pairing, token issuance, lockout |
| 5. Status/notification relay | P1 | 1 | read-only status feed from `app/notifications.rs` |
| 6. App shell + push | P1 | 3; partial on 4, 5 | pairing/status screens, FCM push |
| 7. Device revoke | P1 | 4 | desktop revoke UI, live teardown, token rejection |
| 8. Prompt response | P2 | 1, 5, 6 | read/answer a blocked agent's question |
| 9. Terminal streaming (backend) | P3 | 1, 4 | `capture_pane_ansi` over WS, keystroke forwarding |
| 10. Client rendering modes | P3 | 6, 9 | xterm.js WebView + native simplified view, toggle |

Each epic below has its own checklist and verification. Check items off
as they land; keep this doc current.

### Epic 1 — Server skeleton (P0)

Add tokio/axum (or equivalent) as a new dependency, run it on a
dedicated thread started/stopped by the on/off toggle, and wire a
channel between it and the main event loop. No independent value on its
own — this is the load-bearing dependency for every other server-side
epic.

- [ ] Add tokio + axum (or equivalent) dependency.
- [ ] Dedicated server thread, started/stopped by the on/off toggle.
- [ ] `mpsc` channel wiring: remote requests marshalled onto the main
      loop, responses/state pushed back the same way.
- [ ] Toggle surfaced in the dashboard/leader menu (state only — LAN vs.
      tunnel indicator comes with the tunnel work in Epic 4/9).

Verification: server starts/stops cleanly with the toggle and doesn't
block or slow the existing 50ms/250ms poll loop; a test exercises
start/stop without a real network client.

### Epic 2 — Device storage (P0)

Fully independent of Epic 1 — this is a data-model addition only.
Safe to build first or in parallel.

- [ ] `remote_devices` migration, following the existing `MIGRATION_0xx`
      convention.
- [ ] `src/db/remote_devices.rs` (or similar): create, lookup by token,
      revoke, update last-seen.

Verification: unit tests for create/lookup/revoke, consistent with the
`#[cfg(test)]` convention in `app/tests.rs`.

### Epic 3 — Native app groundwork (P0)

Front-loads the app-store-adjacent lead time the Flutter decision adds
(account approval, signing, CI) so it isn't discovered as a blocker
mid-Phase-1. Fully independent of Epics 1–2; can start immediately.

- [ ] Flutter project scaffold (iOS + Android targets).
- [ ] Apple Developer account + Google Play Console account (or confirm
      existing ones can be used).
- [ ] Code signing set up for both platforms.
- [ ] TestFlight / Play Console internal-testing track configured for
      installing dev builds on a real phone without a store release.
- [ ] Minimal CI build (or documented local build steps) producing an
      installable artifact for each platform.

Verification: an empty scaffold app installs on a real iOS and Android
device via TestFlight/internal testing.

### Epic 4 — Pairing flow (P1)

Needs Epic 1 (server to expose the exchange endpoint) and Epic 2
(device storage).

- [ ] One-time pairing code generation + QR rendering on desktop.
- [ ] Code exchange endpoint issuing a per-device token.
- [ ] Rate-limiting/lockout on repeated failed pairing attempts.
- [ ] Pairing dialog UI (QR + code + status) on desktop.

Verification: automated tests for token issuance, invalid/expired code
rejection, and lockout after repeated failures; one manual pass pairing
a real phone.

### Epic 5 — Status/notification relay (P1)

Needs Epic 1 only — the read-only relay logic can be built and tested
against a local client before pairing/auth exists, though it should be
gated behind auth (Epic 4) before being exposed on a real network.

- [ ] Read-only status/notification endpoint (WebSocket or polling)
      sourced from the existing `app/notifications.rs` scan.
- [ ] Independent of the remote-control toggle's on/off state, per the
      notification/toggle split in Architecture.
- [ ] Auth-gated once Epic 4 lands (do not ship unauthenticated on a
      real network).

Verification: automated test that a simulated attention event is
delivered over the channel; manual check that a live AMF instance's
status is visible over the relay.

### Epic 6 — App shell + push (P1)

Needs Epic 3 (scaffold) to exist at all; needs Epic 4 for a real pairing
flow and Epic 5 for real status data, though UI scaffolding for both
screens can be built against mocked data in parallel with those landing.

- [ ] Pairing/scan screen.
- [ ] Status/notification list screen (Phase 1 view).
- [ ] Firebase Cloud Messaging integration for attention push
      notifications.

Verification: manual install/pairing on a real phone; confirm a
notification triggered by a real agent question arrives.

### Epic 7 — Device revoke (P1)

Needs Epic 4 (pairing/token model).

- [ ] Revoke action in the desktop paired-devices list.
- [ ] Revoke closes any active connection for that device immediately
      (not just on next reconnect).
- [ ] Revoked token rejected on all subsequent requests.

Verification: automated test that a revoked token is rejected; manual
test that an open connection is torn down on revoke.

### Epic 8 — Prompt response (P2)

Needs the full Phase 1 stack (Epics 1, 5, 6) — this extends the same
server/app surfaces rather than introducing new ones.

- [ ] Endpoint for reading an agent's pending question.
- [ ] Endpoint for submitting a response, without full terminal access.
- [ ] Prompt-response view in the Flutter app.

Verification: automated test round-tripping a captured question/answer;
manual test answering a real agent prompt from the phone.

### Epic 9 — Terminal streaming, backend (P3)

Needs Epic 1 (server) and Epic 4 (auth) — full terminal access is the
highest-privilege capability in this plan and must not ship
unauthenticated. Independent of Epic 8 (prompt response); could be
built in parallel with it if capacity allows, though sequencing after
Phase 2 keeps risk ordered from lowest to highest privilege.

- [ ] Stream `TmuxManager::capture_pane_ansi` output over WebSocket.
- [ ] Forward phone keystrokes back through the existing
      `send_literal`/`send_key_name` paths.
- [ ] Shared read/write with local access, no conflict handling
      (matching the concurrent-access decision).

Verification: manual test typing from both phone and desktop into the
same session; automated test on the send/receive framing logic.

### Epic 10 — Client rendering modes (P3)

Needs Epic 6 (app shell) and Epic 9 (terminal backend).

- [ ] xterm.js full-terminal view via embedded WebView.
- [ ] Native simplified mobile view.
- [ ] User toggle between the two.

Verification: manual check of both views over both LAN and tunnel
connections.

## Parallelization view

- **Start immediately, in parallel**: Epic 1 (server skeleton), Epic 2
  (device storage), Epic 3 (native app groundwork). None depend on each
  other; Epic 3 in particular has the longest external lead time (store
  account approval) so starting it early avoids it becoming a late
  blocker.
- **Once Epic 1 lands**: Epic 5 (status relay) can start; Epic 4
  (pairing) can start once Epic 2 also lands.
- **Once Epic 3 lands**: Epic 6's UI scaffolding can start against
  mocked data, ahead of Epic 4/5 landing for real.
- **Once Epic 4 lands**: Epic 7 (revoke) and Epic 9 (terminal backend)
  can both start; they don't depend on each other.
- **Once Epics 1, 5, 6 all land** (end of the Phase 1 cluster): Epic 8
  (prompt response) can start.
- **Once Epics 6 and 9 land**: Epic 10 (client rendering modes) can
  start, closing out Phase 3.

## Risks / open questions

- Scope boundaries, target users/entry points beyond "the user's own
  phone," data-persistence/retention policy for remote sessions, and a
  definition-of-done for v1 were not covered by the original interview
  — clarify before Epic 1 finishes.
- Whether Phase 1 (Epics 1–7) ships to real usage on its own before
  Phases 2/3 exist, or nothing ships until all epics land, is
  unresolved and affects how "done" is judged per phase.
- Whether the on/off toggle state persists across AMF restarts or
  always resets to off is unresolved.
- Whether two phones can hold simultaneous full-terminal (Epic 9/10)
  access to the same session, in addition to the settled
  single-phone-plus-local case, is unresolved.
- No defined behavior for an active phone connection when the
  underlying tmux session/feature is stopped or deleted locally.
- No token expiry policy is defined — unclear if per-device tokens are
  valid indefinitely until manually revoked.
- No measurable latency/responsiveness threshold is defined for
  terminal control over LAN or tunnel; verification in this plan is
  manual/qualitative only.
- The notification/toggle split proposed in Architecture (detection
  always-on, interactive server on-demand) is this plan's proposed
  resolution to a real tension between two settled decisions, not
  something the user explicitly confirmed — needs sign-off.
- Shipping a Flutter native app instead of a PWA adds app-store-adjacent
  overhead the interview didn't originally scope for: Apple
  Developer / Google Play accounts, code signing, TestFlight/Play
  Console internal testing, and store review turnaround for any future
  update. Epic 3 exists specifically to front-load this rather than
  discover it mid-Phase-1.
- The tunnel mechanism is resolved to "integrate with an existing tool"
  (Tailscale, ngrok, or cloudflared), but *which one* to document/support
  first is still open — pick it when Epic 9 (or a LAN/tunnel toggle in
  Epic 1) needs a concrete integration target.

## Reasoning / when to build

Build when phone-based monitoring/steering of AMF sessions is a workflow
actually wanted — the phased structure lets Phase 1 (read-only status +
push) ship and prove value before committing to the higher-risk Phase 3
terminal-streaming work. Epics 1–3 (P0) are worth starting even before
full commitment to later phases, since they are foundational,
independent of each other, and de-risk the two hardest parts of the
plan early: introducing AMF's first async runtime, and the native-app
distribution lead time.
